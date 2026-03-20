// SPDX-License-Identifier: AGPL-3.0-or-later

//! Knowledge files REST endpoints.
//!
//! Manages the markdown file tree for workspace knowledge:
//! - `GET    /{workspace_id}/knowledge-files`          — Full tree
//! - `POST   /{workspace_id}/knowledge-files`          — Create file/folder
//! - `GET    /{workspace_id}/knowledge-files/{id}`      — File with content
//! - `PATCH  /{workspace_id}/knowledge-files/{id}`      — Update content/name/parent
//! - `DELETE /{workspace_id}/knowledge-files/{id}`      — Delete (CASCADE children)
//! - `GET    /{workspace_id}/knowledge-files/search`    — Text search
//! - `POST   /{workspace_id}/knowledge-files/migrate`   — Migrate agent_learnings (admin)

use axum::{
    extract::{Path, Query, State},
    routing::{get, post},
    Json, Router,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use kyomi_auth::middleware::AuthUser;
use kyomi_knowledge::{knowledge_files, migration};

use crate::state::AppState;

// ===========================================================================
// Router
// ===========================================================================

/// Build the `/workspaces/{workspace_id}/knowledge-files` router.
pub fn routes() -> Router<AppState> {
    Router::new()
        .route(
            "/{workspace_id}/knowledge-files",
            get(list_tree).post(create_file),
        )
        .route(
            "/{workspace_id}/knowledge-files/search",
            get(search_files),
        )
        .route(
            "/{workspace_id}/knowledge-files/migrate",
            post(migrate_learnings),
        )
        .route(
            "/{workspace_id}/knowledge-files/{file_id}",
            get(get_file)
                .patch(update_file)
                .delete(delete_file),
        )
}

// ===========================================================================
// Helpers
// ===========================================================================

/// Verify workspace access: user must belong to the requested workspace.
// TODO: extract to shared module (duplicated in learnings.rs)
fn verify_workspace_access(
    user: &AuthUser,
    workspace_id: &str,
) -> Result<(), kyomi_core::Error> {
    let user_workspace = user
        .workspace
        .workspace_id
        .as_deref()
        .ok_or_else(|| kyomi_core::Error::BadRequest("Workspace context required".into()))?;

    if user_workspace != workspace_id {
        return Err(kyomi_core::Error::Forbidden("Access denied".into()));
    }
    Ok(())
}

/// Reject non-workspace-admin users with 403.
fn require_workspace_admin(user: &AuthUser) -> Result<(), kyomi_core::Error> {
    if !user
        .workspace
        .workspace_roles
        .iter()
        .any(|r| *r == kyomi_core::WorkspaceRole::WorkspaceAdmin)
    {
        return Err(kyomi_core::Error::Forbidden(
            "Only workspace admins can perform this action".into(),
        ));
    }
    Ok(())
}

/// Resolve a user ID to a display name (name or email).
async fn resolve_display_name(db: &kyomi_core::db::DbPool, user_id: Option<&str>) -> Option<String> {
    let uid = user_id?;
    let user = kyomi_auth::user_service::get_user_by_id(db, uid).await.ok()??;
    Some(user.name.filter(|n| !n.is_empty()).unwrap_or(user.email))
}

/// Map a service-layer error to the appropriate HTTP error.
///
/// The knowledge_files service uses `anyhow::bail!("File not found")` and
/// `anyhow::bail!("Conflict: ...")` for domain errors. This helper inspects
/// the error message to return 404 / 409 instead of a blanket 500.
fn map_service_error(context: &str, e: impl std::fmt::Display) -> kyomi_core::Error {
    let msg = e.to_string();
    if msg.contains("File not found") {
        kyomi_core::Error::NotFound("Knowledge file not found".into())
    } else if msg.contains("Conflict") {
        kyomi_core::Error::Conflict(msg)
    } else {
        kyomi_core::Error::Internal(format!("{context}: {msg}"))
    }
}

// ===========================================================================
// Request / Response Types
// ===========================================================================

#[derive(Deserialize)]
struct CreateFileRequest {
    name: String,
    #[serde(default)]
    parent_id: Option<String>,
    #[serde(default)]
    content: Option<String>,
    #[serde(default)]
    is_folder: bool,
}

#[derive(Deserialize)]
struct UpdateFileRequest {
    #[serde(default)]
    content: Option<String>,
    #[serde(default)]
    content_hash: Option<String>,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    parent_id: Option<String>,
    #[serde(default)]
    sort_order: Option<i32>,
}

#[derive(Deserialize)]
struct SearchParams {
    q: String,
}

#[derive(Serialize)]
struct FileResponse {
    id: String,
    parent_id: Option<String>,
    name: String,
    is_folder: bool,
    content: Option<String>,
    content_hash: Option<String>,
    sort_order: i32,
    created_by: Option<String>,
    updated_by: Option<String>,
    created_at: String,
    updated_at: String,
}

impl From<knowledge_files::KnowledgeFile> for FileResponse {
    fn from(f: knowledge_files::KnowledgeFile) -> Self {
        Self {
            id: f.id,
            parent_id: f.parent_id,
            name: f.name,
            is_folder: f.is_folder,
            content: f.content,
            content_hash: f.content_hash,
            sort_order: f.sort_order,
            created_by: f.created_by,
            updated_by: f.updated_by,
            created_at: f.created_at.to_rfc3339(),
            updated_at: f.updated_at.to_rfc3339(),
        }
    }
}

// ===========================================================================
// Endpoint Handlers
// ===========================================================================

// ---------------------------------------------------------------------------
// GET /{workspace_id}/knowledge-files — Full tree
// ---------------------------------------------------------------------------

async fn list_tree(
    State(state): State<AppState>,
    user: AuthUser,
    Path(workspace_id): Path<String>,
) -> Result<Json<Vec<knowledge_files::KnowledgeFileTreeEntry>>, kyomi_core::Error> {
    verify_workspace_access(&user, &workspace_id)?;

    let entries = knowledge_files::list_tree(&state.db, &workspace_id)
        .await
        .map_err(|e| kyomi_core::Error::Internal(format!("Failed to list tree: {e}")))?;

    Ok(Json(entries))
}

// ---------------------------------------------------------------------------
// POST /{workspace_id}/knowledge-files — Create file/folder
// ---------------------------------------------------------------------------

async fn create_file(
    State(state): State<AppState>,
    user: AuthUser,
    Path(workspace_id): Path<String>,
    Json(request): Json<CreateFileRequest>,
) -> Result<Json<FileResponse>, kyomi_core::Error> {
    verify_workspace_access(&user, &workspace_id)?;

    if request.name.trim().is_empty() {
        return Err(kyomi_core::Error::BadRequest("Name cannot be empty".into()));
    }

    let embed = state.embedding.wait_ready().await?;

    let file = knowledge_files::create_file(
        &state.db,
        embed,
        &workspace_id,
        request.parent_id.as_deref(),
        &request.name,
        request.content.as_deref(),
        request.is_folder,
        &user.user_id,
    )
    .await
    .map_err(|e| {
        if e.to_string().contains("unique") || e.to_string().contains("duplicate") {
            kyomi_core::Error::BadRequest(format!(
                "A file or folder named '{}' already exists in this location",
                request.name
            ))
        } else {
            kyomi_core::Error::Internal(format!("Failed to create file: {e}"))
        }
    })?;

    Ok(Json(file.into()))
}

// ---------------------------------------------------------------------------
// GET /{workspace_id}/knowledge-files/{id} — File with content
// ---------------------------------------------------------------------------

async fn get_file(
    State(state): State<AppState>,
    user: AuthUser,
    Path((workspace_id, file_id)): Path<(String, String)>,
) -> Result<Json<FileResponse>, kyomi_core::Error> {
    verify_workspace_access(&user, &workspace_id)?;

    let file = knowledge_files::get_file(&state.db, &workspace_id, &file_id)
        .await
        .map_err(|e| kyomi_core::Error::Internal(format!("Failed to get file: {e}")))?
        .ok_or_else(|| kyomi_core::Error::NotFound("Knowledge file not found".into()))?;

    let mut resp = FileResponse::from(file);
    resp.updated_by = resolve_display_name(&state.db, resp.updated_by.as_deref()).await.or(resp.updated_by);
    Ok(Json(resp))
}

// ---------------------------------------------------------------------------
// PATCH /{workspace_id}/knowledge-files/{id} — Update
// ---------------------------------------------------------------------------

async fn update_file(
    State(state): State<AppState>,
    user: AuthUser,
    Path((workspace_id, file_id)): Path<(String, String)>,
    Json(request): Json<UpdateFileRequest>,
) -> Result<Json<Value>, kyomi_core::Error> {
    verify_workspace_access(&user, &workspace_id)?;

    let has_updates = request.content.is_some()
        || request.name.is_some()
        || request.parent_id.is_some()
        || request.sort_order.is_some();

    if !has_updates {
        return Err(kyomi_core::Error::BadRequest("No fields to update".into()));
    }

    // Handle content update (with optional CAS via content_hash)
    if let Some(ref content) = request.content {
        let embed = state.embedding.wait_ready().await?;

        let result = knowledge_files::update_file_content(
            &state.db,
            embed,
            &workspace_id,
            &file_id,
            content,
            &user.user_id,
            request.content_hash.as_deref(),
        )
        .await
        .map_err(|e| map_service_error("Failed to update content", e))?;

        if result.is_none() {
            // CAS failed — content was modified concurrently
            return Err(kyomi_core::Error::Conflict(
                "Content was modified by another user. Reload and try again.".into(),
            ));
        }

        // Content update is the primary mutation — return the updated file
        // so the client gets the new content_hash for future CAS operations.
        let file = knowledge_files::get_file(&state.db, &workspace_id, &file_id)
            .await
            .map_err(|e| map_service_error("Failed to get file", e))?
            .ok_or_else(|| kyomi_core::Error::NotFound("Knowledge file not found".into()))?;

        let mut resp = FileResponse::from(file);
        resp.updated_by = resolve_display_name(&state.db, resp.updated_by.as_deref()).await.or(resp.updated_by);
        return Ok(Json(serde_json::to_value(resp).unwrap()));
    }

    // Handle rename
    if let Some(ref new_name) = request.name {
        if new_name.trim().is_empty() {
            return Err(kyomi_core::Error::BadRequest("Name cannot be empty".into()));
        }
        knowledge_files::rename_file(&state.db, &workspace_id, &file_id, new_name, &user.user_id)
            .await
            .map_err(|e| {
                let msg = e.to_string();
                if msg.contains("unique") || msg.contains("duplicate") {
                    kyomi_core::Error::BadRequest(format!(
                        "A file or folder named '{new_name}' already exists in this location"
                    ))
                } else {
                    map_service_error("Failed to rename", e)
                }
            })?;
    }

    // Handle move / reorder
    if request.parent_id.is_some() {
        // parent_id is explicitly provided — move to new parent (possibly with sort_order)
        knowledge_files::move_file(
            &state.db,
            &workspace_id,
            &file_id,
            request.parent_id.as_deref(),
            request.sort_order,
            &user.user_id,
        )
        .await
        .map_err(|e| map_service_error("Failed to move", e))?;
    } else if let Some(order) = request.sort_order {
        // sort_order only — reorder within current parent (do NOT change parent_id)
        knowledge_files::update_sort_order(
            &state.db,
            &workspace_id,
            &file_id,
            order,
            &user.user_id,
        )
        .await
        .map_err(|e| map_service_error("Failed to reorder", e))?;
    }

    Ok(Json(json!({"success": true})))
}

// ---------------------------------------------------------------------------
// DELETE /{workspace_id}/knowledge-files/{id} — Delete
// ---------------------------------------------------------------------------

async fn delete_file(
    State(state): State<AppState>,
    user: AuthUser,
    Path((workspace_id, file_id)): Path<(String, String)>,
) -> Result<Json<Value>, kyomi_core::Error> {
    verify_workspace_access(&user, &workspace_id)?;

    knowledge_files::delete_file(&state.db, &workspace_id, &file_id)
        .await
        .map_err(|e| map_service_error("Failed to delete", e))?;

    Ok(Json(json!({"success": true})))
}

// ---------------------------------------------------------------------------
// POST /{workspace_id}/knowledge-files/migrate — Migrate agent_learnings
// ---------------------------------------------------------------------------

async fn migrate_learnings(
    State(state): State<AppState>,
    user: AuthUser,
    Path(workspace_id): Path<String>,
) -> Result<Json<Value>, kyomi_core::Error> {
    verify_workspace_access(&user, &workspace_id)?;
    require_workspace_admin(&user)?;

    let embed = state.embedding.wait_ready().await?;

    let result = migration::migrate_learnings_to_knowledge_files(
        &state.db,
        embed,
        &workspace_id,
        &user.user_id,
    )
    .await
    .map_err(|e| kyomi_core::Error::Internal(format!("Migration failed: {e}")))?;

    Ok(Json(serde_json::to_value(result).map_err(|e| {
        kyomi_core::Error::Internal(format!("Failed to serialize migration result: {e}"))
    })?))
}

// ---------------------------------------------------------------------------
// GET /{workspace_id}/knowledge-files/search — Text search
// ---------------------------------------------------------------------------

async fn search_files(
    State(state): State<AppState>,
    user: AuthUser,
    Path(workspace_id): Path<String>,
    Query(params): Query<SearchParams>,
) -> Result<Json<Vec<knowledge_files::KnowledgeFileSearchResult>>, kyomi_core::Error> {
    verify_workspace_access(&user, &workspace_id)?;

    let query = params.q.trim();
    if query.is_empty() {
        return Err(kyomi_core::Error::BadRequest("Search query cannot be empty".into()));
    }

    let results = knowledge_files::search_files(&state.db, &workspace_id, query)
        .await
        .map_err(|e| kyomi_core::Error::Internal(format!("Search failed: {e}")))?;

    Ok(Json(results))
}

// ===========================================================================
// Tests
// ===========================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn create_file_request_deserializes_minimal() {
        let json = json!({"name": "Metrics.md"});
        let req: CreateFileRequest = serde_json::from_value(json).unwrap();
        assert_eq!(req.name, "Metrics.md");
        assert!(req.parent_id.is_none());
        assert!(req.content.is_none());
        assert!(!req.is_folder);
    }

    #[test]
    fn create_file_request_deserializes_full() {
        let json = json!({
            "name": "Revenue",
            "parent_id": "550e8400-e29b-41d4-a716-446655440000",
            "content": "# Revenue\nTracking revenue metrics.",
            "is_folder": false
        });
        let req: CreateFileRequest = serde_json::from_value(json).unwrap();
        assert_eq!(req.name, "Revenue");
        assert!(req.parent_id.is_some());
        assert!(req.content.is_some());
        assert!(!req.is_folder);
    }

    #[test]
    fn create_folder_request() {
        let json = json!({"name": "Data Navigation", "is_folder": true});
        let req: CreateFileRequest = serde_json::from_value(json).unwrap();
        assert_eq!(req.name, "Data Navigation");
        assert!(req.is_folder);
    }

    #[test]
    fn update_file_request_content_only() {
        let json = json!({
            "content": "Updated content",
            "content_hash": "abc123def456"
        });
        let req: UpdateFileRequest = serde_json::from_value(json).unwrap();
        assert_eq!(req.content.as_deref(), Some("Updated content"));
        assert_eq!(req.content_hash.as_deref(), Some("abc123def456"));
        assert!(req.name.is_none());
    }

    #[test]
    fn update_file_request_rename_only() {
        let json = json!({"name": "New Name.md"});
        let req: UpdateFileRequest = serde_json::from_value(json).unwrap();
        assert_eq!(req.name.as_deref(), Some("New Name.md"));
        assert!(req.content.is_none());
    }

    #[test]
    fn update_file_request_move() {
        let json = json!({
            "parent_id": "550e8400-e29b-41d4-a716-446655440000",
            "sort_order": 5
        });
        let req: UpdateFileRequest = serde_json::from_value(json).unwrap();
        assert!(req.parent_id.is_some());
        assert_eq!(req.sort_order, Some(5));
    }

    #[test]
    fn update_file_request_empty() {
        let json = json!({});
        let req: UpdateFileRequest = serde_json::from_value(json).unwrap();
        assert!(req.content.is_none());
        assert!(req.name.is_none());
        assert!(req.parent_id.is_none());
        assert!(req.sort_order.is_none());
    }

    #[test]
    fn file_response_from_knowledge_file() {
        let file = knowledge_files::KnowledgeFile {
            id: "file-1".into(),
            workspace_id: "ws-1".into(),
            parent_id: None,
            name: "Test.md".into(),
            is_folder: false,
            content: Some("Hello".into()),
            content_hash: Some("abc123".into()),
            sort_order: 0,
            created_by: Some("user-1".into()),
            updated_by: Some("user-1".into()),
            created_at: chrono::Utc::now(),
            updated_at: chrono::Utc::now(),
        };

        let response: FileResponse = file.into();
        assert_eq!(response.id, "file-1");
        assert_eq!(response.name, "Test.md");
        assert!(!response.is_folder);
        assert_eq!(response.content.as_deref(), Some("Hello"));
    }

    #[test]
    fn search_params_deserializes() {
        let json = json!({"q": "revenue"});
        let params: SearchParams = serde_json::from_value(json).unwrap();
        assert_eq!(params.q, "revenue");
    }
}
