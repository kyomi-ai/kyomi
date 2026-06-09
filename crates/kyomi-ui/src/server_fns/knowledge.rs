// SPDX-License-Identifier: AGPL-3.0-or-later

//! Server functions for knowledge document CRUD.
//!
//! Knowledge documents are stored in the same `dashboards` table as regular
//! dashboards, differentiated by `doc_type = 'knowledge'`. All functions
//! delegate to `dashboard_service` with the appropriate `DocType`.

use leptos::prelude::*;

use crate::server_fns::dashboards::DashboardListItem;

// ─────────────────────────────────────────────────────────────────────────────
// Knowledge document CRUD
// ─────────────────────────────────────────────────────────────────────────────

/// List or search knowledge documents in the current workspace.
///
/// Delegates to `dashboard_service::search_dashboards` with
/// `doc_type = Knowledge`.
#[server(prefix = "/leptos-api")]
pub async fn list_knowledge_docs(
    query: Option<String>,
    sort_by: Option<String>,
    limit: Option<i64>,
) -> Result<Vec<DashboardListItem>, ServerFnError> {
    let ac = AuthenticatedContext::extract().await?;

    let sort = match sort_by.as_deref() {
        Some("popularity") => kyomi_auth::dashboard_service::SearchSort::Popularity,
        Some("created") => kyomi_auth::dashboard_service::SearchSort::Created,
        _ => kyomi_auth::dashboard_service::SearchSort::Recent,
    };

    let limit = limit.unwrap_or(50).clamp(1, 100);

    let results = kyomi_auth::dashboard_service::search_dashboards(
        ac.db(),
        &ac.ws_id,
        &ac.auth.user_id,
        query.as_deref(),
        Some(kyomi_core::models::DocType::Knowledge),
        sort,
        limit,
    )
    .await
    .into_sfn()?;

    Ok(results
        .into_iter()
        .map(super::dashboards::map_search_result_to_list_item)
        .collect())
}

/// Create a new knowledge document. Returns the new document ID.
///
/// Delegates to `dashboard_service::create_dashboard` with
/// `doc_type = Knowledge`.
#[server(prefix = "/leptos-api")]
pub async fn create_knowledge_doc(
    title: String,
    content: Option<String>,
) -> Result<String, ServerFnError> {
    let ac = AuthenticatedContext::extract().await?;

    let content = content.unwrap_or_default();

    // Get embedding service for both embedding generation and rechunking
    let embedding_svc = ac
        .ctx
        .embedding
        .wait_ready()
        .await
        .map_err(|e| ServerFnError::new(format!("Embedding service unavailable: {e}")))?;

    let dashboard_id = kyomi_auth::dashboard_service::create_dashboard(
        ac.db(),
        &ac.auth.user_id,
        &ac.ws_id,
        &title,
        &content,
        kyomi_core::models::DocType::Knowledge,
        Some(embedding_svc),
    )
    .await
    .into_sfn()?;
    kyomi_auth::dashboard_service::spawn_embedding_generation(
        ac.ctx.db.clone(),
        embedding_svc.clone(),
        dashboard_id.clone(),
        ac.ws_id.clone(),
        title.trim().to_string(),
        content.clone(),
    );

    if let Some(ws_manager) = &ac.ctx.ws_manager {
        kyomi_auth::websocket::helpers::broadcast_dashboard_sync(
            ac.db(), ws_manager, &dashboard_id, &ac.ws_id,
            kyomi_types::sync::SyncActionType::Insert,
            &ac.auth.user_id,
        ).await;
    }

    Ok(dashboard_id)
}

/// Delete a knowledge document by ID.
///
/// Delegates to `dashboard_service::delete_dashboard`. The service layer
/// does not distinguish doc_type for deletion.
#[server(prefix = "/leptos-api")]
pub async fn delete_knowledge_doc(dashboard_id: String) -> Result<(), ServerFnError> {
    let ac = AuthenticatedContext::extract().await?;

    kyomi_auth::dashboard_service::delete_dashboard(
        ac.db(),
        &dashboard_id,
        &ac.ws_id,
        &ac.auth.user_id,
    )
    .await
    .into_sfn()?;

    if let Some(ws_manager) = &ac.ctx.ws_manager {
        kyomi_auth::websocket::helpers::broadcast_entity_delete(
            ws_manager, kyomi_types::sync::entity_types::KNOWLEDGE,
            &dashboard_id, &ac.ws_id,
        ).await;
    }

    Ok(())
}

// SSR-only import — placed at bottom to match convention.
#[cfg(feature = "ssr")]
use super::{AuthenticatedContext, IntoServerFnError};
