// SPDX-License-Identifier: AGPL-3.0-or-later

//! Workspace models — maps to `workspaces` and `workspace_users` tables.
//!
//! Used by the auth middleware for workspace context enrichment,
//! and by the capability service for feature gating.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::enums::{CatalogRefreshStatus, SubscriptionStatus, SubscriptionTier, WorkspaceRole, WorkspaceStatus};

/// Workspace record — full model matching the Python SQLAlchemy schema.
///
/// All optional fields correspond to nullable DB columns. Fields with
/// `#[sqlx(default)]` handle NULL from old rows where the column has a
/// server default but may contain NULL for pre-existing data.
#[derive(Debug, Clone, sqlx::FromRow, Serialize, Deserialize)]
pub struct Workspace {
    // ── Core identity ───────────────────────────────────────────────
    /// Primary key.
    pub workspace_id: String,

    /// Display name.
    pub name: Option<String>,

    /// Domain.
    pub domain: Option<String>,

    /// Status: trial, active, suspended.
    pub status: WorkspaceStatus,

    /// Admin contact email.
    pub admin_email: Option<String>,

    /// Owner user ID.
    pub owner_user_id: String,

    // ── Subscription ────────────────────────────────────────────────
    /// Subscription tier: free, basic, starter, pro, team, enterprise.
    pub subscription_tier: SubscriptionTier,

    /// Subscription status: trialing, active, past_due, cancelled.
    pub subscription_status: SubscriptionStatus,

    /// Billing cycle: "annual", "monthly", or NULL for free.
    pub billing_cycle: Option<String>,

    /// Subscription period start (from Stripe).
    pub subscription_period_start: Option<DateTime<Utc>>,

    /// Subscription period end (from Stripe).
    pub subscription_period_end: Option<DateTime<Utc>>,

    /// Trial expiration timestamp.
    pub trial_ends_at: Option<DateTime<Utc>>,

    // ── AI credits ──────────────────────────────────────────────────
    /// Accumulated AI credit usage in USD for the current billing period.
    /// DB column is Float (f64). Default 0.0.
    #[sqlx(default)]
    pub ai_credits_used_usd: f64,

    // ── Bundle balances ────────────────────────────────────────────
    /// Purchased AI token bundle balance in USD. Non-expiring.
    /// Deducted as AI features are used. 0.0 = no purchased credits.
    #[sqlx(default)]
    pub ai_bundle_balance_usd: f64,

    /// Purchased analytics event bundle balance. Non-expiring.
    /// Additional events beyond the included 100K/month.
    #[sqlx(default)]
    pub analytics_bundle_events: i64,

    // ── User limits ─────────────────────────────────────────────────
    /// Maximum users allowed in this workspace. NULL means unlimited (999_999).
    pub user_limit: Option<i32>,

    // ── Stripe integration ──────────────────────────────────────────
    pub stripe_customer_id: Option<String>,
    pub stripe_subscription_id: Option<String>,

    // ── Settings / knowledge ────────────────────────────────────────
    /// JSON settings blob (WorkspaceSettings in Python).
    pub settings: Option<serde_json::Value>,

    /// Free-text business knowledge about this workspace's data.
    pub business_knowledge: Option<String>,

    /// When business_knowledge was last updated.
    pub knowledge_updated_at: Option<DateTime<Utc>>,

    // ── Catalog ─────────────────────────────────────────────────────
    /// Last time the schema catalog was refreshed.
    pub last_catalog_refresh: Option<DateTime<Utc>>,

    /// Catalog refresh status: idle, running, etc.
    pub catalog_refresh_status: Option<CatalogRefreshStatus>,

    /// JSON progress object during catalog refresh.
    pub catalog_refresh_progress: Option<serde_json::Value>,

    /// Whether the catalog onboarding flow is completed.
    #[sqlx(default)]
    pub catalog_onboarding_completed: bool,

    /// JSON array of indexed BigQuery projects.
    pub catalog_indexed_projects: Option<serde_json::Value>,

    // ── Timestamps ──────────────────────────────────────────────────
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// Workspace-User membership record.
///
/// Maps to the `workspace_users` table.
#[derive(Debug, Clone, sqlx::FromRow, Serialize, Deserialize)]
pub struct WorkspaceUser {
    /// Auto-increment primary key.
    pub id: i32,

    /// Workspace ID.
    pub workspace_id: String,

    /// User ID.
    pub user_id: String,

    /// Role: workspace_admin, user, viewer.
    pub role: WorkspaceRole,

    /// Whether this membership is active.
    pub active: bool,

    /// When membership was created.
    pub created_at: DateTime<Utc>,

    /// Last time the user was active in this workspace.
    pub last_active: Option<DateTime<Utc>>,

    /// Flexible JSON metadata.
    pub extra_metadata: Option<serde_json::Value>,
}
