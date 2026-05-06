// SPDX-License-Identifier: AGPL-3.0-or-later

//! Integrations endpoint — lists registered messaging platforms with connection status.

use axum::{extract::State, routing::get, Json, Router};
use serde::Serialize;

use kyomi_auth::middleware::AuthUser;
use kyomi_core::platform;

use crate::state::AppState;

/// Build the `/integrations` router.
pub fn routes() -> Router<AppState> {
    Router::new().route("/", get(list_integrations))
}

#[derive(Serialize)]
struct IntegrationStatus {
    r#type: String,
    display_name: String,
    workspace_connected: bool,
    user_connected: bool,
}

/// GET /api/v1/integrations
///
/// Returns a JSON array of registered messaging platforms with their
/// connection status for the current workspace and user.
async fn list_integrations(
    State(state): State<AppState>,
    user: AuthUser,
) -> Result<Json<Vec<IntegrationStatus>>, kyomi_core::Error> {
    let workspace_id = user
        .workspace
        .workspace_id
        .as_deref()
        .ok_or_else(|| kyomi_core::Error::BadRequest("No workspace selected".into()))?;

    let mut result = Vec::new();

    for p in state.platforms.all() {
        let pt = p.platform_type();

        let workspace_connected =
            platform::get_workspace_integration(&state.db, workspace_id, pt)
                .await?
                .is_some();

        let user_connected = platform::get_user_integration(
            &state.db,
            workspace_id,
            &user.user_id,
            pt,
        )
        .await?
        .is_some();

        result.push(IntegrationStatus {
            r#type: pt.to_string(),
            display_name: p.display_name().to_string(),
            workspace_connected,
            user_connected,
        });
    }

    Ok(Json(result))
}
