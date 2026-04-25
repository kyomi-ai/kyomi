// SPDX-License-Identifier: AGPL-3.0-or-later

//! Server functions for the Accept Ownership page.
//!
//! These handle fetching, accepting, and declining ownership transfers.
//! The page is a standalone route (`/accept-ownership/:transferId`) that
//! does not use the main layout wrapper.

use leptos::prelude::*;
use serde::{Deserialize, Serialize};

// ─────────────────────────────────────────────────────────────────────────────
// Types
// ─────────────────────────────────────────────────────────────────────────────

/// Transfer details for display on the accept-ownership page.
///
/// This is a slimmer type than `OwnershipTransferData` in `types.rs` — it
/// includes the workspace name (resolved from the workspace record) and
/// only the fields the page actually needs.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct OwnershipTransfer {
    pub transfer_id: String,
    pub workspace_name: String,
    pub from_user_email: String,
    pub expires_at: String,
    pub status: String,
}

// ─────────────────────────────────────────────────────────────────────────────
// Server functions
// ─────────────────────────────────────────────────────────────────────────────

/// Fetch a specific ownership transfer by ID.
///
/// Returns `Ok(Some(transfer))` if a pending transfer exists for the
/// authenticated user, `Ok(None)` if not found / not pending / expired.
///
/// Mirrors the React flow: fetch all pending transfers for the user,
/// find the one matching `transfer_id`, verify status == "pending".
#[server(prefix = "/leptos-api")]
pub async fn get_ownership_transfer(
    transfer_id: String,
) -> Result<Option<OwnershipTransfer>, ServerFnError> {
    let auth = extract_auth().await?;
    let ctx = extract_context()?;

    let detail = kyomi_auth::workspace_service::get_transfer_for_recipient(
        &ctx.db,
        &transfer_id,
        &auth.user_id,
    )
    .await
    .into_sfn()?;

    Ok(detail.map(|d| OwnershipTransfer {
        transfer_id: d.transfer_id,
        workspace_name: d.workspace_name,
        from_user_email: d.from_user_email,
        expires_at: d.expires_at.to_rfc3339(),
        status: d.status.to_string(),
    }))
}

/// Accept an ownership transfer. Only the recipient can accept.
///
/// Mirrors `POST /api/v1/workspaces/ownership/transfer/{id}/accept`.
#[server(prefix = "/leptos-api")]
pub async fn accept_ownership_transfer(
    transfer_id: String,
) -> Result<(), ServerFnError> {
    let auth = extract_auth().await?;
    let ctx = extract_context()?;

    let transfer = kyomi_auth::workspace_service::get_ownership_transfer(&ctx.db, &transfer_id)
        .await
        .into_sfn()?
        .ok_or_else(|| ServerFnError::new("Transfer not found"))?;

    // Authorization first — recipient check before status/expiry
    if transfer.to_user_id != auth.user_id {
        return Err(ServerFnError::new(
            "Only the transfer recipient can accept",
        ));
    }

    if transfer.status != kyomi_core::enums::TransferStatus::Pending {
        return Err(ServerFnError::new("Transfer is no longer pending"));
    }

    if transfer.expires_at < chrono::Utc::now() {
        let _ = kyomi_auth::workspace_service::update_transfer_status(
            &ctx.db,
            &transfer_id,
            "expired",
        )
        .await;
        return Err(ServerFnError::new("Transfer has expired"));
    }

    kyomi_auth::workspace_service::complete_ownership_transfer(
        &ctx.db,
        &transfer_id,
        &transfer.workspace_id,
        &auth.user_id,
    )
    .await
    .into_sfn()?;

    tracing::info!(
        "Ownership transfer {} accepted: workspace {} now owned by {}",
        transfer_id,
        transfer.workspace_id,
        auth.user_id
    );

    Ok(())
}

/// Decline an ownership transfer. Only the recipient can decline.
///
/// Mirrors `POST /api/v1/workspaces/ownership/transfer/{id}/decline`.
#[server(prefix = "/leptos-api")]
pub async fn decline_ownership_transfer(
    transfer_id: String,
) -> Result<(), ServerFnError> {
    let auth = extract_auth().await?;
    let ctx = extract_context()?;

    let transfer = kyomi_auth::workspace_service::get_ownership_transfer(&ctx.db, &transfer_id)
        .await
        .into_sfn()?
        .ok_or_else(|| ServerFnError::new("Transfer not found"))?;

    // Authorization first — recipient check before status
    if transfer.to_user_id != auth.user_id {
        return Err(ServerFnError::new(
            "Only the transfer recipient can decline",
        ));
    }

    if transfer.status != kyomi_core::enums::TransferStatus::Pending {
        return Err(ServerFnError::new("Transfer is no longer pending"));
    }

    kyomi_auth::workspace_service::update_transfer_status(&ctx.db, &transfer_id, "declined")
        .await
        .into_sfn()?;

    tracing::info!(
        "Ownership transfer {} declined by {}",
        transfer_id,
        auth.user_id
    );

    Ok(())
}

// Helpers — delegate to shared extractors in parent module
#[cfg(feature = "ssr")]
use super::{extract_auth, extract_context, IntoServerFnError};
