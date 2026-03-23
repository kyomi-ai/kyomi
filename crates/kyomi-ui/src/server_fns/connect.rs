// SPDX-License-Identifier: AGPL-3.0-or-later

//! Server functions for the Connect Setup page.
//!
//! These replace the REST API calls that ConnectSetupPage.jsx makes:
//! - `GET /api/v1/datasources` (filtered by `connection_type == "connect"`) → `list_connect_datasources()`
//! - `POST /api/v1/datasources` (with `connection_type: "connect"`) → `create_connect_datasource()`
//! - `POST /api/v1/datasources/{id}/connect/rotate-token` → `rotate_connect_token()`
//!
//! Calls the same service-layer code as `apps/server/src/routes/datasources.rs`.
//!
//! ## Server context requirement
//!
//! The `create_connect_datasource` and `rotate_connect_token` functions require
//! `Arc<ConnectTokenService>` to be provided via `leptos::prelude::provide_context`
//! in the server's router setup (alongside `ServerContext`).

use leptos::prelude::*;
use serde::{Deserialize, Serialize};

#[cfg(feature = "ssr")]
use super::{extract_auth, extract_context, workspace_id};

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

// ─── Server Functions ───────────────────────────────────────────────────────

/// List datasources filtered to `connection_type == "connect"`.
///
/// Returns only the fields needed by the Connect Setup page's selection list.
/// Mirrors `GET /api/v1/datasources` with client-side filtering in the React page.
#[server(prefix = "/leptos-api")]
pub async fn list_connect_datasources() -> Result<Vec<ConnectDatasource>, ServerFnError> {
    let auth = extract_auth().await?;
    let ctx = extract_context()?;
    let ws_id = workspace_id(&auth)?;

    let datasources = kyomi_auth::datasource_service::list_datasources(&ctx.db, ws_id, false)
        .await
        .map_err(|e| ServerFnError::new(e.to_string()))?;

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
    let auth = extract_auth().await?;
    let ctx = extract_context()?;
    let ws_id = workspace_id(&auth)?;

    require_workspace_admin(&auth)?;

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

    // ConnectTokenService must be provided as context
    let connect_token_service = leptos::prelude::use_context::<
        std::sync::Arc<kyomi_auth::connect_token::ConnectTokenService>,
    >()
    .ok_or_else(|| ServerFnError::new("Kyomi Connect is not configured on this server"))?;

    let slug_ref = slug.as_deref().filter(|s| !s.is_empty());

    let ds = kyomi_auth::datasource_service::create_datasource(
        &ctx.db,
        ws_id,
        &name,
        slug_ref,
        &datasource_type,
        serde_json::json!({}),
        Some("connect"),
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
    let (token, jti) = connect_token_service
        .generate(&ds.id, ws_id, ds.datasource_type.as_ref())
        .map_err(|e| ServerFnError::new(e.to_string()))?;

    kyomi_auth::datasource_service::update_connect_jti(&ctx.db, &ds.id, &jti)
        .await
        .map_err(|e| ServerFnError::new(e.to_string()))?;

    tracing::info!(
        "Created Connect datasource '{}' (slug: {}, id: {}) for workspace {} by user {}",
        ds.name,
        ds.slug,
        ds.id,
        ws_id,
        auth.user_id
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
    let auth = extract_auth().await?;
    let ctx = extract_context()?;
    let ws_id = workspace_id(&auth)?;

    require_workspace_admin(&auth)?;

    let ds = kyomi_auth::datasource_service::get_datasource(&ctx.db, &datasource_id, ws_id)
        .await
        .map_err(|e| ServerFnError::new(e.to_string()))?
        .ok_or_else(|| ServerFnError::new("Datasource not found"))?;

    if ds.connection_type != "connect" {
        return Err(ServerFnError::new(
            "Token rotation is only available for Connect datasources",
        ));
    }

    let connect_token_service = leptos::prelude::use_context::<
        std::sync::Arc<kyomi_auth::connect_token::ConnectTokenService>,
    >()
    .ok_or_else(|| ServerFnError::new("Kyomi Connect is not configured on this server"))?;

    let (token, jti) = connect_token_service
        .generate(&ds.id, ws_id, ds.datasource_type.as_ref())
        .map_err(|e| ServerFnError::new(e.to_string()))?;

    kyomi_auth::datasource_service::update_connect_jti(&ctx.db, &ds.id, &jti)
        .await
        .map_err(|e| ServerFnError::new(e.to_string()))?;

    tracing::info!(
        "Rotated Connect token for datasource '{}' (id: {}) by user {}",
        ds.slug,
        ds.id,
        auth.user_id
    );

    Ok(token)
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
