// SPDX-License-Identifier: AGPL-3.0-or-later

//! Workspace permission catalog.
//!
//! `Permission` enumerates every capability a workspace member can be
//! granted, independent of *how* the grant is decided. Deciding which
//! permissions a given user holds is `kyomi_auth::permissions::permissions_for`
//! — the single role→capability mapping (KYO-189 P1). This enum lives in
//! `kyomi-types` rather than `kyomi-core` because `kyomi-core` is an
//! `ssr`-only dependency of `kyomi-ui` (see that crate's `Cargo.toml`);
//! `kyomi-types` is unconditional, so it's the only shared crate reachable
//! from both the server and the `wasm32-unknown-unknown` client build.
//!
//! Every variant here corresponds to a real enforcement point that existed
//! before this refactor — see the KYO-189 P1 report for the file:line
//! mapping. Do not add a variant unless something actually checks it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Permission {
    /// Create, update, delete, and view settings for conventional
    /// (non-Connect) datasources. Also covers generating SSH keys for
    /// tunnel-based datasources and viewing an inactive datasource's
    /// settings.
    ManageDatasources,
    /// Trigger a manual catalog refresh for a datasource.
    RefreshCatalog,
    /// Update workspace-level settings: name, default AI model, title-model
    /// override, ChartML palette.
    ManageWorkspaceSettings,
    /// Manage workspace membership: update member roles, remove members,
    /// create/list/cancel invitations.
    ManageTeam,
    /// Configure the workspace's BYOK AI provider (API key, base URL,
    /// model), test keys, and list available models.
    ManageAiConfig,
    /// Create, update, delete, and list analytics sites.
    ManageAnalytics,
    /// Create and manage Kyomi Connect datasources: create, rotate/revoke
    /// tokens, check agent connection status, discover containers.
    ManageConnect,
    /// View and change the workspace's subscription: plan, seat cap,
    /// checkout/portal sessions, AI bundle purchases. Owner-only — the
    /// owner is the workspace's single spending authority.
    ManageBilling,
    /// Set or clear the workspace's default dashboard.
    SetWorkspaceDefaults,
    /// Install, configure, or remove third-party workspace integrations
    /// (currently: Slack app install/uninstall).
    ManageIntegrations,
    /// Initiate a workspace ownership transfer to another member. Owner-only
    /// — an admin cannot give away ownership they don't hold.
    TransferOwnership,
}
