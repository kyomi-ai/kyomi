// SPDX-License-Identifier: AGPL-3.0-or-later

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
    /// Whether the catalog needs attention (no tables, no index, or stale)
    pub needs_catalog_attention: bool,
}
