// SPDX-License-Identifier: AGPL-3.0-or-later

//! BigQuery catalog indexer.
//!
//! BigQuery catalog indexing uses the BigQuery REST API (datasets.list,
//! tables.list, tables.get), NOT SQL queries. This indexer resolves
//! credentials based on the configured `auth_mode`, resolves which
//! project(s) to index (see "Project Scope" below), then delegates to
//! `UserDatasetIndexer::index_workspace_catalog()` for the actual REST
//! API work.
//!
//! ## Project Scope (KYO-444)
//!
//! `connection_config["catalog_projects"]` has three possible states,
//! mirroring the three states `get_catalog_containers`
//! (`kyomi_agent::catalog::traits`) distinguishes for every SQL indexer:
//!
//! | State | Meaning |
//! |---|---|
//! | absent, or `null` | discover all projects accessible to the resolved access token |
//! | `[]` | the user explicitly cleared the project list — index nothing |
//! | `[...]` with entries | index exactly those projects |
//!
//! Before KYO-444 this indexer collapsed "absent" into the same "index
//! nothing" outcome as `[]`, so a brand-new datasource (which never has the
//! key at all — the create modal only writes it when the optional catalog
//! picker is filled in) indexed nothing, forever, indistinguishable from a
//! deliberate empty selection. See [`classify_configured_project_scope`].
//!
//! Every terminal path here — skip, failure, or a successful delegation to
//! `UserDatasetIndexer` — writes a `catalog_refresh_status` the Catalog tab
//! renders, so a run that gave up looks different from a run that never
//! started.
//!
//! ## Auth Modes
//!
//! - **kyomi_oauth** (default) — user connected via Kyomi's Google OAuth.
//!   Tokens are stored in the user's `oauth_data`. Refreshed via
//!   `ensure_valid_google_token()`.
//!
//! - **enterprise_oauth** — workspace-level OAuth with per-datasource
//!   client credentials in `connection_config`. Refreshed via
//!   `ensure_valid_oauth_credentials()`.
//!
//! - **service_account** — GCP service account JSON in `connection_config`.
//!   Token exchanged via `exchange_service_account_jwt()`.
//!
//! ### Token resolution failures (KYO-449)
//!
//! Each of the three branches above can fail to produce an access token —
//! an expired/revoked OAuth grant, a rotated or under-scoped service
//! account, missing workspace OAuth client credentials. Before KYO-449
//! these three branches `return`ed a `CatalogIndexResult::error(..)`
//! straight to the caller without ever calling `update_datasource_status`,
//! so the failure was silent: the Catalog tab showed zero tables, "Last
//! indexed: never", no error — the same silence KYO-444 fixed on the
//! project-scope branches below, just on an earlier early-exit path. All
//! three now go through [`record_failure`] with a message worded for that
//! specific auth mode (a rotated service account reads differently from an
//! expired user OAuth token that needs reconnecting), so the reason is both
//! visible and actionable in the Catalog tab.

use async_trait::async_trait;
use kyomi_core::datasource_registry::DatasourceType;
use kyomi_embed::EmbeddingService;
use serde_json::Value;
use tracing::{info, warn};

use crate::catalog::traits::CatalogIndexer;
use kyomi_auth::catalog::helpers::{
    update_datasource_last_refresh, update_datasource_status, IndexerContext,
};
use kyomi_auth::catalog::indexers::user_dataset::UserDatasetIndexer;
use kyomi_auth::catalog::types::CatalogIndexResult;
use kyomi_auth::google_oauth::list_active_google_projects;

/// BigQuery catalog indexer.
///
/// Resolves an access token based on `auth_mode`, resolves the project
/// scope to index (see the module doc's "Project Scope" table), and
/// delegates to `UserDatasetIndexer::index_workspace_catalog()`.
pub struct BigQueryIndexer;

#[async_trait]
impl CatalogIndexer for BigQueryIndexer {
    async fn index_catalog(
        &self,
        ctx: &IndexerContext,
        db: &kyomi_core::DbPool,
        embedding: &EmbeddingService,
        user_email: Option<&str>,
        credentials: Option<&Value>,
        max_tables_per_dataset: Option<usize>,
    ) -> CatalogIndexResult {
        let auth_mode = ctx
            .connection_config
            .get("auth_mode")
            .and_then(|v| v.as_str())
            .unwrap_or("kyomi_oauth");

        info!(
            workspace_id = ctx.workspace_id,
            datasource_config_id = ctx.datasource_config_id,
            auth_mode,
            "BigQuery background indexer starting"
        );

        // 1. Resolve access token based on auth_mode.
        //
        // KYO-449: each failure branch here used to `return
        // CatalogIndexResult::error(...)` directly — bypassing
        // `update_datasource_status` entirely, so a token-resolution
        // failure left `catalog_refresh_status` at whatever it was before
        // (usually never-set) and the Catalog tab showed zero tables, "Last
        // indexed: never", no error — the exact silence KYO-444 fixed on
        // the project-scope branches. Routing through `record_failure`
        // writes a visible `"failed"` status with a per-auth-mode,
        // actionable reason, and — matching every other `record_failure`
        // call site — deliberately does NOT stamp `last_catalog_refresh`,
        // so a broken datasource doesn't look freshly indexed and isn't
        // rate-limited out of hourly retries by `can_refresh_now`.
        let access_token = match auth_mode {
            "service_account" => {
                match resolve_service_account_token(&ctx.connection_config).await {
                    Ok(token) => token,
                    Err(e) => {
                        let msg = format!(
                            "BigQuery service account authentication failed: {e}. The \
                             service account JSON in datasource settings may have been \
                             rotated, revoked, or lost a required IAM role — verify it \
                             in the Google Cloud console and update it in datasource \
                             settings if needed."
                        );
                        return record_failure(db, ctx, &msg).await;
                    }
                }
            }
            "enterprise_oauth" => {
                match resolve_enterprise_oauth_token(
                    db,
                    ctx,
                    user_email,
                    credentials,
                )
                .await
                {
                    Ok(token) => token,
                    Err(e) => {
                        let msg = format!(
                            "BigQuery enterprise OAuth credentials failed to resolve: {e}. \
                             The workspace's OAuth client credentials for this datasource \
                             may be invalid or revoked — reconfigure them in datasource \
                             settings."
                        );
                        return record_failure(db, ctx, &msg).await;
                    }
                }
            }
            _ => {
                match resolve_kyomi_oauth_token(db, ctx, user_email).await {
                    Ok(token) => token,
                    Err(e) => {
                        let msg = format!(
                            "BigQuery's connected Google account needs to be reconnected: \
                             {e}. Reconnect this datasource's Google account in datasource \
                             settings to restore catalog indexing."
                        );
                        return record_failure(db, ctx, &msg).await;
                    }
                }
            }
        };

        // 2. Resolve which project(s) to index (KYO-444 — see the module
        // doc for the three-state table this mirrors). Extracted into
        // `resolve_project_scope` so the decision — including the
        // populated-but-filters-to-empty case — is directly unit-testable
        // without needing a real access token.
        let catalog_projects = match resolve_project_scope(db, ctx, &access_token).await {
            Ok(projects) => projects,
            Err(result) => return *result,
        };

        info!(
            workspace_id = ctx.workspace_id,
            projects = ?catalog_projects,
            "BigQuery indexing {} project(s)",
            catalog_projects.len()
        );

        // 3. Delegate to UserDatasetIndexer for the actual REST API work
        UserDatasetIndexer::index_workspace_catalog(
            db,
            embedding,
            &ctx.workspace_id,
            &ctx.datasource_config_id,
            &access_token,
            &catalog_projects,
            max_tables_per_dataset,
        )
        .await
    }
}

// ─── Project scope resolution (KYO-444) ────────────────────────────────────────

/// The three states BigQuery's `catalog_projects` scope key in
/// `connection_config` can be in, mirroring the three states
/// `get_catalog_containers` (`kyomi_agent::catalog::traits`) distinguishes
/// for every SQL indexer. Before KYO-444, this indexer derived its decision
/// from `catalog_projects.is_empty()` alone, which cannot tell "the key is
/// absent" apart from "the key is `[]`" — both produced an empty `Vec` and
/// both were treated as "index nothing". Representing `DiscoverAll` and
/// `ExplicitlyEmpty` as distinct enum variants (rather than re-deriving the
/// distinction from `is_empty()` at each call site) makes *that* collapse a
/// type error, not just a code-review nit.
///
/// `Explicit`'s own empty case is a narrower guarantee: a populated key that
/// filters down to zero valid project IDs (e.g. `[123, true, null]`) still
/// produces `Explicit(vec![])`, indistinguishable at the type level from a
/// real, non-empty `Explicit`. Only the runtime `is_empty()` guard in
/// `resolve_project_scope` — not this enum — keeps that case from silently
/// proceeding to index zero projects.
#[derive(Debug, PartialEq, Eq)]
enum ConfiguredProjectScope {
    /// Key absent, or present as JSON `null` — discover all projects
    /// accessible to the resolved access token.
    DiscoverAll,
    /// Key present as `[]` — the user explicitly cleared the project list.
    /// Index nothing; this is a genuine skip, not a defect.
    ExplicitlyEmpty,
    /// Key present with one or more entries — index exactly those projects.
    /// May still be `Explicit(vec![])` after filtering out non-string
    /// entries; `resolve_project_scope` is what checks for that and skips
    /// rather than proceeding to index zero projects.
    Explicit(Vec<String>),
}

/// Classify `connection_config["catalog_projects"]` into the three states
/// above. Pure and synchronous so it's directly unit-testable without a
/// network call, following the shape of `bq_kyomi_oauth_access_gate_satisfied`
/// / `connection_step_satisfied_from` (`kyomi-ui/src/pages/settings/datasources.rs`).
fn classify_configured_project_scope(connection_config: &Value) -> ConfiguredProjectScope {
    match connection_config.get("catalog_projects") {
        None | Some(Value::Null) => ConfiguredProjectScope::DiscoverAll,
        Some(Value::Array(arr)) if arr.is_empty() => ConfiguredProjectScope::ExplicitlyEmpty,
        Some(Value::Array(arr)) => {
            let projects: Vec<String> = arr
                .iter()
                .filter_map(|v| v.as_str().map(String::from))
                .collect();
            ConfiguredProjectScope::Explicit(projects)
        }
        // Unexpected type (e.g. a stray string or number) — discover all as
        // a fallback, mirroring `get_catalog_containers`'s treatment of the
        // same case.
        Some(_) => ConfiguredProjectScope::DiscoverAll,
    }
}

/// Resolve which BigQuery project(s) `index_catalog` should index, or
/// short-circuit with a terminal `CatalogIndexResult` (a recorded skip or
/// failure) for the KYO-444 three-state handling.
///
/// Returns `Ok(projects)` — always non-empty — when indexing should
/// proceed, or `Err(result)` for every early-return case, so the caller can
/// simply propagate `result`. Splitting this out of `index_catalog` makes
/// the decision testable on its own: in particular, `Explicit(_)` filtering
/// down to zero valid project IDs never reaches `list_active_google_projects`
/// at all, so this is directly exercisable with a throwaway `access_token`
/// that would fail if the network call were ever made.
async fn resolve_project_scope(
    db: &kyomi_core::DbPool,
    ctx: &IndexerContext,
    access_token: &str,
) -> Result<Vec<String>, Box<CatalogIndexResult>> {
    match classify_configured_project_scope(&ctx.connection_config) {
        ConfiguredProjectScope::Explicit(projects) if !projects.is_empty() => Ok(projects),
        ConfiguredProjectScope::Explicit(_) => Err(Box::new(
            record_skip(
                db,
                ctx,
                "BigQuery catalog_projects contained no valid project IDs — nothing to index.",
            )
            .await,
        )),
        ConfiguredProjectScope::ExplicitlyEmpty => Err(Box::new(
            record_skip(
                db,
                ctx,
                "BigQuery catalog indexing configured with zero projects \
                 (catalog_projects: []) — nothing to index.",
            )
            .await,
        )),
        ConfiguredProjectScope::DiscoverAll => {
            info!(
                workspace_id = ctx.workspace_id,
                datasource_config_id = ctx.datasource_config_id,
                "no BigQuery catalog_projects configured, discovering all accessible projects"
            );

            match list_active_google_projects(access_token).await {
                Ok(projects) if projects.is_empty() => Err(Box::new(
                    record_skip(
                        db,
                        ctx,
                        "No BigQuery projects are accessible to this account.",
                    )
                    .await,
                )),
                Ok(projects) => Ok(projects.into_iter().map(|p| p.project_id).collect()),
                Err(e) => {
                    // Commonly `resourcemanager.projects.list` denied — e.g.
                    // a service account scoped only to "BigQuery Job User".
                    // Degrade to a recorded, visible failure rather than a
                    // silent skip or a propagated error, mirroring
                    // `discover_datasource_resources`'s treatment of the
                    // same denial (server_fns/datasources.rs).
                    warn!(
                        workspace_id = ctx.workspace_id,
                        datasource_config_id = ctx.datasource_config_id,
                        error = %e,
                        "BigQuery project discovery failed"
                    );
                    let msg = format!(
                        "Failed to list accessible BigQuery projects: {e}. If this account's \
                         IAM role lacks resourcemanager.projects.list (e.g. \"BigQuery Job \
                         User\"), configure specific projects in datasource settings instead \
                         of relying on discovery."
                    );
                    Err(Box::new(record_failure(db, ctx, &msg).await))
                }
            }
        }
    }
}

/// Record a "skip" outcome: `catalog_refresh_status = idle` with `reason`
/// carried in the progress envelope's `warnings` array (so
/// `get_catalog_stats`'s `extract_refresh_warnings` surfaces it in the
/// Catalog tab — `refresh_failure_reason` only reads `"failed"` runs, so a
/// reason for an `"idle"` run has to travel as a warning instead), then
/// stamp `last_catalog_refresh`.
///
/// Stamping `last_catalog_refresh` here mirrors `index_catalog_sql`'s
/// treatment of an explicitly-empty container scope: a deliberate,
/// nothing-to-do run still counts as a completed refresh. Without this, the
/// Catalog tab cannot distinguish "indexing ran and found nothing to do"
/// from "indexing never ran" — the exact silence KYO-444 exists to fix.
async fn record_skip(
    db: &kyomi_core::DbPool,
    ctx: &IndexerContext,
    reason: &str,
) -> CatalogIndexResult {
    let _ = update_datasource_status(
        db,
        &ctx.workspace_id,
        &ctx.datasource_config_id,
        "idle",
        None,
        None,
        &[reason.to_string()],
    )
    .await;
    let _ = update_datasource_last_refresh(db, &ctx.datasource_config_id).await;

    CatalogIndexResult::skipped(reason).with_ids(&ctx.datasource_config_id, &ctx.workspace_id)
}

/// Record a "failed" outcome: `catalog_refresh_status = failed` with
/// `message` as the recorded error (read by `get_catalog_stats` as
/// `refresh_failure_reason`). Does **not** stamp `last_catalog_refresh` —
/// nothing was refreshed, mirroring `index_catalog_sql`'s treatment of a
/// discovery failure (e.g. a schema-listing permission error), which also
/// leaves the timestamp untouched.
async fn record_failure(
    db: &kyomi_core::DbPool,
    ctx: &IndexerContext,
    message: &str,
) -> CatalogIndexResult {
    let _ = update_datasource_status(
        db,
        &ctx.workspace_id,
        &ctx.datasource_config_id,
        "failed",
        None,
        Some(message),
        &[],
    )
    .await;

    CatalogIndexResult::error(message).with_ids(&ctx.datasource_config_id, &ctx.workspace_id)
}

// ─── Auth mode token resolvers ─────────────────────────────────────────────────

/// Resolve access token for `service_account` auth mode.
///
/// Reads `service_account_json` from `connection_config` and exchanges
/// a signed JWT for a short-lived access token.
async fn resolve_service_account_token(
    connection_config: &Value,
) -> Result<String, String> {
    let client = kyomi_auth::http_client().map_err(|e| format!("{e}"))?;

    let (token, _project_id) =
        kyomi_datasource_server::providers::bigquery::exchange_service_account_jwt(
            &client,
            connection_config,
        )
        .await
        .map_err(|e| format!("{e}"))?;

    Ok(token)
}

/// Resolve access token for `enterprise_oauth` auth mode.
///
/// Uses `ensure_valid_oauth_credentials()` to refresh the token if expired,
/// then extracts `oauth_access_token`. Persists refreshed credentials back
/// to the database.
async fn resolve_enterprise_oauth_token(
    db: &kyomi_core::DbPool,
    ctx: &IndexerContext,
    user_email: Option<&str>,
    provided_credentials: Option<&Value>,
) -> Result<String, String> {
    // Resolve credentials (provided → shared → stored user creds)
    let credentials = crate::catalog::traits::resolve_indexing_credentials(
        db,
        ctx,
        user_email,
        provided_credentials,
    )
    .await
    .ok_or("No credentials available for enterprise_oauth BigQuery")?;

    // Refresh if expired
    let refreshed = kyomi_datasource_server::ensure_valid_oauth_credentials(
        &credentials,
        &ctx.connection_config,
        &DatasourceType::BigQuery,
    )
    .await
    .map_err(|e| format!("{e}"))?;

    // If credentials changed (token was refreshed), persist them back
    if refreshed != credentials
        && let Some(email) = user_email
        && let Some(user_id) = resolve_user_id(db, email).await
    {
        let _ = kyomi_auth::datasource_service::save_user_credential(
            db,
            &ctx.encryption_key,
            &user_id,
            &ctx.datasource_config_id,
            &ctx.workspace_id,
            &refreshed,
        )
        .await;
        info!(
            user_id,
            "Persisted refreshed enterprise_oauth credentials"
        );
    }

    // Extract the access token
    refreshed
        .get("oauth_access_token")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .map(String::from)
        .ok_or_else(|| {
            "enterprise_oauth credentials missing oauth_access_token after refresh".into()
        })
}

/// Resolve access token for `kyomi_oauth` auth mode (default).
///
/// The user connected via Kyomi's own Google OAuth flow. Tokens are stored
/// in the user's `oauth_data` (encrypted on the `users` table). This
/// function refreshes the token if expired and persists it back.
///
/// Requires `GOOGLE_OAUTH_CLIENT_ID` and `GOOGLE_OAUTH_CLIENT_SECRET`
/// environment variables to be set.
async fn resolve_kyomi_oauth_token(
    db: &kyomi_core::DbPool,
    ctx: &IndexerContext,
    user_email: Option<&str>,
) -> Result<String, String> {
    let email = user_email.filter(|s| !s.is_empty()).ok_or(
        "kyomi_oauth requires a user email for token refresh, but none was provided. \
         Ensure the workspace has an owner.",
    )?;

    let user_id = resolve_user_id(db, email)
        .await
        .ok_or_else(|| format!("User not found for email: {email}"))?;

    // Read Google OAuth client credentials from environment
    let client_id = std::env::var("GOOGLE_OAUTH_CLIENT_ID").map_err(|_| {
        "GOOGLE_OAUTH_CLIENT_ID not set — required for kyomi_oauth background refresh"
    })?;
    let client_secret = std::env::var("GOOGLE_OAUTH_CLIENT_SECRET").map_err(|_| {
        "GOOGLE_OAUTH_CLIENT_SECRET not set — required for kyomi_oauth background refresh"
    })?;

    // ensure_valid_google_token handles: read oauth_data → check expiry → refresh → persist
    let tokens = kyomi_auth::google_oauth::ensure_valid_google_token(
        db,
        &user_id,
        &ctx.encryption_key,
        &client_id,
        &client_secret,
    )
    .await
    .map_err(|e| format!("{e}"))?;

    if tokens.access_token.is_empty() {
        return Err("Google OAuth token is empty after refresh".into());
    }

    Ok(tokens.access_token)
}

// ─── Helpers ───────────────────────────────────────────────────────────────────

/// Look up a user_id from an email address.
async fn resolve_user_id(db: &kyomi_core::DbPool, email: &str) -> Option<String> {
    let row: Option<String> = match db {
        kyomi_core::DbPool::Postgres(pg) => {
            sqlx::query_scalar::<_, String>("SELECT user_id FROM users WHERE email = $1")
                .bind(email)
                .fetch_optional(pg)
                .await
                .ok()
                .flatten()
        }
        kyomi_core::DbPool::Sqlite(sq) => {
            sqlx::query_scalar::<_, String>("SELECT user_id FROM users WHERE email = $1")
                .bind(email)
                .fetch_optional(sq)
                .await
                .ok()
                .flatten()
        }
    };

    row
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bigquery_indexer_exists() {
        // Verify the struct exists and can be instantiated
        let _indexer = BigQueryIndexer;
    }

    // ── classify_configured_project_scope (KYO-444) ─────────────────────
    //
    // These call the real function the indexer uses, not a reimplementation
    // — the pre-fix version of this test module asserted
    // `catalog_projects_empty_when_missing` produced an *empty Vec*, which
    // was itself an assertion of the bug: a missing key must never be
    // treated the same as an explicitly emptied list.

    #[test]
    fn scope_absent_key_discovers_all() {
        let config = serde_json::json!({});
        assert_eq!(
            classify_configured_project_scope(&config),
            ConfiguredProjectScope::DiscoverAll,
            "a brand-new datasource has no catalog_projects key at all (the create modal \
             only writes it when the optional catalog picker is filled in) — it must \
             discover all accessible projects, not be treated as \"index nothing\" (KYO-444)"
        );
    }

    #[test]
    fn scope_null_key_discovers_all() {
        let config = serde_json::json!({"catalog_projects": null});
        assert_eq!(
            classify_configured_project_scope(&config),
            ConfiguredProjectScope::DiscoverAll
        );
    }

    #[test]
    fn scope_explicit_empty_array_skips() {
        let config = serde_json::json!({"catalog_projects": []});
        assert_eq!(
            classify_configured_project_scope(&config),
            ConfiguredProjectScope::ExplicitlyEmpty,
            "an explicitly cleared project list is a genuine skip and must stay \
             distinguishable from an absent key (KYO-444)"
        );
    }

    #[test]
    fn scope_populated_array_indexes_those_projects() {
        let config = serde_json::json!({"catalog_projects": ["project-a", "project-b"]});
        assert_eq!(
            classify_configured_project_scope(&config),
            ConfiguredProjectScope::Explicit(vec![
                "project-a".to_string(),
                "project-b".to_string()
            ])
        );
    }

    /// KYO-444 review follow-up: a *populated* `catalog_projects` key that
    /// filters down to zero valid string project IDs is the original bug
    /// wearing a new hat — a non-empty key that ends up indexing nothing.
    /// `classify_configured_project_scope` alone cannot fail this case (an
    /// enum variant doesn't know it's "empty" the way `is_empty()` does) —
    /// see `resolve_project_scope_skips_when_explicit_scope_filters_to_empty`
    /// below for the routing that actually catches it.
    #[test]
    fn scope_populated_array_with_no_valid_strings_classifies_as_explicit_empty_vec() {
        let config = serde_json::json!({"catalog_projects": [123, true, null]});
        assert_eq!(
            classify_configured_project_scope(&config),
            ConfiguredProjectScope::Explicit(vec![]),
            "a populated catalog_projects key with no valid string entries must still \
             classify as Explicit(vec![]) — it's resolve_project_scope's job to catch \
             that and skip rather than silently proceed to index zero projects"
        );
    }

    /// The core KYO-444 assertion: absent and `[]` must not collapse into
    /// the same outcome. This is the exact bug — both previously produced
    /// an empty `Vec` via `.unwrap_or_default()` and were indistinguishable
    /// by the time `is_empty()` ran.
    #[test]
    fn scope_absent_and_explicitly_empty_remain_distinguishable() {
        let absent = classify_configured_project_scope(&serde_json::json!({}));
        let explicit_empty =
            classify_configured_project_scope(&serde_json::json!({"catalog_projects": []}));
        assert_ne!(
            absent, explicit_empty,
            "absent catalog_projects and an explicit [] must resolve to different scopes"
        );
    }

    // ── record_skip / record_failure write a visible status (KYO-444) ──
    //
    // Uses the same `DbPool::connect("sqlite::memory:")` + real migrations
    // pattern as `catalog::traits`'s tests (see `resolve_indexing_credentials`
    // tests in that module) so these exercise the real
    // `update_datasource_status` / `update_datasource_last_refresh` SQL,
    // not a mock.

    /// Seeds the FK chain a `datasource_configs` row requires: a user, a
    /// workspace they own, then the datasource config itself — same shape
    /// as `catalog::traits::tests::seed_credential_resolution_rows`.
    async fn seed_bq_datasource(sq: &sqlx::SqlitePool, id: &str, workspace_id: &str) {
        let user_id = format!("user-{id}");
        sqlx::query("INSERT INTO users (user_id, email) VALUES (?, ?)")
            .bind(&user_id)
            .bind(format!("{id}@test.local"))
            .execute(sq)
            .await
            .expect("insert user");
        sqlx::query("INSERT INTO workspaces (workspace_id, name, owner_user_id) VALUES (?, 'WS', ?)")
            .bind(workspace_id)
            .bind(&user_id)
            .execute(sq)
            .await
            .expect("insert workspace");
        sqlx::query(
            "INSERT INTO datasource_configs (id, workspace_id, name, datasource_type, slug) \
             VALUES (?, ?, 'BQ', 'bigquery', ?)",
        )
        .bind(id)
        .bind(workspace_id)
        .bind(id)
        .execute(sq)
        .await
        .expect("insert bigquery datasource_config");
    }

    fn bq_ctx(workspace_id: &str, datasource_config_id: &str) -> IndexerContext {
        IndexerContext {
            workspace_id: workspace_id.to_string(),
            datasource_config_id: datasource_config_id.to_string(),
            connection_config: serde_json::json!({}),
            encryption_key: std::sync::Arc::new([0u8; 32]),
        }
    }

    /// Connects a fresh in-memory sqlite pool and seeds the FK chain a
    /// single BigQuery `datasource_configs` row needs. Extracted so the
    /// tests in this module (three from KYO-444, three more from KYO-449)
    /// share one setup path instead of a sixth inline copy of the same
    /// `DbPool::connect` + pattern-match + `seed_bq_datasource` sequence —
    /// see "the third copy of a test helper is the extraction trigger"
    /// (`docs/standards/code-organization/`).
    async fn seeded_bq_db(id: &str, workspace_id: &str) -> kyomi_core::DbPool {
        let db = kyomi_core::DbPool::connect("sqlite::memory:")
            .await
            .expect("connect in-memory sqlite");
        let kyomi_core::DbPool::Sqlite(sq) = &db else {
            unreachable!("expected sqlite pool");
        };
        seed_bq_datasource(sq, id, workspace_id).await;
        db
    }

    /// The MAJOR from the KYO-444 review: a populated-but-filters-to-empty
    /// `catalog_projects` must skip via `resolve_project_scope`, not
    /// silently proceed to index zero projects. Proven by the reviewer to
    /// be a live gap — replacing the `Explicit(_) if !projects.is_empty()`
    /// guard with an unconditional `Explicit(projects) => projects` still
    /// passed every other test in this module.
    ///
    /// Uses a garbage `access_token` deliberately: `Explicit(_)` must never
    /// reach `list_active_google_projects` at all, so if this test ever
    /// timed out or errored on a network call instead of returning `Err(..)`
    /// immediately, that failure mode would itself indicate the guard was
    /// removed.
    #[tokio::test]
    async fn resolve_project_scope_skips_when_explicit_scope_filters_to_empty() {
        let db = seeded_bq_db("ds-filtered", "ws-filtered").await;
        let kyomi_core::DbPool::Sqlite(sq) = &db else {
            unreachable!("expected sqlite pool");
        };
        let ctx = IndexerContext {
            workspace_id: "ws-filtered".to_string(),
            datasource_config_id: "ds-filtered".to_string(),
            connection_config: serde_json::json!({"catalog_projects": [123, true, null]}),
            encryption_key: std::sync::Arc::new([0u8; 32]),
        };

        let outcome = resolve_project_scope(&db, &ctx, "not-a-real-access-token").await;

        let Err(result) = outcome else {
            panic!(
                "a catalog_projects key that filters to zero valid project IDs must skip, \
                 not proceed to index zero projects — got Ok({outcome:?})"
            );
        };
        assert_eq!(result.status, "skipped");

        let status: String =
            sqlx::query_scalar("SELECT catalog_refresh_status FROM datasource_configs WHERE id = ?")
                .bind(&ctx.datasource_config_id)
                .fetch_one(sq)
                .await
                .expect("read catalog_refresh_status");
        assert_eq!(
            status, "idle",
            "a populated-but-invalid catalog_projects key must record a visible skip \
             (KYO-444), not proceed silently"
        );
    }

    /// The heart of the KYO-444 fix: a skip must not reach "no status write
    /// at all" — the exact defect that made a run which deliberately gave
    /// up indistinguishable from a run that never happened.
    #[tokio::test]
    async fn record_skip_writes_idle_status_with_reason_and_stamps_last_refresh() {
        let db = seeded_bq_db("ds-skip", "ws-skip").await;
        let kyomi_core::DbPool::Sqlite(sq) = &db else {
            unreachable!("expected sqlite pool");
        };
        let ctx = bq_ctx("ws-skip", "ds-skip");

        // Sanity check on the seeded fixture: a fresh datasource starts
        // with no refresh timestamp, matching the "Last indexed: never"
        // production symptom — so the post-call assertion below actually
        // proves something changed.
        let before: Option<String> =
            sqlx::query_scalar("SELECT last_catalog_refresh FROM datasource_configs WHERE id = ?")
                .bind(&ctx.datasource_config_id)
                .fetch_one(sq)
                .await
                .expect("read last_catalog_refresh before");
        assert!(
            before.is_none(),
            "expected a freshly seeded datasource to start with no refresh timestamp"
        );

        let result = record_skip(&db, &ctx, "test skip reason").await;
        assert_eq!(result.status, "skipped");

        let status: String =
            sqlx::query_scalar("SELECT catalog_refresh_status FROM datasource_configs WHERE id = ?")
                .bind(&ctx.datasource_config_id)
                .fetch_one(sq)
                .await
                .expect("read catalog_refresh_status");
        assert_eq!(
            status, "idle",
            "a skip must write a status the Catalog tab renders, not leave the column \
             at its default"
        );

        let progress_raw: String = sqlx::query_scalar(
            "SELECT catalog_refresh_progress FROM datasource_configs WHERE id = ?",
        )
        .bind(&ctx.datasource_config_id)
        .fetch_one(sq)
        .await
        .expect("read catalog_refresh_progress");
        let progress: serde_json::Value =
            serde_json::from_str(&progress_raw).expect("progress is JSON");
        let warnings: Vec<&str> = progress["warnings"]
            .as_array()
            .expect("warnings array present")
            .iter()
            .filter_map(|v| v.as_str())
            .collect();
        assert_eq!(
            warnings,
            vec!["test skip reason"],
            "the skip reason must be readable — get_catalog_stats reads \
             catalog_refresh_progress.warnings for a non-\"failed\" status \
             (refresh_failure_reason only reads .error on a \"failed\" run)"
        );

        let after: Option<String> =
            sqlx::query_scalar("SELECT last_catalog_refresh FROM datasource_configs WHERE id = ?")
                .bind(&ctx.datasource_config_id)
                .fetch_one(sq)
                .await
                .expect("read last_catalog_refresh after");
        assert!(
            after.is_some(),
            "a skip must stamp last_catalog_refresh so \"Last indexed\" no longer reads \
             \"never\" for a run that deliberately gave up (KYO-444)"
        );
    }

    /// A `resourcemanager.projects.list` denial must degrade to a recorded,
    /// visible failure — not a silent skip, and not a run that claims to
    /// have refreshed the catalog when nothing was discovered.
    #[tokio::test]
    async fn record_failure_writes_failed_status_and_does_not_stamp_last_refresh() {
        let db = seeded_bq_db("ds-fail", "ws-fail").await;
        let kyomi_core::DbPool::Sqlite(sq) = &db else {
            unreachable!("expected sqlite pool");
        };
        let ctx = bq_ctx("ws-fail", "ds-fail");

        let result = record_failure(&db, &ctx, "resourcemanager.projects.list denied").await;
        assert_eq!(result.status, "error");

        let status: String =
            sqlx::query_scalar("SELECT catalog_refresh_status FROM datasource_configs WHERE id = ?")
                .bind(&ctx.datasource_config_id)
                .fetch_one(sq)
                .await
                .expect("read catalog_refresh_status");
        assert_eq!(
            status, "failed",
            "a resourcemanager.projects.list denial must degrade to a recorded, visible \
             failure, not a silent skip"
        );

        let progress_raw: String = sqlx::query_scalar(
            "SELECT catalog_refresh_progress FROM datasource_configs WHERE id = ?",
        )
        .bind(&ctx.datasource_config_id)
        .fetch_one(sq)
        .await
        .expect("read catalog_refresh_progress");
        let progress: serde_json::Value =
            serde_json::from_str(&progress_raw).expect("progress is JSON");
        assert_eq!(
            progress["error"].as_str(),
            Some("resourcemanager.projects.list denied"),
            "get_catalog_stats reads catalog_refresh_progress.error as refresh_failure_reason \
             for a \"failed\" run"
        );

        let after: Option<String> =
            sqlx::query_scalar("SELECT last_catalog_refresh FROM datasource_configs WHERE id = ?")
                .bind(&ctx.datasource_config_id)
                .fetch_one(sq)
                .await
                .expect("read last_catalog_refresh after");
        assert!(
            after.is_none(),
            "nothing was refreshed on a discovery failure — last_catalog_refresh must stay \
             untouched, mirroring index_catalog_sql's treatment of a discover-containers failure"
        );
    }

    // ── Token-resolution failures write a visible status (KYO-449) ─────
    //
    // Before this fix, all three `auth_mode` branches in `index_catalog`
    // `return`ed `CatalogIndexResult::error(..)` straight to the caller on
    // a token-resolution failure, bypassing `update_datasource_status`
    // entirely — the same silent-failure shape KYO-444 fixed on the
    // project-scope branches, just on this earlier early-exit path. These
    // three tests run the real `BigQueryIndexer::index_catalog` trait
    // method end to end (not `record_failure` directly, and not a
    // reimplementation of the branch) so a regression that reintroduces a
    // bare early return is caught here.
    //
    // Each `connection_config`/`user_email` combination is chosen so the
    // underlying token resolver fails deterministically without ever
    // making a network call or reading an environment variable — verified
    // against each resolver's own logic (see `resolve_service_account_token`,
    // `resolve_enterprise_oauth_token`, `resolve_kyomi_oauth_token` above,
    // and the vendored `kyomi-datasource` v1.6.0 source for
    // `exchange_service_account_jwt`).

    /// Runs `BigQueryIndexer::index_catalog` and reads back the three
    /// pieces of persisted state every test below needs — extracted so the
    /// three KYO-449 tests share one readback path instead of a third and
    /// fourth copy of the same query trio already used inline by the
    /// `record_skip`/`record_failure` tests above.
    async fn run_index_catalog_and_read_status(
        db: &kyomi_core::DbPool,
        sq: &sqlx::SqlitePool,
        ctx: &IndexerContext,
        user_email: Option<&str>,
    ) -> (CatalogIndexResult, String, Option<String>, Option<String>) {
        let embedding = EmbeddingService::new().expect("load embedding model");
        let result = BigQueryIndexer
            .index_catalog(ctx, db, &embedding, user_email, None, None)
            .await;

        let status: String = sqlx::query_scalar(
            "SELECT catalog_refresh_status FROM datasource_configs WHERE id = ?",
        )
        .bind(&ctx.datasource_config_id)
        .fetch_one(sq)
        .await
        .expect("read catalog_refresh_status");

        let progress_raw: String = sqlx::query_scalar(
            "SELECT catalog_refresh_progress FROM datasource_configs WHERE id = ?",
        )
        .bind(&ctx.datasource_config_id)
        .fetch_one(sq)
        .await
        .expect("read catalog_refresh_progress");
        let progress: serde_json::Value =
            serde_json::from_str(&progress_raw).expect("progress is JSON");
        let error = progress["error"].as_str().map(str::to_string);

        let last_refresh: Option<String> = sqlx::query_scalar(
            "SELECT last_catalog_refresh FROM datasource_configs WHERE id = ?",
        )
        .bind(&ctx.datasource_config_id)
        .fetch_one(sq)
        .await
        .expect("read last_catalog_refresh");

        (result, status, error, last_refresh)
    }

    /// `service_account` mode: `connection_config` deliberately omits
    /// `service_account_json`, so `exchange_service_account_jwt` fails at
    /// its very first field lookup — before it would ever build a JWT or
    /// make a network call.
    #[tokio::test]
    async fn service_account_token_failure_writes_failed_status_with_actionable_reason() {
        let db = seeded_bq_db("ds-sa-fail", "ws-sa-fail").await;
        let kyomi_core::DbPool::Sqlite(sq) = &db else {
            unreachable!("expected sqlite pool");
        };
        let ctx = IndexerContext {
            workspace_id: "ws-sa-fail".to_string(),
            datasource_config_id: "ds-sa-fail".to_string(),
            connection_config: serde_json::json!({"auth_mode": "service_account"}),
            encryption_key: std::sync::Arc::new([0u8; 32]),
        };

        let (result, status, error, last_refresh) =
            run_index_catalog_and_read_status(&db, sq, &ctx, None).await;

        assert_eq!(result.status, "error");
        assert_eq!(
            status, "failed",
            "a service_account token-exchange failure must write a visible \"failed\" \
             status (KYO-449) — the pre-fix code returned CatalogIndexResult::error \
             directly and never called update_datasource_status at all"
        );

        let error = error.expect("a \"failed\" run must carry an error reason (get_catalog_stats reads it as refresh_failure_reason)");
        assert!(
            error.to_lowercase().contains("service account"),
            "the reason must be actionable for service_account specifically, not a generic \
             \"credentials failed\" message — got: {error}"
        );
        assert!(
            error.contains("service_account_json"),
            "must still carry the underlying, offline-deterministic cause — got: {error}"
        );
        assert!(
            !error.to_lowercase().contains("oauth client credentials")
                && !error.to_lowercase().contains("reconnect"),
            "the three auth-mode messages must be distinguishable — this one must not reuse \
             the enterprise_oauth or kyomi_oauth wording, got: {error}"
        );

        assert!(
            last_refresh.is_none(),
            "a failed token resolution must NOT stamp last_catalog_refresh — doing so would \
             make a broken datasource look freshly indexed, and would rate-limit hourly \
             retries via can_refresh_now for 24h (helpers.rs:76)"
        );
    }

    /// `enterprise_oauth` mode: no `provided_credentials`, no
    /// `shared_credentials` in `connection_config`, and no `user_email` —
    /// `resolve_indexing_credentials` returns `None` at its very first
    /// `user_email?` short-circuit, before any DB query or network call.
    #[tokio::test]
    async fn enterprise_oauth_token_failure_writes_failed_status_with_actionable_reason() {
        let db = seeded_bq_db("ds-eo-fail", "ws-eo-fail").await;
        let kyomi_core::DbPool::Sqlite(sq) = &db else {
            unreachable!("expected sqlite pool");
        };
        let ctx = IndexerContext {
            workspace_id: "ws-eo-fail".to_string(),
            datasource_config_id: "ds-eo-fail".to_string(),
            connection_config: serde_json::json!({"auth_mode": "enterprise_oauth"}),
            encryption_key: std::sync::Arc::new([0u8; 32]),
        };

        let (result, status, error, last_refresh) =
            run_index_catalog_and_read_status(&db, sq, &ctx, None).await;

        assert_eq!(result.status, "error");
        assert_eq!(
            status, "failed",
            "an enterprise_oauth credential-resolution failure must write a visible \
             \"failed\" status (KYO-449)"
        );

        let error = error.expect("a \"failed\" run must carry an error reason");
        assert!(
            error.to_lowercase().contains("oauth client credentials"),
            "the reason must be actionable for enterprise_oauth specifically — a \
             workspace-level OAuth client credentials problem reads differently from an \
             expired end-user OAuth token — got: {error}"
        );
        assert!(
            error.contains("No credentials available for enterprise_oauth BigQuery"),
            "must still carry the underlying, offline-deterministic cause — got: {error}"
        );
        assert!(
            !error.to_lowercase().contains("service account") && !error.to_lowercase().contains("reconnect"),
            "the three auth-mode messages must be distinguishable — this one must not reuse \
             the service_account or kyomi_oauth wording, got: {error}"
        );

        assert!(
            last_refresh.is_none(),
            "a failed token resolution must NOT stamp last_catalog_refresh"
        );
    }

    /// `kyomi_oauth` mode (the default, catch-all `_` arm): no
    /// `user_email` is provided, so `resolve_kyomi_oauth_token` fails at
    /// its very first check, before any DB query or network call.
    #[tokio::test]
    async fn kyomi_oauth_token_failure_writes_failed_status_with_actionable_reason() {
        let db = seeded_bq_db("ds-ko-fail", "ws-ko-fail").await;
        let kyomi_core::DbPool::Sqlite(sq) = &db else {
            unreachable!("expected sqlite pool");
        };
        let ctx = IndexerContext {
            workspace_id: "ws-ko-fail".to_string(),
            datasource_config_id: "ds-ko-fail".to_string(),
            connection_config: serde_json::json!({"auth_mode": "kyomi_oauth"}),
            encryption_key: std::sync::Arc::new([0u8; 32]),
        };

        let (result, status, error, last_refresh) =
            run_index_catalog_and_read_status(&db, sq, &ctx, None).await;

        assert_eq!(result.status, "error");
        assert_eq!(
            status, "failed",
            "a kyomi_oauth token-resolution failure must write a visible \"failed\" status \
             (KYO-449) — this is the exact scenario from the ticket: an expired user OAuth \
             token must not leave the Catalog tab silently at zero tables / \"Last indexed: \
             never\""
        );

        let error = error.expect("a \"failed\" run must carry an error reason");
        assert!(
            error.to_lowercase().contains("reconnect"),
            "the reason must tell the user to reconnect their Google account — the one \
             concrete action available for kyomi_oauth specifically — got: {error}"
        );
        assert!(
            error.contains("kyomi_oauth requires a user email for token refresh"),
            "must still carry the underlying, offline-deterministic cause — got: {error}"
        );
        assert!(
            !error.to_lowercase().contains("service account")
                && !error.to_lowercase().contains("oauth client credentials"),
            "the three auth-mode messages must be distinguishable — this one must not reuse \
             the service_account or enterprise_oauth wording, got: {error}"
        );

        assert!(
            last_refresh.is_none(),
            "a failed token resolution must NOT stamp last_catalog_refresh"
        );
    }

    #[test]
    fn auth_mode_defaults_to_kyomi_oauth() {
        let config = serde_json::json!({});
        let auth_mode = config
            .get("auth_mode")
            .and_then(|v| v.as_str())
            .unwrap_or("kyomi_oauth");
        assert_eq!(auth_mode, "kyomi_oauth");
    }

    #[test]
    fn auth_mode_reads_from_config() {
        let config = serde_json::json!({"auth_mode": "service_account"});
        let auth_mode = config
            .get("auth_mode")
            .and_then(|v| v.as_str())
            .unwrap_or("kyomi_oauth");
        assert_eq!(auth_mode, "service_account");
    }
}
