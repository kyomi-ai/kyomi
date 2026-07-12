// SPDX-License-Identifier: AGPL-3.0-or-later

//! Server functions for the Connect Setup page.
//!
//! These replace the REST API calls that ConnectSetupPage.jsx makes:
//! - `GET /api/v1/datasources` (filtered by `connection_type == "connect"`) → `list_connect_datasources()`
//! - `POST /api/v1/datasources` (with `connection_type: "connect"`) → `create_connect_datasource()`
//! - `POST /api/v1/datasources/{id}/connect/rotate-token` → `rotate_connect_token()`
//!
//! Calls the same service-layer code as `apps/server/src/routes/datasources.rs`.

use leptos::prelude::*;
use serde::{Deserialize, Serialize};

#[cfg(feature = "ssr")]
use super::{AuthenticatedContext, IntoServerFnError};

// ─── Types ──────────────────────────────────────────────────────────────────

/// A Connect-type datasource summary, returned by the list server function.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ConnectDatasource {
    pub id: String,
    pub name: String,
    pub slug: String,
    pub datasource_type: String,
}

/// Result of creating a Connect datasource — includes the initial token.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CreateConnectResult {
    pub datasource_id: String,
    pub connect_token: String,
}

/// Response for [`connect_status`] — Kyomi Connect agent presence.
///
/// Matches the shape of the REST `GET /{identifier}/connect/status` response:
/// `last_seen` is an ISO-8601/RFC-3339 string when the agent is connected,
/// `None` otherwise.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ConnectStatusResponse {
    pub connected: bool,
    pub last_seen: Option<String>,
}

// ─── Server Functions ───────────────────────────────────────────────────────

/// List datasources filtered to `connection_type == "connect"`.
///
/// Returns only the fields needed by the Connect Setup page's selection list.
/// Mirrors `GET /api/v1/datasources` with client-side filtering in the React page.
#[server(prefix = "/leptos-api")]
pub async fn list_connect_datasources() -> Result<Vec<ConnectDatasource>, ServerFnError> {
    let ac = AuthenticatedContext::extract().await?;

    let datasources = kyomi_auth::datasource_service::list_datasources(ac.db(), &ac.ws_id, false)
        .await
        .into_sfn()?;

    let connect_ds = datasources
        .into_iter()
        .filter(|ds| ds.connection_type == "connect")
        .map(|ds| ConnectDatasource {
            id: ds.id,
            name: ds.name,
            slug: ds.slug,
            datasource_type: ds.datasource_type.to_string(),
        })
        .collect();

    Ok(connect_ds)
}

/// Create a new Connect datasource and return its initial token.
///
/// Mirrors `POST /api/v1/datasources` with `connection_type: "connect"`,
/// followed by automatic token generation (same as the REST route).
#[server(prefix = "/leptos-api")]
pub async fn create_connect_datasource(
    name: String,
    slug: Option<String>,
    datasource_type: String,
) -> Result<CreateConnectResult, ServerFnError> {
    let ac = AuthenticatedContext::extract().await?;

    require_workspace_admin(&ac.auth)?;

    // Validate datasource type is supported
    if !kyomi_core::datasource_registry::is_supported_type(&datasource_type) {
        return Err(ServerFnError::new(format!(
            "Unsupported datasource type: {datasource_type}"
        )));
    }

    // OAuth datasources don't support Connect
    match datasource_type.as_str() {
        "bigquery" | "snowflake" | "databricks" => {
            return Err(ServerFnError::new(format!(
                "Kyomi Connect is not supported for {datasource_type} — use OAuth authentication instead"
            )));
        }
        _ => {}
    }

    // Validate ConnectTokenService is present via ServerContext.connect_token
    // before mutating the database. `None` means Kyomi Connect is not configured
    // on this server — surface that up front so we don't create an orphan
    // datasource row that we can't mint a token for.
    if ac.ctx.connect_token.is_none() {
        return Err(ServerFnError::new(
            "Kyomi Connect is not configured on this server",
        ));
    }

    let slug_ref = slug.as_deref().filter(|s| !s.is_empty());
    let encryption_key = ac.encryption_key()?;

    let ds = kyomi_auth::datasource_service::create_datasource(
        ac.db(),
        kyomi_auth::datasource_service::CreateDatasourceParams {
            workspace_id: &ac.ws_id,
            name: &name,
            slug: slug_ref,
            ds_type: &datasource_type,
            connection_config: serde_json::json!({}),
            connection_type: Some("connect"),
            encryption_key: &encryption_key,
        },
    )
    .await
    .map_err(|e| {
        let msg = e.to_string();
        // Normalize constraint violation errors so the client can reliably detect slug conflicts
        if msg.contains("UNIQUE") || msg.contains("unique") || msg.contains("duplicate key") || msg.contains("already exists") {
            ServerFnError::new("A datasource with this slug already exists in your workspace. Please choose a different name or slug.")
        } else {
            ServerFnError::new(msg)
        }
    })?;

    // Generate Connect JWT token and store the JTI for revocation
    let (token, jti) = generate_connect_token(
        ac.ctx.connect_token.as_ref(),
        &ds.id,
        &ac.ws_id,
        ds.datasource_type.as_ref(),
    )?;

    kyomi_auth::datasource_service::update_connect_jti(ac.db(), &ds.id, &jti)
        .await
        .into_sfn()?;

    tracing::info!(
        "Created Connect datasource '{}' (slug: {}, id: {}) for workspace {} by user {}",
        ds.name,
        ds.slug,
        ds.id,
        ac.ws_id,
        ac.auth.user_id
    );

    Ok(CreateConnectResult {
        datasource_id: ds.id,
        connect_token: token,
    })
}

/// Rotate (regenerate) the Connect token for an existing datasource.
///
/// Mirrors `POST /api/v1/datasources/{id}/connect/rotate-token`.
/// The old token is immediately invalidated (JTI replaced).
#[server(prefix = "/leptos-api")]
pub async fn rotate_connect_token(datasource_id: String) -> Result<String, ServerFnError> {
    let ac = AuthenticatedContext::extract().await?;

    require_workspace_admin(&ac.auth)?;

    let ds = kyomi_auth::datasource_service::get_datasource(ac.db(), &datasource_id, &ac.ws_id)
        .await
        .into_sfn()?
        .ok_or_else(|| ServerFnError::new("Datasource not found"))?;

    if ds.connection_type != "connect" {
        return Err(ServerFnError::new(
            "Token rotation is only available for Connect datasources",
        ));
    }

    let (token, jti) = generate_connect_token(
        ac.ctx.connect_token.as_ref(),
        &ds.id,
        &ac.ws_id,
        ds.datasource_type.as_ref(),
    )?;

    kyomi_auth::datasource_service::update_connect_jti(ac.db(), &ds.id, &jti)
        .await
        .into_sfn()?;

    tracing::info!(
        "Rotated Connect token for datasource '{}' (id: {}) by user {}",
        ds.slug,
        ds.id,
        ac.auth.user_id
    );

    Ok(token)
}

/// Check whether the Kyomi Connect agent is currently connected for a
/// datasource.
///
/// Mirrors `GET /api/v1/datasources/{identifier}/connect/status`. When the
/// server is running without Redis (single-instance mode), the agent is always
/// reported as disconnected — matching the REST handler's behavior.
#[server(prefix = "/leptos-api")]
pub async fn connect_status(datasource_id: String) -> Result<ConnectStatusResponse, ServerFnError> {
    let ac = AuthenticatedContext::extract().await?;

    require_workspace_admin(&ac.auth)?;

    let ds = kyomi_auth::datasource_service::get_datasource(ac.db(), &datasource_id, &ac.ws_id)
        .await
        .into_sfn()?
        .ok_or_else(|| ServerFnError::new("Datasource not found"))?;

    if ds.connection_type != "connect" {
        return Err(ServerFnError::new(
            "Connect status is only available for Connect datasources",
        ));
    }

    // Single-instance mode (no Redis) — always report disconnected.
    let Some(mut redis) = ac.ctx.redis.clone() else {
        return Ok(ConnectStatusResponse {
            connected: false,
            last_seen: None,
        });
    };

    let presence = kyomi_auth::connect_token::check_presence(&mut redis, &ds.id)
        .await
        .into_sfn()?;

    Ok(ConnectStatusResponse {
        connected: presence.connected,
        last_seen: presence.last_seen.map(|ts| ts.to_rfc3339()),
    })
}

/// Disconnect (revoke) the Kyomi Connect token for a datasource.
///
/// Mirrors `POST /api/v1/datasources/{identifier}/connect/disconnect`.
/// Clears the stored JTI so any active token immediately fails verification.
#[server(prefix = "/leptos-api")]
pub async fn disconnect_connect_datasource(datasource_id: String) -> Result<(), ServerFnError> {
    let ac = AuthenticatedContext::extract().await?;

    require_workspace_admin(&ac.auth)?;

    let ds = kyomi_auth::datasource_service::get_datasource(ac.db(), &datasource_id, &ac.ws_id)
        .await
        .into_sfn()?
        .ok_or_else(|| ServerFnError::new("Datasource not found"))?;

    if ds.connection_type != "connect" {
        return Err(ServerFnError::new(
            "Disconnect is only available for Connect datasources",
        ));
    }

    kyomi_auth::datasource_service::clear_connect_jti(ac.db(), &ds.id)
        .await
        .into_sfn()?;

    tracing::info!(
        "Disconnected Connect datasource '{}' (id: {}) by user {}",
        ds.slug,
        ds.id,
        ac.auth.user_id
    );

    Ok(())
}

// ─── Helpers (server-only) ──────────────────────────────────────────────────

/// Reject non-workspace-admin users.
#[cfg(feature = "ssr")]
fn require_workspace_admin(auth: &kyomi_auth::middleware::AuthUser) -> Result<(), ServerFnError> {
    if !auth
        .workspace
        .workspace_roles
        .contains(&kyomi_core::enums::WorkspaceRole::WorkspaceAdmin)
    {
        return Err(ServerFnError::new("Workspace admin access required"));
    }
    Ok(())
}

/// Mint a Connect JWT for a datasource using the server's `ConnectTokenService`.
///
/// Returns `(token, jti)` on success. This helper centralizes the `Option`
/// handling for `ServerContext.connect_token`: when the service is absent
/// (single-instance/no-Connect deployments), callers get a user-facing
/// `"Kyomi Connect is not configured on this server"` error instead of a
/// misleading 500.
///
/// The `Option<&Arc<...>>` signature is intentional — it lets callers pass
/// `ctx.connect_token.as_ref()` without cloning the `Arc`, and it makes the
/// Some/None contract explicit at every call site.
#[cfg(feature = "ssr")]
fn generate_connect_token(
    connect_token: Option<&std::sync::Arc<kyomi_auth::connect_token::ConnectTokenService>>,
    datasource_id: &str,
    workspace_id: &str,
    datasource_type: &str,
) -> Result<(String, String), ServerFnError> {
    let service = connect_token
        .ok_or_else(|| ServerFnError::new("Kyomi Connect is not configured on this server"))?;
    service
        .generate(datasource_id, workspace_id, datasource_type)
        .into_sfn()
}

#[cfg(all(test, feature = "ssr"))]
mod tests {
    //! Unit tests for the `ServerContext.connect_token` → `generate_connect_token`
    //! plumbing. Regression coverage for KYO-115: the rotate/create Connect
    //! flows were broken because they looked up `Arc<ConnectTokenService>` via
    //! `leptos::use_context::<Arc<ConnectTokenService>>()`, which the server
    //! never provides as a standalone context — the service lives on
    //! `ServerContext.connect_token` instead.

    use super::generate_connect_token;
    use kyomi_auth::connect_token::ConnectTokenService;
    use std::sync::Arc;

    /// Generate a fresh P-256 PKCS#8 PEM key for tests — mirrors the
    /// `generate_test_key_pem` helper in `kyomi-auth::connect_token::tests`.
    /// We need our own copy here because that helper isn't exported.
    fn test_private_key_pem() -> String {
        use p256::elliptic_curve::rand_core::OsRng;
        use p256::pkcs8::{EncodePrivateKey, LineEnding};

        let secret_key = p256::SecretKey::random(&mut OsRng);
        secret_key
            .to_pkcs8_pem(LineEnding::LF)
            .expect("encode test key as PKCS#8 PEM")
            .to_string()
    }

    fn test_service() -> Arc<ConnectTokenService> {
        Arc::new(
            ConnectTokenService::new(&test_private_key_pem(), "wss://connect.test/v1")
                .expect("construct test ConnectTokenService"),
        )
    }

    #[test]
    fn none_returns_not_configured_error() {
        // This is the exact bug from KYO-115: the production server never
        // provided `Arc<ConnectTokenService>` as a standalone Leptos context,
        // so the lookup always returned `None` and every rotate/create call
        // short-circuited with this error. The fix is to pull from
        // `ServerContext.connect_token`, which the server *does* provide —
        // but the `None` contract still matters for deployments that don't
        // configure Kyomi Connect.
        let err = generate_connect_token(None, "ds-1", "ws-1", "postgres")
            .expect_err("None connect_token must yield an error");
        let msg = err.to_string();
        assert!(
            msg.contains("Kyomi Connect is not configured on this server"),
            "expected not-configured error, got: {msg}"
        );
    }

    #[test]
    fn some_generates_a_valid_token() {
        let service = test_service();
        let (token, jti) = generate_connect_token(
            Some(&service),
            "ds-abc",
            "ws-xyz",
            "postgres",
        )
        .expect("Some(service) must mint a token");

        // Shape checks: non-empty JWT with 3 dot-separated parts, 22-char jti.
        assert!(!token.is_empty(), "token must not be empty");
        assert_eq!(token.matches('.').count(), 2, "JWT must have 3 parts");
        assert_eq!(jti.len(), 22, "jti must be 22 base64url chars (16 bytes)");

        // The minted token must verify under the same service — proving we
        // actually reached the generate path, not a stubbed/faked success.
        let claims = service.verify(&token).expect("minted token must verify");
        assert_eq!(claims.jti, jti, "returned jti must match claim");
        assert_eq!(claims.dsid, "ds-abc", "dsid must match input");
        assert_eq!(claims.wid, "ws-xyz", "wid must match input");
        assert_eq!(claims.db, "postgres", "db must match input");
    }

    #[test]
    fn some_rotation_mints_distinct_jtis() {
        // Rotating (generating twice for the same datasource) must produce
        // different jtis — this is what makes the old token immediately
        // unusable after a rotate.
        let service = test_service();
        let (_t1, jti1) =
            generate_connect_token(Some(&service), "ds-rot", "ws-rot", "mysql").unwrap();
        let (_t2, jti2) =
            generate_connect_token(Some(&service), "ds-rot", "ws-rot", "mysql").unwrap();
        assert_ne!(jti1, jti2, "rotated token must have a fresh jti");
    }
}
