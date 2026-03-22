// SPDX-License-Identifier: AGPL-3.0-or-later

//! Server functions for knowledge file CRUD, search, and tree listing.
//!
//! These replace the REST API calls for managing the workspace knowledge base.
//! Each function calls the same service-layer code as the existing REST routes
//! in `apps/server/src/routes/knowledge_files.rs`.

use leptos::prelude::*;

use crate::types::{KnowledgeFileDetail, KnowledgeSearchResult, KnowledgeTreeEntry};

// ─────────────────────────────────────────────────────────────────────────────
// Helpers (server-only)
// ─────────────────────────────────────────────────────────────────────────────

/// Convert a `KnowledgeFileTreeEntry` (service layer) to a `KnowledgeTreeEntry` (UI).
#[cfg(feature = "ssr")]
fn tree_entry_to_item(
    entry: &kyomi_knowledge::knowledge_files::KnowledgeFileTreeEntry,
) -> KnowledgeTreeEntry {
    KnowledgeTreeEntry {
        id: entry.id.clone(),
        parent_id: entry.parent_id.clone(),
        name: entry.name.clone(),
        is_folder: entry.is_folder,
        sort_order: entry.sort_order,
        updated_at: entry.updated_at.to_rfc3339(),
        updated_by: entry.updated_by.clone(),
    }
}

/// Convert a `KnowledgeFile` (service layer) to a `KnowledgeFileDetail` (UI).
#[cfg(feature = "ssr")]
fn file_to_detail(
    file: &kyomi_knowledge::knowledge_files::KnowledgeFile,
) -> KnowledgeFileDetail {
    KnowledgeFileDetail {
        id: file.id.clone(),
        parent_id: file.parent_id.clone(),
        name: file.name.clone(),
        is_folder: file.is_folder,
        content: file.content.clone(),
        content_hash: file.content_hash.clone(),
        sort_order: file.sort_order,
        created_by: file.created_by.clone(),
        updated_by: file.updated_by.clone(),
        created_at: file.created_at.to_rfc3339(),
        updated_at: file.updated_at.to_rfc3339(),
    }
}

/// Resolve a user ID to a display name (name or email).
///
/// Mirrors `resolve_display_name` in `apps/server/src/routes/knowledge_files.rs`.
#[cfg(feature = "ssr")]
async fn resolve_display_name(
    db: &kyomi_core::db::DbPool,
    user_id: Option<&str>,
) -> Option<String> {
    let uid = user_id?;
    let user = kyomi_auth::user_service::get_user_by_id(db, uid)
        .await
        .ok()??;
    Some(user.name.filter(|n| !n.is_empty()).unwrap_or(user.email))
}

/// Map a knowledge service error to a `ServerFnError`, distinguishing 404.
///
/// Note: CAS conflict errors from `update_file_content` are handled directly in
/// `update_knowledge_file` (the function returns `None` rather than an error).
/// This helper covers rename, move, reorder, and delete error paths.
#[cfg(feature = "ssr")]
fn map_service_error(context: &str, e: impl std::fmt::Display) -> ServerFnError {
    let msg = e.to_string();
    if msg.contains("File not found") {
        ServerFnError::new("Knowledge file not found")
    } else {
        ServerFnError::new(format!("{context}: {msg}"))
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Knowledge file CRUD
// ─────────────────────────────────────────────────────────────────────────────

/// List the full knowledge file tree for the current workspace.
///
/// Mirrors `GET /{workspace_id}/knowledge-files` in
/// `apps/server/src/routes/knowledge_files.rs`.
#[server(prefix = "/leptos-api")]
pub async fn list_knowledge_tree() -> Result<Vec<KnowledgeTreeEntry>, ServerFnError> {
    let auth = extract_auth().await?;
    let ctx = extract_context()?;
    let ws_id = workspace_id(&auth)?;

    let entries = kyomi_knowledge::knowledge_files::list_tree(&ctx.db, ws_id)
        .await
        .map_err(|e| ServerFnError::new(format!("Failed to list tree: {e}")))?;

    Ok(entries.iter().map(tree_entry_to_item).collect())
}

/// Get a single knowledge file with content.
///
/// Mirrors `GET /{workspace_id}/knowledge-files/{id}` in
/// `apps/server/src/routes/knowledge_files.rs`.
#[server(prefix = "/leptos-api")]
pub async fn get_knowledge_file(file_id: String) -> Result<KnowledgeFileDetail, ServerFnError> {
    let auth = extract_auth().await?;
    let ctx = extract_context()?;
    let ws_id = workspace_id(&auth)?;

    let file = kyomi_knowledge::knowledge_files::get_file(&ctx.db, ws_id, &file_id)
        .await
        .map_err(|e| ServerFnError::new(format!("Failed to get file: {e}")))?
        .ok_or_else(|| ServerFnError::new("Knowledge file not found"))?;

    let mut detail = file_to_detail(&file);
    detail.updated_by = resolve_display_name(&ctx.db, detail.updated_by.as_deref())
        .await
        .or(detail.updated_by);

    Ok(detail)
}

/// Search knowledge files by text query.
///
/// Mirrors `GET /{workspace_id}/knowledge-files/search` in
/// `apps/server/src/routes/knowledge_files.rs`.
#[server(prefix = "/leptos-api")]
pub async fn search_knowledge_files(
    query: String,
) -> Result<Vec<KnowledgeSearchResult>, ServerFnError> {
    let auth = extract_auth().await?;
    let ctx = extract_context()?;
    let ws_id = workspace_id(&auth)?;

    let query = query.trim().to_string();
    if query.is_empty() {
        return Err(ServerFnError::new("Search query cannot be empty"));
    }

    let results = kyomi_knowledge::knowledge_files::search_files(&ctx.db, ws_id, &query)
        .await
        .map_err(|e| ServerFnError::new(format!("Search failed: {e}")))?;

    Ok(results
        .into_iter()
        .map(|r| KnowledgeSearchResult {
            id: r.id,
            parent_id: r.parent_id,
            name: r.name,
            is_folder: r.is_folder,
            content_preview: r.content_preview,
        })
        .collect())
}

/// Create a new knowledge file or folder.
///
/// Mirrors `POST /{workspace_id}/knowledge-files` in
/// `apps/server/src/routes/knowledge_files.rs`.
#[server(prefix = "/leptos-api")]
pub async fn create_knowledge_file(
    name: String,
    parent_id: Option<String>,
    content: Option<String>,
    is_folder: bool,
) -> Result<KnowledgeFileDetail, ServerFnError> {
    let auth = extract_auth().await?;
    let ctx = extract_context()?;
    let ws_id = workspace_id(&auth)?;

    if name.trim().is_empty() {
        return Err(ServerFnError::new("Name cannot be empty"));
    }

    let embed = ctx
        .embedding
        .wait_ready()
        .await
        .map_err(|e| ServerFnError::new(format!("Embedding service unavailable: {e}")))?;

    let file = kyomi_knowledge::knowledge_files::create_file(
        &ctx.db,
        embed,
        ws_id,
        parent_id.as_deref(),
        &name,
        content.as_deref(),
        is_folder,
        &auth.user_id,
    )
    .await
    .map_err(|e| {
        let msg = e.to_string();
        if msg.contains("unique") || msg.contains("duplicate") {
            ServerFnError::new(format!(
                "A file or folder named '{name}' already exists in this location"
            ))
        } else {
            ServerFnError::new(format!("Failed to create file: {msg}"))
        }
    })?;

    Ok(file_to_detail(&file))
}

/// Update a knowledge file (content, name, parent, or sort order).
///
/// Handles 4 cases exactly like the REST handler:
/// - content + content_hash: CAS update with conflict detection
/// - name: rename
/// - parent_id: move (optionally with sort_order)
/// - sort_order alone: reorder within current parent
///
/// Mirrors `PATCH /{workspace_id}/knowledge-files/{id}` in
/// `apps/server/src/routes/knowledge_files.rs`.
#[server(prefix = "/leptos-api")]
pub async fn update_knowledge_file(
    file_id: String,
    content: Option<String>,
    content_hash: Option<String>,
    name: Option<String>,
    parent_id: Option<String>,
    sort_order: Option<i32>,
) -> Result<KnowledgeFileDetail, ServerFnError> {
    let auth = extract_auth().await?;
    let ctx = extract_context()?;
    let ws_id = workspace_id(&auth)?;

    let has_updates = content.is_some()
        || name.is_some()
        || parent_id.is_some()
        || sort_order.is_some();

    if !has_updates {
        return Err(ServerFnError::new("No fields to update"));
    }

    // Handle content update (with optional CAS via content_hash)
    if let Some(ref content_val) = content {
        let embed = ctx
            .embedding
            .wait_ready()
            .await
            .map_err(|e| ServerFnError::new(format!("Embedding service unavailable: {e}")))?;

        let result = kyomi_knowledge::knowledge_files::update_file_content(
            &ctx.db,
            embed,
            ws_id,
            &file_id,
            content_val,
            &auth.user_id,
            content_hash.as_deref(),
        )
        .await
        .map_err(|e| map_service_error("Failed to update content", e))?;

        if result.is_none() {
            // CAS failed — content was modified concurrently.
            // The "CONFLICT:" prefix is a protocol marker checked by the file
            // editor component (components/knowledge/file_editor.rs) to
            // distinguish CAS failures from generic errors.
            return Err(ServerFnError::new(
                "CONFLICT: Content was modified by another user. Reload and try again.",
            ));
        }

        // Re-fetch to get the updated file with new content_hash
        let file = kyomi_knowledge::knowledge_files::get_file(&ctx.db, ws_id, &file_id)
            .await
            .map_err(|e| map_service_error("Failed to get file", e))?
            .ok_or_else(|| ServerFnError::new("Knowledge file not found"))?;

        let mut detail = file_to_detail(&file);
        detail.updated_by = resolve_display_name(&ctx.db, detail.updated_by.as_deref())
            .await
            .or(detail.updated_by);

        return Ok(detail);
    }

    // Handle rename and/or move/reorder.
    // These are NOT mutually exclusive — a single call can rename + move simultaneously,
    // matching the REST handler behavior in knowledge_files.rs.
    if let Some(ref new_name) = name {
        if new_name.trim().is_empty() {
            return Err(ServerFnError::new("Name cannot be empty"));
        }
        kyomi_knowledge::knowledge_files::rename_file(
            &ctx.db,
            ws_id,
            &file_id,
            new_name,
            &auth.user_id,
        )
        .await
        .map_err(|e| {
            let msg = e.to_string();
            if msg.contains("unique") || msg.contains("duplicate") {
                ServerFnError::new(format!(
                    "A file or folder named '{new_name}' already exists in this location"
                ))
            } else {
                map_service_error("Failed to rename", e)
            }
        })?;
    }

    // Handle move / reorder
    if parent_id.is_some() {
        // parent_id is explicitly provided — move to new parent (possibly with sort_order)
        kyomi_knowledge::knowledge_files::move_file(
            &ctx.db,
            ws_id,
            &file_id,
            parent_id.as_deref(),
            sort_order,
            &auth.user_id,
        )
        .await
        .map_err(|e| map_service_error("Failed to move", e))?;
    } else if let Some(order) = sort_order {
        // sort_order only — reorder within current parent (do NOT change parent_id)
        kyomi_knowledge::knowledge_files::update_sort_order(
            &ctx.db,
            ws_id,
            &file_id,
            order,
            &auth.user_id,
        )
        .await
        .map_err(|e| map_service_error("Failed to reorder", e))?;
    }

    // Re-fetch and return the updated file
    let file = kyomi_knowledge::knowledge_files::get_file(&ctx.db, ws_id, &file_id)
        .await
        .map_err(|e| map_service_error("Failed to get file", e))?
        .ok_or_else(|| ServerFnError::new("Knowledge file not found"))?;

    let mut detail = file_to_detail(&file);
    detail.updated_by = resolve_display_name(&ctx.db, detail.updated_by.as_deref())
        .await
        .or(detail.updated_by);

    Ok(detail)
}

/// Delete a knowledge file or folder (cascades to children).
///
/// Mirrors `DELETE /{workspace_id}/knowledge-files/{id}` in
/// `apps/server/src/routes/knowledge_files.rs`.
#[server(prefix = "/leptos-api")]
pub async fn delete_knowledge_file(file_id: String) -> Result<(), ServerFnError> {
    let auth = extract_auth().await?;
    let ctx = extract_context()?;
    let ws_id = workspace_id(&auth)?;

    kyomi_knowledge::knowledge_files::delete_file(&ctx.db, ws_id, &file_id)
        .await
        .map_err(|e| map_service_error("Failed to delete", e))?;

    Ok(())
}

// SSR-only import — placed at bottom to match `watches.rs` convention.
#[cfg(feature = "ssr")]
use super::{extract_auth, extract_context, workspace_id};
