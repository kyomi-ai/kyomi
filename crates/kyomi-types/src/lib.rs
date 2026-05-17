// SPDX-License-Identifier: AGPL-3.0-or-later

pub mod sync;
pub mod websocket;
pub use websocket::{MessageType, WebSocketMessage};

use serde::{Deserialize, Serialize};

/// A datasource with its per-user credential status.
///
/// Returned by [`kyomi_auth::datasource_service::list_datasources_with_status`].
/// Combines the datasource config with credential and catalog information so
/// the UI can render everything in one pass.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct DatasourceInfo {
    pub id: String,
    pub name: String,
    pub slug: String,
    pub datasource_type: String,
    pub type_display_name: String,
    pub active: bool,
    pub connection_type: String,
    /// User's credential status: "valid", "shared", "missing", "expired"
    pub credential_status: String,
    /// Auth method: "oauth", "password", "connect"
    pub auth_method: String,
    /// Whether the user has this datasource enabled
    pub user_enabled: bool,
    /// Whether the user can enable this datasource
    pub can_enable: bool,
    /// Whether this is a sample datasource
    pub is_sample: bool,
    /// Whether this is an analytics datasource (auto-provisioned by the analytics site system)
    pub is_analytics: bool,
    /// Whether the catalog needs attention (no tables, no index, or stale)
    pub needs_catalog_attention: bool,
    /// The `auth_mode` from `connection_config` (e.g. `"kyomi_oauth"`, `"enterprise_oauth"`, `"oauth"`, `"token"`).
    /// `None` when the field is absent from the stored config.
    pub auth_mode: Option<String>,
}

/// Status of a single datasource's credentials for the current user.
///
/// Used by the onboarding page to show which datasources need credential
/// setup and what action the user should take (OAuth connect, password entry).
/// Shared between `kyomi_auth::onboarding_service` and the Leptos server_fn
/// layer so both sides use the same type without a conversion step.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CredentialStatusItem {
    pub datasource_id: String,
    pub datasource_name: String,
    pub datasource_type: String,
    pub slug: String,
    /// `"valid"` | `"expired"` | `"missing"` | `"shared"`
    pub status: String,
    /// `"password"` | `"oauth"` | `"connect"` — determines the UI action button
    pub auth_method: String,
    /// For OAuth providers: `"google"` | `"snowflake"` | `"microsoft"` | `"databricks"`
    pub oauth_provider: Option<String>,
    /// The `auth_mode` from the datasource `connection_config`.
    /// For BigQuery: `"kyomi_oauth"` | `"enterprise_oauth"` | `"service_account"`
    pub auth_mode: Option<String>,
    /// True if the user needs to take action (missing or expired).
    pub needs_action: bool,
}

/// Combined onboarding state fetched in a single call.
///
/// The onboarding page uses this to decide which of the 5 states to show
/// without making multiple sequential API calls.
/// Shared between `kyomi_auth::onboarding_service` and the Leptos server_fn
/// layer so both sides use the same type without a conversion step.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct OnboardingState {
    pub has_datasources: bool,
    pub is_admin: bool,
    pub sample_available: bool,
    pub needs_credentials: bool,
    pub total_datasources: usize,
    pub credential_status: Vec<CredentialStatusItem>,
}

/// Per-user fair-share usage info.
///
/// Shared between `kyomi_auth::billing_service` and the Leptos server_fn
/// layer so both sides use the same type without a conversion step.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PerUserUsage {
    pub percentage_used: f64,
    pub fair_share_percentage: f64,
}

/// User attribution — who created or sent something.
///
/// Unified type replacing the two local `CreatedBy` structs that existed
/// separately in `kyomi_auth::chat_service` and `kyomi_auth::dashboard_service`.
///
/// - `display_name`: display label used by chat (was non-optional there, but
///   dashboard_service had no such field, so it becomes `Option`).
/// - `name`:  structured name field from dashboard versions (was `Option<String>`).
/// - `email`: email address from dashboard versions (was non-optional there,
///   but chat_service had no email, so it becomes `Option`).
///
/// All formerly non-optional fields are `Option` so both call sites can omit
/// fields they don't have, and old JSON without those fields deserializes via
/// `#[serde(default)]`.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct CreatedBy {
    pub user_id: String,
    #[serde(default)]
    pub display_name: Option<String>,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub email: Option<String>,
}
