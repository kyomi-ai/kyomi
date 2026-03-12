// SPDX-License-Identifier: AGPL-3.0-or-later

//! User management endpoints.
//!
//! Wire-compatible with Python's `routers/users.py`.
//! Admin endpoints require the `admin` role in extra_metadata.
//! All responses use `{"detail": "message"}` format for errors.

use axum::{
    extract::{Path, State},
    routing::{get, patch, post, put},
    Json, Router,
};
use serde::Deserialize;

use kyomi_auth::{middleware::AuthUser, user_service};

use crate::state::AppState;

/// Build the `/users` router with all user management endpoints.
///
/// Axum resolves literal path segments (e.g. `/me`, `/tokens`) before
/// dynamic captures (`/{user_id}`), so there are no routing conflicts.
pub fn routes() -> Router<AppState> {
    Router::new()
        // Admin list/create
        .route("/", post(create_user).get(list_users))
        // Current user endpoints (/me/...)
        .route("/me", patch(update_me))
        .route("/me/chartml-config", get(get_chartml_config).put(update_chartml_config))
        .route("/me/knowledge", get(get_knowledge).put(update_knowledge))
        .route("/me/preferences", patch(update_preferences))
        .route("/me/tours", get(get_tours))
        .route("/me/tours/{tour_id}", post(mark_tour_complete))
        // Token endpoints
        .route("/tokens", post(create_token))
        .route("/tokens/{param}", get(list_tokens).delete(revoke_token))
        // Admin user CRUD — dynamic capture, matches after /me and /tokens
        .route("/{user_id}", put(update_user).delete(delete_user))
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Reject non-admin users with 403.
fn require_admin(user: &AuthUser) -> Result<(), kyomi_core::Error> {
    if !user.roles.contains(&"admin".to_string()) {
        return Err(kyomi_core::Error::Forbidden("Admin access required".into()));
    }
    Ok(())
}

/// Load a user from the database or return 404.
async fn fetch_user_or_404(
    db: &kyomi_core::DbPool,
    user_id: &str,
) -> Result<kyomi_core::models::User, kyomi_core::Error> {
    user_service::get_user_by_id(db, user_id)
        .await?
        .ok_or_else(|| kyomi_core::Error::NotFound("User not found".into()))
}

/// Build the standard UserResponse JSON from a User model.
fn user_response_json(user: &kyomi_core::models::User) -> serde_json::Value {
    serde_json::json!({
        "user_id": user.user_id,
        "email": user.email,
        "name": user.name,
        "roles": user.roles(),
        "active": user.active,
        "created_at": user.created_at,
        "last_login": user.last_login,
    })
}

// ---------------------------------------------------------------------------
// Request/Response types
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
struct CreateUserRequest {
    email: String,
    name: String,
    #[serde(default = "default_roles")]
    roles: Vec<String>,
}

fn default_roles() -> Vec<String> {
    vec!["user".to_string()]
}

#[derive(Deserialize)]
struct UpdateUserRequest {
    name: Option<String>,
    roles: Option<Vec<String>>,
    active: Option<bool>,
}

#[derive(Deserialize)]
struct UpdateMeRequest {
    name: Option<String>,
}

#[derive(Deserialize)]
struct ChartMLConfigRequest {
    config: Option<serde_json::Value>,
}

#[derive(Deserialize)]
struct UpdateKnowledgeRequest {
    knowledge: String,
}

#[derive(Deserialize)]
struct UserPreferencesRequest {
    query_history_retention_days: Option<i32>,
    theme: Option<String>,
    landing_page: Option<String>,
    default_dashboard_id: Option<serde_json::Value>, // String or null (to clear)
}

#[derive(Deserialize)]
struct CreateTokenRequest {
    user_email: String,
    token_name: String,
    #[serde(default = "default_expires_days")]
    expires_days: i32,
}

fn default_expires_days() -> i32 {
    30
}

// ---------------------------------------------------------------------------
// POST /users/ — Create user (admin)
// ---------------------------------------------------------------------------

async fn create_user(
    State(state): State<AppState>,
    user: AuthUser,
    Json(data): Json<CreateUserRequest>,
) -> Result<Json<serde_json::Value>, kyomi_core::Error> {
    require_admin(&user)?;

    // Validate roles
    let valid_roles: std::collections::HashSet<&str> = ["user", "admin"].into_iter().collect();
    if !data.roles.iter().all(|r| valid_roles.contains(r.as_str())) {
        return Err(kyomi_core::Error::BadRequest(
            format!("Invalid roles. Valid roles: {}", valid_roles.into_iter().collect::<Vec<_>>().join(", "))
        ));
    }

    // Check if user already exists
    if user_service::get_user_by_email(&state.db, &data.email).await?.is_some() {
        return Err(kyomi_core::Error::Conflict(
            format!("User with email '{}' already exists", data.email)
        ));
    }

    // Create user (admin-created users are verified, no password)
    let new_user = user_service::admin_create_user(
        &state.db,
        &data.email,
        &data.name,
        &data.roles,
    ).await?;

    tracing::info!("User created: {} by {}", data.email, user.email);

    Ok(Json(user_response_json(&new_user)))
}

// ---------------------------------------------------------------------------
// GET /users/ — List all users (admin)
// ---------------------------------------------------------------------------

async fn list_users(
    State(state): State<AppState>,
    user: AuthUser,
) -> Result<Json<serde_json::Value>, kyomi_core::Error> {
    require_admin(&user)?;

    let all_users = user_service::list_all_users(&state.db).await?;

    let users_json: Vec<serde_json::Value> = all_users
        .iter()
        .map(user_response_json)
        .collect();

    tracing::info!("Listed {} users for admin {}", users_json.len(), user.email);

    Ok(Json(serde_json::json!(users_json)))
}

// ---------------------------------------------------------------------------
// PUT /users/{user_id} — Update user (admin)
// ---------------------------------------------------------------------------

async fn update_user(
    State(state): State<AppState>,
    user: AuthUser,
    Path(user_id): Path<String>,
    Json(data): Json<UpdateUserRequest>,
) -> Result<Json<serde_json::Value>, kyomi_core::Error> {
    require_admin(&user)?;

    // Verify user exists
    let _ = fetch_user_or_404(&state.db, &user_id).await?;

    // Validate name if provided
    let trimmed_name = data.name.as_deref().map(str::trim);
    if let Some(name) = trimmed_name
        && name.is_empty()
    {
        return Err(kyomi_core::Error::BadRequest("Name cannot be empty".into()));
    }

    // Apply updates
    user_service::admin_update_user(
        &state.db,
        &user_id,
        trimmed_name,
        data.active,
        data.roles.as_deref(),
    ).await?;

    // Return updated user
    let updated = fetch_user_or_404(&state.db, &user_id).await?;

    tracing::info!("User updated: {} by {}", updated.email, user.email);

    Ok(Json(user_response_json(&updated)))
}

// ---------------------------------------------------------------------------
// DELETE /users/{user_id} — Delete user (admin)
// ---------------------------------------------------------------------------

async fn delete_user(
    State(state): State<AppState>,
    user: AuthUser,
    Path(user_id): Path<String>,
) -> Result<Json<serde_json::Value>, kyomi_core::Error> {
    require_admin(&user)?;

    let target = fetch_user_or_404(&state.db, &user_id).await?;

    // Prevent self-delete
    if target.email == user.email {
        return Err(kyomi_core::Error::BadRequest(
            "Cannot delete your own account".into(),
        ));
    }

    let success = user_service::delete_user(&state.db, &user_id).await?;
    if !success {
        return Err(kyomi_core::Error::Internal("Failed to delete user".into()));
    }

    tracing::info!("User deleted: {} by {}", target.email, user.email);

    Ok(Json(serde_json::json!({
        "message": format!("User {} has been deleted", target.email),
    })))
}

// ---------------------------------------------------------------------------
// PATCH /users/me — Update own name
// ---------------------------------------------------------------------------

async fn update_me(
    State(state): State<AppState>,
    user: AuthUser,
    Json(data): Json<UpdateMeRequest>,
) -> Result<Json<serde_json::Value>, kyomi_core::Error> {
    if let Some(ref name) = data.name {
        let trimmed = name.trim();
        if trimmed.is_empty() {
            return Err(kyomi_core::Error::BadRequest("Name cannot be empty".into()));
        }
        user_service::update_user_name(&state.db, &user.user_id, trimmed).await?;
    }

    let updated = fetch_user_or_404(&state.db, &user.user_id).await?;

    tracing::info!("User {} updated their profile", user.user_id);

    Ok(Json(serde_json::json!({
        "success": true,
        "message": "Profile updated successfully",
        "user": {
            "user_id": updated.user_id,
            "name": updated.name,
            "email": updated.email,
        }
    })))
}

// ---------------------------------------------------------------------------
// GET /users/me/chartml-config — Get user ChartML config
// ---------------------------------------------------------------------------

async fn get_chartml_config(
    State(state): State<AppState>,
    user: AuthUser,
) -> Result<Json<serde_json::Value>, kyomi_core::Error> {
    let db_user = fetch_user_or_404(&state.db, &user.user_id).await?;

    let config = db_user.chartml_config.unwrap_or(serde_json::json!({}));

    tracing::info!("ChartML config retrieved for user {}", user.email);

    Ok(Json(serde_json::json!({ "config": config })))
}

// ---------------------------------------------------------------------------
// PUT /users/me/chartml-config — Update user ChartML config
// ---------------------------------------------------------------------------

async fn update_chartml_config(
    State(state): State<AppState>,
    user: AuthUser,
    Json(data): Json<ChartMLConfigRequest>,
) -> Result<Json<serde_json::Value>, kyomi_core::Error> {
    let _ = fetch_user_or_404(&state.db, &user.user_id).await?;

    // Validate config is a JSON object if provided
    if let Some(ref config) = data.config
        && !config.is_object()
    {
        return Err(kyomi_core::Error::BadRequest(
            "ChartML config must be a valid JSON object".into(),
        ));
    }

    let config_value = data.config.unwrap_or(serde_json::json!(null));
    let success = user_service::update_chartml_config(
        &state.db,
        &user.user_id,
        &config_value,
    ).await?;

    if !success {
        return Err(kyomi_core::Error::NotFound("User not found".into()));
    }

    tracing::info!("ChartML config updated for user {}", user.email);

    Ok(Json(serde_json::json!({
        "message": "ChartML config updated successfully",
    })))
}

// ---------------------------------------------------------------------------
// GET /users/me/knowledge — Get user knowledge doc
// ---------------------------------------------------------------------------

async fn get_knowledge(
    State(state): State<AppState>,
    user: AuthUser,
) -> Result<Json<serde_json::Value>, kyomi_core::Error> {
    let db_user = fetch_user_or_404(&state.db, &user.user_id).await?;

    let knowledge = db_user.knowledge.unwrap_or_default();
    let updated_at = db_user.updated_at.to_rfc3339();

    tracing::info!("Knowledge retrieved for user {}", user.email);

    Ok(Json(serde_json::json!({
        "knowledge": knowledge,
        "updated_at": updated_at,
    })))
}

// ---------------------------------------------------------------------------
// PUT /users/me/knowledge — Update user knowledge doc
// ---------------------------------------------------------------------------

async fn update_knowledge(
    State(state): State<AppState>,
    user: AuthUser,
    Json(data): Json<UpdateKnowledgeRequest>,
) -> Result<Json<serde_json::Value>, kyomi_core::Error> {
    let _ = fetch_user_or_404(&state.db, &user.user_id).await?;

    user_service::update_knowledge(&state.db, &user.user_id, &data.knowledge).await?;

    // Re-fetch to get updated timestamp
    let updated = fetch_user_or_404(&state.db, &user.user_id).await?;

    tracing::info!("Knowledge updated for user {}", user.email);

    Ok(Json(serde_json::json!({
        "knowledge": data.knowledge,
        "updated_at": updated.updated_at.to_rfc3339(),
    })))
}

// ---------------------------------------------------------------------------
// PATCH /users/me/preferences — Update preferences (theme, retention)
// ---------------------------------------------------------------------------

async fn update_preferences(
    State(state): State<AppState>,
    user: AuthUser,
    Json(data): Json<UserPreferencesRequest>,
) -> Result<Json<serde_json::Value>, kyomi_core::Error> {
    let _ = fetch_user_or_404(&state.db, &user.user_id).await?;

    let mut metadata_updates = serde_json::Map::new();

    if let Some(days) = data.query_history_retention_days {
        metadata_updates.insert(
            "query_history_retention_days".into(),
            serde_json::json!(days),
        );
    }

    if let Some(ref theme) = data.theme {
        if !["light", "dark", "system"].contains(&theme.as_str()) {
            return Err(kyomi_core::Error::BadRequest(
                "Invalid theme. Must be 'light', 'dark', or 'system'.".into(),
            ));
        }
        metadata_updates.insert("theme".into(), serde_json::json!(theme));
    }

    if let Some(ref landing) = data.landing_page {
        if !["chat", "dashboards", "watches", "sql_editor"].contains(&landing.as_str()) {
            return Err(kyomi_core::Error::BadRequest(
                "Invalid landing_page. Must be 'chat', 'dashboards', 'watches', or 'sql_editor'.".into(),
            ));
        }
        metadata_updates.insert("landing_page".into(), serde_json::json!(landing));
    }

    if let Some(ref dashboard_id) = data.default_dashboard_id {
        match dashboard_id {
            serde_json::Value::Null => {
                metadata_updates.insert("default_dashboard_id".into(), serde_json::Value::Null);
            }
            serde_json::Value::String(id) if !id.is_empty() => {
                if uuid::Uuid::parse_str(id).is_err() {
                    return Err(kyomi_core::Error::BadRequest(
                        "default_dashboard_id must be a valid UUID.".into(),
                    ));
                }
                metadata_updates.insert("default_dashboard_id".into(), serde_json::json!(id));
            }
            _ => {
                return Err(kyomi_core::Error::BadRequest(
                    "default_dashboard_id must be a non-empty string or null.".into(),
                ));
            }
        }
    }

    let metadata_value = serde_json::Value::Object(metadata_updates);
    let success = user_service::update_extra_metadata(
        &state.db,
        &user.user_id,
        &metadata_value,
    ).await?;

    if !success {
        return Err(kyomi_core::Error::Internal(
            "Failed to update user metadata".into(),
        ));
    }

    tracing::info!("User preferences updated for {}", user.email);

    Ok(Json(serde_json::json!({
        "message": "Preferences updated successfully",
    })))
}

// ---------------------------------------------------------------------------
// GET /users/me/tours — Get tour completion status
// ---------------------------------------------------------------------------

async fn get_tours(
    State(state): State<AppState>,
    user: AuthUser,
) -> Result<Json<serde_json::Value>, kyomi_core::Error> {
    let db_user = fetch_user_or_404(&state.db, &user.user_id).await?;

    let extra_metadata = db_user.extra_metadata.unwrap_or(serde_json::json!({}));
    let tours_completed = extra_metadata
        .get("tours_completed")
        .cloned()
        .unwrap_or(serde_json::json!({}));

    Ok(Json(serde_json::json!({
        "tours_completed": tours_completed,
    })))
}

// ---------------------------------------------------------------------------
// POST /users/me/tours/{tour_id} — Mark tour complete
// ---------------------------------------------------------------------------

async fn mark_tour_complete(
    State(state): State<AppState>,
    user: AuthUser,
    Path(tour_id): Path<String>,
) -> Result<Json<serde_json::Value>, kyomi_core::Error> {
    let db_user = fetch_user_or_404(&state.db, &user.user_id).await?;

    // Get current tours_completed
    let extra_metadata = db_user.extra_metadata.unwrap_or(serde_json::json!({}));
    let mut tours_completed = extra_metadata
        .get("tours_completed")
        .and_then(|t| t.as_object().cloned())
        .unwrap_or_default();

    // Mark tour as completed
    tours_completed.insert(tour_id.clone(), serde_json::json!(true));

    // Update metadata with merged tours_completed
    let metadata = serde_json::json!({
        "tours_completed": tours_completed,
    });
    let success = user_service::update_extra_metadata(
        &state.db,
        &user.user_id,
        &metadata,
    ).await?;

    if !success {
        return Err(kyomi_core::Error::Internal(
            "Failed to update tour status".into(),
        ));
    }

    tracing::info!("Tour '{}' marked as completed for user {}", tour_id, user.email);

    Ok(Json(serde_json::json!({
        "message": format!("Tour '{}' marked as completed", tour_id),
    })))
}

// ---------------------------------------------------------------------------
// POST /users/tokens — Create API token (admin)
// ---------------------------------------------------------------------------

async fn create_token(
    State(state): State<AppState>,
    user: AuthUser,
    Json(data): Json<CreateTokenRequest>,
) -> Result<Json<serde_json::Value>, kyomi_core::Error> {
    require_admin(&user)?;

    // Find user by email
    let target_user = user_service::get_user_by_email(&state.db, &data.user_email)
        .await?
        .ok_or_else(|| {
            kyomi_core::Error::NotFound(
                format!("User with email '{}' not found", data.user_email),
            )
        })?;

    let (token_id, token_plaintext) = user_service::create_api_token(
        &state.db,
        &target_user.user_id,
        &data.token_name,
        Some(data.expires_days),
        &user.email,
    ).await?;

    // Calculate expiration for response
    let expires_at = chrono::Utc::now() + chrono::Duration::days(data.expires_days as i64);

    tracing::info!(
        "Token created: {} for {} by {}",
        data.token_name,
        data.user_email,
        user.email
    );

    Ok(Json(serde_json::json!({
        "token_id": token_id,
        "token": token_plaintext,
        "expires_at": expires_at,
    })))
}

// ---------------------------------------------------------------------------
// GET /users/tokens/by-email/{user_email} — List tokens for user (admin)
// ---------------------------------------------------------------------------

async fn list_tokens(
    State(state): State<AppState>,
    user: AuthUser,
    Path(user_email): Path<String>,
) -> Result<Json<serde_json::Value>, kyomi_core::Error> {
    require_admin(&user)?;

    // The path param is the user's email address
    let target_user = user_service::get_user_by_email(&state.db, &user_email)
        .await?
        .ok_or_else(|| {
            kyomi_core::Error::NotFound(
                format!("User with email '{}' not found", user_email),
            )
        })?;

    let tokens = user_service::get_user_api_tokens(&state.db, &target_user.user_id).await?;

    let tokens_json: Vec<serde_json::Value> = tokens
        .iter()
        .map(|t| {
            serde_json::json!({
                "token_id": t.token_id,
                "user_id": t.user_id,
                "name": t.name,
                "active": t.active,
                "created_at": t.created_at,
                "expires_at": t.expires_at,
                "last_used": t.last_used,
            })
        })
        .collect();

    tracing::info!("Listed {} tokens for user {}", tokens_json.len(), user_email);

    Ok(Json(serde_json::json!(tokens_json)))
}

// ---------------------------------------------------------------------------
// DELETE /users/tokens/{token_id} — Revoke token (admin)
// ---------------------------------------------------------------------------

async fn revoke_token(
    State(state): State<AppState>,
    user: AuthUser,
    Path(token_id): Path<String>,
) -> Result<Json<serde_json::Value>, kyomi_core::Error> {
    require_admin(&user)?;

    let success = user_service::revoke_api_token(&state.db, &token_id, &user.email).await?;
    if !success {
        return Err(kyomi_core::Error::NotFound(
            format!("Token with ID '{}' not found", token_id),
        ));
    }

    tracing::info!("Token revoked: {} by {}", token_id, user.email);

    Ok(Json(serde_json::json!({
        "message": format!("Token {} has been revoked", token_id),
    })))
}
