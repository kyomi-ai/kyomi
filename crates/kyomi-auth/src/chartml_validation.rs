// SPDX-License-Identifier: AGPL-3.0-or-later
//! Shared authorized datasource context for ChartML validation and query execution.
use kyomi_core::chartml_validation::SqlFailure;
use std::sync::Arc;
#[derive(Clone)]
pub struct QueryContext {
    /// PostgreSQL connection pool.
    pub db: kyomi_core::DbPool,
    /// ID of the user making the request.
    pub user_id: String,
    /// ID of the user's active workspace.
    pub workspace_id: String,
    /// AES-256-GCM encryption key for credential decryption.
    pub encryption_key: Arc<[u8; 32]>,
    /// Application configuration (needed for Google OAuth client credentials).
    pub config: Arc<kyomi_core::Config>,
    /// Connect registry for routing queries through Kyomi Connect instances.
    /// `None` when Connect is not available (e.g., lightweight callers).
    pub connect_registry: Option<kyomi_datasource_server::ConnectRegistry>,
}
fn safe_context_error(operation: &str, error: &kyomi_core::Error) -> String {
    tracing::error!(%error, operation, "ChartML datasource context failure");
    format!("Failed to {operation}: {}", kyomi_core::sanitize_error(error.user_message()))
}
pub async fn resolve_credentials(
    ctx: &QueryContext,
    ds: &kyomi_core::models::datasource::DatasourceConfig,
    ds_type: &kyomi_core::datasource_registry::DatasourceType,
    connection_config: &serde_json::Value,
) -> kyomi_core::Result<serde_json::Value> {
    let is_shared =
        crate::datasource_auth_service::is_shared_auth(ds_type.as_str(), connection_config);

    if is_shared {
        // Shared auth: credentials live in connection_config.
        // The factory's resolve_shared_credentials() will extract them.
        // `service_account` (the registry default since KYO-704) and
        // `enterprise_oauth` are both handled that way, below.
        //
        // Special case: BigQuery `kyomi_oauth` needs the user's Google OAuth
        // token from users.oauth_data (not from datasource credentials).
        // KYO-704: `kyomi_oauth` is retired and no longer selectable for a
        // new or re-saved datasource — this branch only still fires for a
        // pre-KYO-704 row whose stored `auth_mode` literally names it.
        // `ensure_valid_google_token` below refreshes whatever token that
        // user already granted; it does not request any new scopes, so
        // this deliberately isn't gated the same way the connect/enable
        // paths now are (KYO-704 phase A, KYO-739).
        //
        // Deliberately NOT `.unwrap_or(BIGQUERY_DEFAULT_AUTH_MODE)` —
        // an absent `auth_mode` must never match `"kyomi_oauth"` here
        // regardless of what the registry default is, so the empty-string
        // sentinel stays correct on its own.
        let auth_mode = connection_config
            .get("auth_mode")
            .and_then(|v| v.as_str())
            .unwrap_or("");

        if ds_type.as_str() == "bigquery"
            && auth_mode == "kyomi_oauth"
            && let (Some(client_id), Some(client_secret)) = (
                ctx.config.google_oauth_client_id.as_deref(),
                ctx.config.google_oauth_client_secret.as_deref(),
            )
        {
            let tokens = crate::google_oauth::ensure_valid_google_token(
                &ctx.db,
                &ctx.user_id,
                &ctx.encryption_key,
                client_id,
                client_secret,
            )
            .await?;
            let oauth_data = crate::google_oauth::OAuthData {
                google_oauth_tokens: Some(tokens),
                ..Default::default()
            };

            // Also load per-user credentials so that billing_project is
            // available to resolve_billing_project() downstream. Without
            // this, the per-user billing project stored in
            // user_datasource_credentials is invisible to the BigQuery
            // factory and the query fails.
            let mut result = if let Some(cred) =
                crate::datasource_service::get_user_credential(&ctx.db, &ctx.user_id, &ds.id)
                    .await?
            {
                crate::encryption::decrypt_json(&cred.credentials, &ctx.encryption_key)?
            } else {
                serde_json::json!({})
            };

            result["oauth_data"] = serde_json::json!(oauth_data);
            return Ok(result);
        }

        Ok(serde_json::json!({}))
    } else {
        // Personal auth: decrypt per-user credentials
        let cred = crate::datasource_service::get_user_credential(&ctx.db, &ctx.user_id, &ds.id)
            .await?
            .ok_or_else(|| {
                kyomi_core::Error::NotFound("No credentials found for this datasource".into())
            })?;
        let decrypted = crate::encryption::decrypt_json(&cred.credentials, &ctx.encryption_key)?;

        // OAuth refresh if needed. `connection_config` is already decrypted
        // (see this function's doc) — `ensure_valid_oauth_credentials` needs
        // plaintext to refresh against the provider's token endpoint.
        let refreshed = kyomi_datasource_server::oauth_refresh::ensure_valid_oauth_credentials(
            &decrypted,
            connection_config,
            ds_type,
        )
        .await?;

        Ok(refreshed)
    }
}
pub async fn create_provider_for_datasource(
    ctx: &QueryContext,
    ds: &kyomi_core::models::datasource::DatasourceConfig,
) -> Result<Box<dyn kyomi_datasource_server::DatasourceProvider>, String> {
    if ds.connection_type == "connect" {
        // Connect datasources route through the WebSocket registry —
        // no credentials needed (the Connect agent has direct DB access).
        let registry = ctx.connect_registry.as_ref().ok_or_else(|| {
            format!(
                "Datasource '{}' uses Kyomi Connect but the Connect registry is not available",
                ds.slug
            )
        })?;
        Ok(Box::new(kyomi_datasource_server::ConnectProvider::new(
            registry.clone(),
            ds.id.clone(),
        )))
    } else {
        // Direct datasources: resolve credentials and create provider via factory.
        let ds_type: kyomi_core::datasource_registry::DatasourceType = ds.datasource_type.into();

        // `ds.connection_config` came straight from the database and may
        // hold encrypted `COMMON_SENSITIVE` fields, including
        // `oauth_client_secret` (KYO-786) — every driver, and
        // `resolve_credentials`'s own OAuth-refresh step below, needs
        // plaintext. Decrypted once here and threaded through both, rather
        // than each decrypting `ds.connection_config` separately.
        let decrypted_config = crate::credential_service::decrypt_connection_config_secrets(
            &ds.connection_config,
            &ctx.encryption_key,
        )
        .map_err(|e| safe_context_error("decrypt connection config", &e))?;

        let credentials = resolve_credentials(ctx, ds, &ds_type, &decrypted_config)
            .await
            .map_err(|e| safe_context_error("resolve credentials", &e))?;

        let user_context = kyomi_datasource_server::factory::UserContext {
            oauth_data: credentials.get("oauth_data").cloned(),
            user_email: String::new(),
            workspace_id: ctx.workspace_id.clone(),
        };

        kyomi_datasource_server::factory::create_provider(
            &ds_type,
            &decrypted_config,
            &credentials,
            Some(&user_context),
        )
        .await
        .map_err(|e| format!("Failed to create provider for '{}': {e}", ds.slug))
    }
}
pub async fn dry_run_datasource_query(
    ctx: &QueryContext,
    datasource_slug: &str,
    sql: &str,
) -> Result<(), String> {
    dry_run_datasource_query_detailed(ctx, datasource_slug, sql)
        .await
        .map_err(|e| e.message)
}

pub async fn dry_run_datasource_query_detailed(
    ctx: &QueryContext,
    datasource_slug: &str,
    sql: &str,
) -> Result<(), SqlFailure> {
    let ds = crate::datasource_service::resolve_datasource(
        &ctx.db,
        datasource_slug,
        &ctx.workspace_id,
        false,
    )
    .await
    .map_err(|e| {
        SqlFailure::new(
            "sql_datasource",
            safe_context_error("resolve datasource", &e),
        )
    })?;

    let provider = create_provider_for_datasource(ctx, &ds)
        .await
        .map_err(|e| SqlFailure::new("sql_provider", kyomi_core::sanitize_error(&e)))?;

    validate_provider_query(provider.as_ref(), sql).await
}

/// Exercise only the provider's dry-run operation; always release resources.
pub async fn validate_provider_query(
    provider: &dyn kyomi_datasource_server::DatasourceProvider,
    sql: &str,
) -> Result<(), SqlFailure> {
    let result =
        tokio::time::timeout(std::time::Duration::from_secs(30), provider.dry_run(sql)).await;
    provider.close().await;
    let result = result.map_err(|_| SqlFailure::new("sql_timeout", "SQL dry-run timed out"))?;
    log_dry_run_issue(&result);
    let result = sanitize_dry_run_result(result).map_err(|e| SqlFailure::new("sql_dry_run", e))?;
    if result.message == "Dry run not available for this provider" {
        Err(SqlFailure::new("sql_unavailable", result.message))
    } else if result.valid {
        Ok(())
    } else {
        Err(SqlFailure::new("sql", result.message))
    }
}

pub fn log_dry_run_issue(
    result: &kyomi_connect_protocol::Result<kyomi_datasource_server::DryRunResult>,
) {
    match result {
        Ok(result) if !result.valid => {
            tracing::warn!(
                message = %result.message,
                "Datasource SQL dry run validation failed"
            );
        }
        Err(error) => tracing::warn!(error = %error, "Datasource SQL dry run failed"),
        _ => {}
    }
}

pub fn sanitize_dry_run_result(
    result: kyomi_connect_protocol::Result<kyomi_datasource_server::DryRunResult>,
) -> Result<kyomi_datasource_server::DryRunResult, String> {
    match result {
        Ok(mut result) => {
            result.message = kyomi_core::sanitize_error(&result.message);
            Ok(result)
        }
        Err(error) => Err(kyomi_core::sanitize_error(&format!(
            "Dry-run failed: {error}"
        ))),
    }
}

pub async fn validate_content(
    content: &str,
    context: Option<&QueryContext>,
) -> kyomi_core::Result<()> {
    let errors = kyomi_core::chartml_validation::validate_blocks(
        &kyomi_core::chartml_validation::markdown_blocks(content),
        |source| async move {
            let context = context.ok_or_else(|| {
                SqlFailure::new(
                    "sql_unavailable",
                    "SQL dry-run unavailable: validation requires an authorized datasource context",
                )
            })?;
            dry_run_datasource_query_detailed(
                context,
                source.datasource.as_deref().expect("checked datasource"),
                &source.sql,
            )
            .await
        },
    )
    .await;
    if errors.is_empty() {
        Ok(())
    } else {
        Err(kyomi_core::Error::BadRequest(
            errors
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
                .join("; "),
        ))
    }
}

#[cfg(test)]
#[path = "chartml_validation_tests.rs"]
mod tests;
