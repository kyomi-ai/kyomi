// SPDX-License-Identifier: AGPL-3.0-or-later

//! Ownership transfer model — maps to `ownership_transfers` table.
//!
//! Used for transferring workspace ownership between users.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::enums::TransferStatus;

/// A pending or completed workspace ownership transfer request.
///
/// Not a duplicate of `kyomi_ui::server_fns::ownership::OwnershipTransfer`:
/// this is the full `sqlx::FromRow` DB row for the `ownership_transfers`
/// table, while that one is a slimmer page-view DTO with `workspace_name`
/// and `from_user_email` resolved via a join.
#[derive(Debug, Clone, sqlx::FromRow, Serialize, Deserialize)]
pub struct OwnershipTransfer {
    /// Primary key: "transfer-{uuid_hex[0..20]}".
    pub transfer_id: String,

    /// Workspace being transferred.
    pub workspace_id: String,

    /// Current owner (initiator of the transfer).
    pub from_user_id: String,

    /// Target new owner.
    pub to_user_id: String,

    /// Transfer status: pending, accepted, declined, cancelled.
    pub status: TransferStatus,

    /// When the transfer was initiated.
    pub created_at: DateTime<Utc>,

    /// When the transfer expires if not acted upon.
    pub expires_at: DateTime<Utc>,

    /// When the transfer was completed (accepted/declined/cancelled).
    pub completed_at: Option<DateTime<Utc>>,
}
