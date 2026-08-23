// SPDX-License-Identifier: AGPL-3.0-or-later

//! Datasource type registry — static metadata for all supported datasource types.
//!
//! Mirrors the Python `DatasourceTypeRegistry` + `DatasourceTypeMetadata` +
//! `AuthModeConfig` from `datasources/registry.py` and `datasources/auth_modes.py`.
//!
//! The Rust version is a static registry (match-based) rather than a runtime
//! self-registration pattern, since all types are known at compile time.

use std::collections::HashMap;
use std::str::FromStr;

use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// DatasourceType — re-exported from kyomi-connect-protocol
// ---------------------------------------------------------------------------

pub use kyomi_connect_protocol::types::DatasourceType;

/// All variants in the canonical order (matches Python's list).
const ALL_TYPES: [DatasourceType; 10] = [
    DatasourceType::BigQuery,
    DatasourceType::ClickHouse,
    DatasourceType::Snowflake,
    DatasourceType::Databricks,
    DatasourceType::Redshift,
    DatasourceType::Postgres,
    DatasourceType::MySQL,
    DatasourceType::SqlServer,
    DatasourceType::Synapse,
    DatasourceType::FlareDb,
];

// ---------------------------------------------------------------------------
// AuthModeConfig
// ---------------------------------------------------------------------------

/// Authentication mode configuration for a datasource type.
///
/// Mirrors Python's `AuthModeConfig` from `datasources/auth_modes.py`.
/// Contains all fields needed for credential handling, UI rendering,
/// and routing logic.
///
/// Serialized to JSON for the `/types` endpoint.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuthModeConfig {
    // === Identity ===
    /// Internal identifier matching `connection_config.auth_mode`
    /// (e.g., `"password"`, `"kyomi_oauth"`, `"service_account"`).
    pub mode_id: String,

    /// Human-readable name shown in UI
    /// (e.g., `"Password"`, `"Google OAuth (Kyomi)"`).
    pub display_name: String,

    /// Brief description for UI tooltips
    /// (e.g., `"Authenticate with database username and password"`).
    pub description: String,

    // === Credential Type Configuration ===
    /// Category of credentials for routing logic.
    ///
    /// Values: `"password"`, `"oauth_global"`, `"oauth_per_datasource"`,
    /// `"service_account"`, `"token"`, `"keypair"`, `"none"`.
    pub credential_type: String,

    /// OAuth provider name if applicable (e.g., `"google"`, `"snowflake"`, `"microsoft"`).
    pub oauth_provider: Option<String>,

    /// `true` if uses app-level OAuth (`User.oauth_data`),
    /// `false` for enterprise OAuth (per-datasource tokens).
    pub oauth_global: bool,

    // === Scope Configuration ===
    /// Where credentials are stored/managed.
    ///
    /// Values: `"user"` (per-user), `"workspace"` (connection_config),
    /// `"global_oauth"` (User.oauth_data).
    pub credential_scope: String,

    /// Which table tracks the user's enabled preference.
    ///
    /// Values: `"credential"` (UserDatasourceCredential.enabled),
    /// `"preference"` (UserDatasourcePreference.enabled).
    pub preference_tracking: String,

    // === Field Configuration ===
    /// Credential field names required from users (e.g., `["username", "password"]`).
    pub credential_fields: Vec<String>,

    /// Fields to mask in API responses (e.g., `["password"]`).
    pub sensitive_fields: Vec<String>,

    // === Flags ===
    /// `true` if this is the default auth mode for the datasource.
    pub is_default: bool,

    /// `true` if workspace can share credentials for this auth mode.
    pub supports_shared_credentials: bool,

    /// Whether this mode can authenticate a *headless* background catalog-indexing
    /// run. Interactive OAuth modes cannot: there is no user session to complete the
    /// flow. Modes with no credentials at all (`none`) have nothing to store, so the
    /// indexing credential form has nothing to offer.
    ///
    /// Rust-side only — there is no Python counterpart for this field.
    pub supports_headless_indexing: bool,
}

impl AuthModeConfig {
    /// Check if this auth mode uses shared/workspace-level authentication.
    ///
    /// Shared auth means users don't provide individual credentials.
    /// Their enabled/disabled preference is tracked in `UserDatasourcePreference`.
    pub fn is_shared_auth(&self) -> bool {
        self.preference_tracking == "preference"
    }

    /// Check if this auth mode requires OAuth authentication.
    pub fn requires_oauth(&self) -> bool {
        self.credential_type == "oauth_global" || self.credential_type == "oauth_per_datasource"
    }

    /// Check if users need to provide their own credentials.
    pub fn requires_user_credentials(&self) -> bool {
        self.credential_scope == "user"
            && matches!(
                self.credential_type.as_str(),
                "password" | "token" | "keypair" | "oauth_per_datasource"
            )
    }
}

// ---------------------------------------------------------------------------
// AuthModeConfig factory functions
// ---------------------------------------------------------------------------
// These mirror the Python factory functions in `datasources/auth_modes.py`.

/// Create a password authentication mode.
///
/// Used by: PostgreSQL, MySQL, ClickHouse, Redshift, SQL Server.
fn password_auth_mode(is_default: bool, supports_shared: bool) -> AuthModeConfig {
    AuthModeConfig {
        mode_id: "password".into(),
        display_name: "Password".into(),
        description: "Authenticate with database username and password".into(),
        credential_type: "password".into(),
        oauth_provider: None,
        oauth_global: false,
        credential_scope: "user".into(),
        preference_tracking: "credential".into(),
        credential_fields: vec!["username".into(), "password".into()],
        sensitive_fields: vec!["password".into()],
        is_default,
        supports_shared_credentials: supports_shared,
        supports_headless_indexing: true,
    }
}

/// Create a global OAuth authentication mode (app-level OAuth, e.g., Kyomi's Google OAuth).
fn global_oauth_auth_mode(oauth_provider: &str, is_default: bool) -> AuthModeConfig {
    let provider_display = match oauth_provider {
        "google" => "Google",
        "microsoft" => "Microsoft",
        "snowflake" => "Snowflake",
        _ => oauth_provider,
    };
    AuthModeConfig {
        mode_id: "kyomi_oauth".into(),
        display_name: format!("{provider_display} OAuth (Kyomi)"),
        description: format!(
            "Sign in with your {provider_display} account using Kyomi's OAuth app"
        ),
        credential_type: "oauth_global".into(),
        oauth_provider: Some(oauth_provider.into()),
        oauth_global: true,
        credential_scope: "global_oauth".into(),
        preference_tracking: "preference".into(),
        credential_fields: vec![],
        sensitive_fields: vec![],
        is_default,
        supports_shared_credentials: false,
        supports_headless_indexing: false,
    }
}

/// Create an enterprise OAuth authentication mode (customer's OAuth client per datasource).
fn enterprise_oauth_auth_mode(
    oauth_provider: &str,
    is_default: bool,
    display_name: Option<&str>,
    description: Option<&str>,
) -> AuthModeConfig {
    let provider_display = match oauth_provider {
        "google" => "Google",
        "microsoft" => "Microsoft",
        "snowflake" => "Snowflake",
        _ => oauth_provider,
    };
    AuthModeConfig {
        mode_id: "enterprise_oauth".into(),
        display_name: display_name
            .map(String::from)
            .unwrap_or_else(|| format!("{provider_display} OAuth (Enterprise)")),
        description: description
            .map(String::from)
            .unwrap_or_else(|| {
                format!("Use your organization's {provider_display} OAuth configuration")
            }),
        credential_type: "oauth_per_datasource".into(),
        oauth_provider: Some(oauth_provider.into()),
        oauth_global: false,
        credential_scope: "user".into(),
        preference_tracking: "credential".into(),
        credential_fields: vec!["oauth_token".into()],
        sensitive_fields: vec!["oauth_token".into()],
        is_default,
        supports_shared_credentials: false,
        supports_headless_indexing: false,
    }
}

/// Create an OAuth authentication mode with `mode_id="oauth"`.
///
/// Similar to enterprise OAuth but uses `mode_id="oauth"` to match frontend
/// schemas (e.g., Snowflake, Databricks).
fn oauth_auth_mode(
    oauth_provider: &str,
    is_default: bool,
    display_name: Option<&str>,
    description: Option<&str>,
) -> AuthModeConfig {
    let provider_display = match oauth_provider {
        "google" => "Google",
        "microsoft" => "Microsoft",
        "snowflake" => "Snowflake",
        "databricks" => "Databricks",
        _ => oauth_provider,
    };
    AuthModeConfig {
        mode_id: "oauth".into(),
        display_name: display_name
            .map(String::from)
            .unwrap_or_else(|| format!("{provider_display} OAuth")),
        description: description
            .map(String::from)
            .unwrap_or_else(|| {
                format!("Authenticate with your {provider_display} account via OAuth")
            }),
        credential_type: "oauth_per_datasource".into(),
        oauth_provider: Some(oauth_provider.into()),
        oauth_global: false,
        credential_scope: "user".into(),
        preference_tracking: "credential".into(),
        credential_fields: vec!["oauth_token".into()],
        sensitive_fields: vec!["oauth_token".into()],
        is_default,
        supports_shared_credentials: false,
        supports_headless_indexing: false,
    }
}

/// Create a service account authentication mode (workspace-level shared auth).
fn service_account_auth_mode(is_default: bool) -> AuthModeConfig {
    AuthModeConfig {
        mode_id: "service_account".into(),
        display_name: "Service Account".into(),
        // KYO-274: the previous copy ("Use a service account for
        // server-side authentication") didn't say who shares it. That's
        // the one fact that actually distinguishes this mode from the
        // per-user password/OAuth modes above — every workspace member
        // authenticates as the same identity — so it belongs in the
        // description, not just in `credential_scope: "workspace"`.
        description: "All users share a service account for automated access".into(),
        credential_type: "service_account".into(),
        oauth_provider: None,
        oauth_global: false,
        credential_scope: "workspace".into(),
        preference_tracking: "preference".into(),
        credential_fields: vec![],
        sensitive_fields: vec![],
        is_default,
        supports_shared_credentials: true,
        supports_headless_indexing: true,
    }
}

/// Create a token authentication mode (e.g., Databricks personal access token).
fn token_auth_mode(
    is_default: bool,
    token_field: &str,
    display_name: &str,
    description: &str,
    supports_shared: bool,
) -> AuthModeConfig {
    AuthModeConfig {
        mode_id: "token".into(),
        display_name: display_name.into(),
        description: description.into(),
        credential_type: "token".into(),
        oauth_provider: None,
        oauth_global: false,
        credential_scope: "user".into(),
        preference_tracking: "credential".into(),
        credential_fields: vec![token_field.into()],
        sensitive_fields: vec![token_field.into()],
        is_default,
        supports_shared_credentials: supports_shared,
        supports_headless_indexing: true,
    }
}

// ---------------------------------------------------------------------------
// DatasourceTypeMetadata
// ---------------------------------------------------------------------------

/// Complete metadata for a datasource type, returned by [`get_metadata`].
///
/// This is a static struct returned as a `&'static` reference from the registry.
#[derive(Debug)]
pub struct DatasourceTypeMetadata {
    /// Internal type identifier (e.g., `"postgres"`).
    pub type_id: &'static str,

    /// Human-readable name (e.g., `"PostgreSQL"`).
    pub display_name: &'static str,

    /// Brief description (e.g., `"PostgreSQL database"`).
    pub description: &'static str,

    /// Default port, or `None` for API-based (BigQuery, Snowflake).
    pub default_port: Option<u16>,

    /// Credential field names required from users (e.g., `["username", "password"]`).
    pub credential_fields: &'static [&'static str],

    /// Credential fields that must be masked in API responses (e.g., `["password"]`).
    pub sensitive_credential_fields: &'static [&'static str],

    /// Connection config fields that must be masked in API responses
    /// (e.g., `["oauth_client_secret", "service_account_json"]`).
    ///
    /// Note: `shared_password`, `ssh_private_key`, and `ssh_passphrase` are
    /// always masked (and encrypted at rest) regardless of this list — see
    /// `COMMON_SENSITIVE` in `kyomi_auth::credential_service`.
    pub sensitive_connection_config_fields: &'static [&'static str],

    /// Whether users must provide their own credentials.
    pub requires_user_credentials: bool,

    /// Whether the provider needs user context (OAuth flow).
    pub accepts_user_context: bool,

    /// Supported authentication modes.
    pub auth_modes: &'static [AuthModeConfig],

    /// UI label for catalog containers (singular: `"schema"`, `"project"`, `"database"`).
    pub catalog_container_label: &'static str,

    /// Config keys for catalog status (e.g., `["catalog_schemas"]`).
    pub catalog_config_keys: &'static [&'static str],

    /// Whether the provider supports listing schemas/databases for discovery.
    pub supports_catalog_discovery: bool,

    /// Tree level 1 node type (e.g., `"project"` for BigQuery, `"database"` for Snowflake/SQL Server).
    ///
    /// Used when building hierarchical catalog trees. Only meaningful when
    /// `skip_empty_project_wrapper` and `skip_single_project_wrapper` are both `false`.
    pub tree_level1_type: &'static str,

    /// Tree level 2 node type (e.g., `"dataset"` for BigQuery, `"schema"` for Postgres).
    ///
    /// This is the container level that holds tables in the catalog tree.
    pub tree_level2_type: &'static str,

    /// Skip the level-1 wrapper when `project_id` is empty.
    ///
    /// Set to `true` for datasources where `project_id` is always empty
    /// (e.g., PostgreSQL, MySQL, ClickHouse, Redshift).
    pub skip_empty_project_wrapper: bool,

    /// Skip the level-1 wrapper when there is only a single project.
    ///
    /// Set to `true` for single-database datasources where showing the project
    /// wrapper is redundant (e.g., PostgreSQL, MySQL).
    pub skip_single_project_wrapper: bool,

    /// Whether this datasource type supports connecting through an SSH tunnel.
    ///
    /// `true` for direct-TCP database protocols (PostgreSQL, MySQL, Redshift,
    /// ClickHouse, SQL Server, Azure Synapse). `false` for API/HTTPS-based
    /// providers (BigQuery, Snowflake, Databricks, FlareDB) where a tunnel
    /// doesn't apply.
    pub supports_ssh_tunnel: bool,
}

impl DatasourceTypeMetadata {
    /// Get an [`AuthModeConfig`] by its `mode_id`.
    ///
    /// Returns `None` if no auth mode with that ID exists.
    pub fn get_auth_mode(&self, mode_id: &str) -> Option<&AuthModeConfig> {
        self.auth_modes.iter().find(|m| m.mode_id == mode_id)
    }

    /// Get the default [`AuthModeConfig`] for this datasource.
    ///
    /// Returns the mode with `is_default == true`, or the first mode if none
    /// is explicitly marked as default, or `None` if there are no auth modes.
    pub fn get_default_auth_mode(&self) -> Option<&AuthModeConfig> {
        // First, look for explicitly marked default
        if let Some(mode) = self.auth_modes.iter().find(|m| m.is_default) {
            return Some(mode);
        }
        // Fall back to first mode
        self.auth_modes.first()
    }

    /// Get the active [`AuthModeConfig`] based on `connection_config`.
    ///
    /// Looks up `auth_mode` from the config map and returns the matching
    /// `AuthModeConfig`. Falls back to the default if not specified.
    pub fn get_active_auth_mode(
        &self,
        connection_config: &HashMap<String, serde_json::Value>,
    ) -> Option<&AuthModeConfig> {
        if let Some(serde_json::Value::String(mode_id)) = connection_config.get("auth_mode")
            && let Some(mode) = self.get_auth_mode(mode_id)
        {
            return Some(mode);
        }
        // Fall back to default
        self.get_default_auth_mode()
    }

    /// Check if the active auth mode uses shared/workspace-level authentication.
    ///
    /// Shared auth means users don't provide individual credentials.
    pub fn is_shared_auth(
        &self,
        connection_config: &HashMap<String, serde_json::Value>,
    ) -> bool {
        // Check explicit shared_credentials flag first
        if let Some(serde_json::Value::Bool(true)) = connection_config.get("shared_credentials") {
            return true;
        }

        // Get active auth mode and check its preference_tracking
        if let Some(active_mode) = self.get_active_auth_mode(connection_config) {
            return active_mode.is_shared_auth();
        }

        // Legacy fallback for known shared auth mode IDs
        if let Some(serde_json::Value::String(mode_id)) = connection_config.get("auth_mode") {
            return mode_id == "service_account" || mode_id == "kyomi_oauth";
        }

        false
    }

    /// Get list of all auth mode IDs for this datasource.
    pub fn get_auth_mode_ids(&self) -> Vec<&str> {
        self.auth_modes.iter().map(|m| m.mode_id.as_str()).collect()
    }

    /// Auth modes usable for headless catalog-indexing credentials.
    ///
    /// This is a strict subset of `auth_modes`: interactive OAuth modes are
    /// excluded because there is no user session for a background job to
    /// complete the flow with, and modes with no credentials at all (`none`)
    /// have nothing for the indexing credential form to collect. See
    /// [`AuthModeConfig::supports_headless_indexing`].
    pub fn indexing_auth_modes(&self) -> impl Iterator<Item = &AuthModeConfig> {
        self.auth_modes.iter().filter(|m| m.supports_headless_indexing)
    }
}

// ---------------------------------------------------------------------------
// Static metadata per type
// ---------------------------------------------------------------------------

use std::sync::LazyLock;

/// Leak a `Vec<AuthModeConfig>` to get a `&'static [AuthModeConfig]`.
///
/// This is called exactly once per datasource type (10 calls total) during
/// `LazyLock` initialization. The leaked memory is program-lifetime data
/// that is never freed — this is intentional and safe because:
///
/// - There are exactly 10 datasource types, each with 1-4 auth modes.
/// - Total leaked memory is under 5 KB for all types combined.
/// - The data lives for the entire program lifetime (same as `&'static`).
/// - `LazyLock` ensures each call happens exactly once.
///
/// This pattern avoids the complexity of `const` construction for types
/// containing `String` fields while maintaining `&'static` references.
fn leak_auth_modes(modes: Vec<AuthModeConfig>) -> &'static [AuthModeConfig] {
    Vec::leak(modes)
}

// --- BigQuery ---
// Python: apps/backend-python/src/api/datasources/bigquery/__init__.py
static BIGQUERY_META: LazyLock<DatasourceTypeMetadata> = LazyLock::new(|| DatasourceTypeMetadata {
    type_id: "bigquery",
    display_name: "BigQuery",
    description: "Google Cloud BigQuery",
    default_port: None,
    credential_fields: &["billing_project", "query_size_limit_gb"],
    sensitive_credential_fields: &[],
    sensitive_connection_config_fields: &["oauth_client_secret", "service_account_json"],
    requires_user_credentials: false,
    accepts_user_context: true,
    auth_modes: leak_auth_modes(vec![
        global_oauth_auth_mode("google", true),
        enterprise_oauth_auth_mode("google", false, None, None),
        service_account_auth_mode(false),
    ]),
    catalog_container_label: "project",
    catalog_config_keys: &["catalog_projects", "include_public_datasets"],
    supports_catalog_discovery: true,
    tree_level1_type: "project",
    tree_level2_type: "dataset",
    skip_empty_project_wrapper: false,
    skip_single_project_wrapper: false,
    supports_ssh_tunnel: false,
});

// --- ClickHouse ---
// Python: apps/backend-python/src/api/datasources/clickhouse/__init__.py
static CLICKHOUSE_META: LazyLock<DatasourceTypeMetadata> =
    LazyLock::new(|| DatasourceTypeMetadata {
        type_id: "clickhouse",
        display_name: "ClickHouse",
        description: "ClickHouse analytics database",
        default_port: Some(8123),
        credential_fields: &["username", "password"],
        sensitive_credential_fields: &["password"],
        sensitive_connection_config_fields: &[],
        requires_user_credentials: true,
        accepts_user_context: false,
        auth_modes: leak_auth_modes(vec![password_auth_mode(true, true)]),
        catalog_container_label: "database",
        catalog_config_keys: &["catalog_databases"],
        supports_catalog_discovery: true,
        tree_level1_type: "database",
        tree_level2_type: "database",
        skip_empty_project_wrapper: true,
        skip_single_project_wrapper: true,
        supports_ssh_tunnel: true,
    });

// --- Snowflake ---
// Python: apps/backend-python/src/api/datasources/snowflake/__init__.py
// auth_modes: [password_auth_mode(is_default=True), oauth_auth_mode(oauth_provider="snowflake")]
// The Python source predates key-pair auth (KYO-274) and only ever masked
// "password". Key-pair mode's own PEM `private_key` field (see the `keypair`
// AuthModeConfig below) is just as sensitive and is masked here too — see
// KYO-330.
// No sensitive_connection_config_fields
static SNOWFLAKE_META: LazyLock<DatasourceTypeMetadata> =
    LazyLock::new(|| DatasourceTypeMetadata {
        type_id: "snowflake",
        display_name: "Snowflake",
        description: "Snowflake cloud data warehouse",
        default_port: None,
        // Type-level field surface across all auth modes: password mode's
        // ["username", "password"] plus keypair mode's "private_key" (see
        // the `keypair` AuthModeConfig below, which lists the same three
        // fields at the per-mode level).
        credential_fields: &["username", "password", "private_key"],
        sensitive_credential_fields: &["password", "private_key"],
        sensitive_connection_config_fields: &[],
        requires_user_credentials: true,
        accepts_user_context: false,
        auth_modes: leak_auth_modes(vec![
            password_auth_mode(true, true),
            oauth_auth_mode("snowflake", false, None, None),
            // Key-pair auth (KYO-274). This mode was already fully wired in
            // `kyomi-ui` — a dedicated credential field (`cred_private_key`),
            // its own field-set UI, and a live server-side credential-status
            // branch (`datasource_auth_service.rs`'s `"keypair"` arm) — but
            // had no registry entry at all, so it was invisible to every
            // registry-driven consumer. That included
            // `indexing_auth_modes()` (KYO-187): a workspace using Snowflake
            // key-pair auth could not select key-pair for catalog-indexing
            // credentials, because the indexing selector is built from this
            // list and key-pair simply wasn't in it. `supports_headless_indexing:
            // true` below is what fixes that — a key-pair is precisely the
            // kind of credential that authenticates without an interactive
            // session, exactly like `password` and `token`
            // (`datasource_auth_service.rs:345-356` already treats it that
            // way).
            AuthModeConfig {
                mode_id: "keypair".into(),
                display_name: "Key Pair".into(),
                description: "Authenticate using an RSA key pair".into(),
                credential_type: "keypair".into(),
                oauth_provider: None,
                oauth_global: false,
                // Matches `password_auth_mode`: a personal, per-user
                // credential tracked via `UserDatasourceCredential.enabled`,
                // same as password. No reason found to diverge.
                credential_scope: "user".into(),
                preference_tracking: "credential".into(),
                // Mirrors exactly what `kyomi-ui`'s connection-config /
                // credential builder persists for this mode
                // (`datasources.rs:2028-2037` writes `username` + `password`
                // + `private_key`; the keypair-mode field UI at
                // `datasources.rs:5946-5985` shows "Username" and "Private
                // Key (PEM)" as required, and "Private Key Passphrase" —
                // stored under the `password` key — as optional). All three
                // are listed here since `credential_fields` documents the
                // field surface, not just the required subset (compare
                // `password_auth_mode`, which lists both of its fields
                // despite neither being conditionally optional in the same
                // way). `password` and `private_key` both carry secrets, so
                // both are sensitive.
                credential_fields: vec!["username".into(), "password".into(), "private_key".into()],
                sensitive_fields: vec!["password".into(), "private_key".into()],
                is_default: false,
                // The "Shared credentials (all users)" toggle
                // (`ProviderCredentialsFields`, `datasources.rs:5908-5920`)
                // is not excluded for Snowflake's keypair mode — it falls
                // through to the same generic branch that renders the
                // toggle for `password` mode. A workspace admin can already
                // configure one shared key-pair for every member today.
                supports_shared_credentials: true,
                supports_headless_indexing: true,
            },
        ]),
        catalog_container_label: "database",
        catalog_config_keys: &["catalog_databases"],
        supports_catalog_discovery: true,
        tree_level1_type: "database",
        tree_level2_type: "schema",
        skip_empty_project_wrapper: true,
        skip_single_project_wrapper: false,
        supports_ssh_tunnel: false,
    });

// --- Databricks ---
// Python: apps/backend-python/src/api/datasources/databricks/__init__.py
// credential_fields: ["access_token"]
// sensitive_credential_fields: ["access_token"]
// No sensitive_connection_config_fields (defaults to [])
static DATABRICKS_META: LazyLock<DatasourceTypeMetadata> =
    LazyLock::new(|| DatasourceTypeMetadata {
        type_id: "databricks",
        display_name: "Databricks",
        description: "Databricks SQL warehouse",
        default_port: Some(443),
        credential_fields: &["access_token"],
        sensitive_credential_fields: &["access_token"],
        sensitive_connection_config_fields: &[],
        requires_user_credentials: true,
        accepts_user_context: false,
        auth_modes: leak_auth_modes(vec![
            token_auth_mode(
                true,
                "access_token",
                "Personal Access Token",
                "Use a Databricks personal access token for authentication",
                true,
            ),
            oauth_auth_mode(
                "databricks",
                false,
                Some("Databricks OAuth"),
                Some("Authenticate with your Databricks account via OAuth"),
            ),
        ]),
        catalog_container_label: "catalog",
        catalog_config_keys: &["catalog_catalogs"],
        supports_catalog_discovery: true,
        tree_level1_type: "catalog",
        tree_level2_type: "schema",
        skip_empty_project_wrapper: true,
        skip_single_project_wrapper: false,
        supports_ssh_tunnel: false,
    });

// --- Redshift ---
// Python: apps/backend-python/src/api/datasources/redshift/__init__.py
// credential_fields: ["username", "password"]
// sensitive_credential_fields: ["password"]
// No sensitive_connection_config_fields
static REDSHIFT_META: LazyLock<DatasourceTypeMetadata> =
    LazyLock::new(|| DatasourceTypeMetadata {
        type_id: "redshift",
        display_name: "Amazon Redshift",
        description: "Amazon Redshift data warehouse",
        default_port: Some(5439),
        credential_fields: &["username", "password"],
        sensitive_credential_fields: &["password"],
        sensitive_connection_config_fields: &[],
        requires_user_credentials: true,
        accepts_user_context: false,
        auth_modes: leak_auth_modes(vec![password_auth_mode(true, true)]),
        catalog_container_label: "schema",
        catalog_config_keys: &["catalog_schemas"],
        supports_catalog_discovery: true,
        tree_level1_type: "database",
        tree_level2_type: "schema",
        skip_empty_project_wrapper: true,
        skip_single_project_wrapper: true,
        supports_ssh_tunnel: true,
    });

// --- PostgreSQL ---
// Python: apps/backend-python/src/api/datasources/postgres/__init__.py
// No sensitive_connection_config_fields (defaults to [])
// Note: ssh_private_key is handled by COMMON_SENSITIVE in mask_connection_config.
static POSTGRES_META: LazyLock<DatasourceTypeMetadata> =
    LazyLock::new(|| DatasourceTypeMetadata {
        type_id: "postgres",
        display_name: "PostgreSQL",
        description: "PostgreSQL database",
        default_port: Some(5432),
        credential_fields: &["username", "password"],
        sensitive_credential_fields: &["password"],
        sensitive_connection_config_fields: &[],
        requires_user_credentials: true,
        accepts_user_context: false,
        auth_modes: leak_auth_modes(vec![password_auth_mode(true, true)]),
        catalog_container_label: "schema",
        catalog_config_keys: &["catalog_schemas"],
        supports_catalog_discovery: true,
        tree_level1_type: "database",
        tree_level2_type: "schema",
        skip_empty_project_wrapper: true,
        skip_single_project_wrapper: true,
        supports_ssh_tunnel: true,
    });

// --- MySQL ---
// Python: apps/backend-python/src/api/datasources/mysql/__init__.py
static MYSQL_META: LazyLock<DatasourceTypeMetadata> = LazyLock::new(|| DatasourceTypeMetadata {
    type_id: "mysql",
    display_name: "MySQL",
    description: "MySQL database server",
    default_port: Some(3306),
    credential_fields: &["username", "password"],
    sensitive_credential_fields: &["password"],
    sensitive_connection_config_fields: &[],
    requires_user_credentials: true,
    accepts_user_context: false,
    auth_modes: leak_auth_modes(vec![password_auth_mode(true, true)]),
    catalog_container_label: "database",
    catalog_config_keys: &["catalog_databases"],
    supports_catalog_discovery: true,
    tree_level1_type: "database",
    tree_level2_type: "database",
    skip_empty_project_wrapper: true,
    skip_single_project_wrapper: true,
    supports_ssh_tunnel: true,
});

// --- SQL Server ---
// Python: apps/backend-python/src/api/datasources/sqlserver/__init__.py
// Uses password_auth_mode(is_default=True, supports_shared=True) which gives mode_id="password"
static SQLSERVER_META: LazyLock<DatasourceTypeMetadata> =
    LazyLock::new(|| DatasourceTypeMetadata {
        type_id: "sqlserver",
        display_name: "SQL Server",
        description: "Microsoft SQL Server database",
        default_port: Some(1433),
        credential_fields: &["username", "password"],
        sensitive_credential_fields: &["password"],
        sensitive_connection_config_fields: &[],
        requires_user_credentials: true,
        accepts_user_context: false,
        auth_modes: leak_auth_modes(vec![password_auth_mode(true, true)]),
        catalog_container_label: "schema",
        catalog_config_keys: &["catalog_schemas"],
        supports_catalog_discovery: true,
        tree_level1_type: "database",
        tree_level2_type: "schema",
        skip_empty_project_wrapper: true,
        skip_single_project_wrapper: false,
        supports_ssh_tunnel: true,
    });

// --- Azure Synapse ---
// Python: apps/backend-python/src/api/datasources/synapse/__init__.py
static SYNAPSE_META: LazyLock<DatasourceTypeMetadata> =
    LazyLock::new(|| DatasourceTypeMetadata {
        type_id: "synapse",
        display_name: "Azure Synapse",
        description: "Azure Synapse Analytics (SQL pools)",
        default_port: Some(1433),
        credential_fields: &[
            "auth_type",
            "username",
            "password",
            "client_id",
            "client_secret",
            "tenant_id",
            "oauth_access_token",
            "oauth_refresh_token",
        ],
        sensitive_credential_fields: &[
            "password",
            "client_secret",
            "oauth_access_token",
            "oauth_refresh_token",
        ],
        sensitive_connection_config_fields: &["oauth_client_secret"],
        requires_user_credentials: true,
        accepts_user_context: false,
        auth_modes: leak_auth_modes(vec![
            // SQL Authentication - mode_id="sql" (custom, not from password_auth_mode factory)
            AuthModeConfig {
                mode_id: "sql".into(),
                display_name: "SQL Authentication".into(),
                description: "Authenticate with database username and password".into(),
                credential_type: "password".into(),
                oauth_provider: None,
                oauth_global: false,
                credential_scope: "user".into(),
                preference_tracking: "credential".into(),
                credential_fields: vec!["username".into(), "password".into()],
                sensitive_fields: vec!["password".into()],
                is_default: true,
                supports_shared_credentials: true,
                supports_headless_indexing: true,
            },
            // Service Principal - Azure AD app registration
            AuthModeConfig {
                mode_id: "service_principal".into(),
                display_name: "Service Principal".into(),
                // KYO-274: same downgrade as BigQuery's service_account —
                // "Authenticate using an Azure AD service principal" doesn't
                // say the client ID/secret is one identity used by the whole
                // workspace, not a per-user credential (there is exactly one
                // Client ID/Secret field pair for the datasource; every
                // connecting user shares it). That's the fact worth stating.
                description: "All users share a service principal (app registration) identity"
                    .into(),
                credential_type: "password".into(),
                oauth_provider: None,
                oauth_global: false,
                credential_scope: "user".into(),
                preference_tracking: "credential".into(),
                credential_fields: vec![
                    "tenant_id".into(),
                    "client_id".into(),
                    "client_secret".into(),
                ],
                sensitive_fields: vec!["client_secret".into()],
                is_default: false,
                supports_shared_credentials: true,
                supports_headless_indexing: true,
            },
            // NOTE: a plain (non-enterprise) `oauth_auth_mode("microsoft", ...)`
            // — "Microsoft Account" — lived here previously but was never
            // wired up: no credential-field UI rendered for it, the
            // connection-config builder's synapse match had no arm for it
            // (would have silently mis-persisted a username/password for an
            // OAuth-only mode), and `synapse_oauth_source` never mapped it to
            // an OAuth-status fetch. `kyomi-ui`'s Authentication Mode
            // selector never offered it either — the selector only ever
            // exposed `sql` / `service_principal` / `enterprise_oauth`.
            // Removed as dead (KYO-274); `enterprise_oauth` already covers
            // Microsoft OAuth for Synapse. Do not re-add it speculatively —
            // if a non-enterprise Microsoft OAuth mode is genuinely needed,
            // it needs the UI wiring (field set, credential-config arm,
            // OAuth-source mapping) built alongside it, not just a registry
            // entry.
            // Microsoft Enterprise OAuth
            enterprise_oauth_auth_mode(
                "microsoft",
                false,
                Some("Microsoft OAuth (Enterprise)"),
                Some("Use your organization's Azure AD OAuth configuration"),
            ),
        ]),
        catalog_container_label: "schema",
        catalog_config_keys: &["catalog_schemas"],
        supports_catalog_discovery: true,
        tree_level1_type: "database",
        tree_level2_type: "schema",
        skip_empty_project_wrapper: true,
        skip_single_project_wrapper: false,
        supports_ssh_tunnel: true,
    });

// --- FlareDB ---
static FLAREDB_META: LazyLock<DatasourceTypeMetadata> =
    LazyLock::new(|| DatasourceTypeMetadata {
        type_id: "flaredb",
        display_name: "FlareDB",
        description: "FlareDB analytics database (Arrow Flight SQL)",
        default_port: Some(8815),
        credential_fields: &[],
        sensitive_credential_fields: &[],
        sensitive_connection_config_fields: &[],
        requires_user_credentials: false,
        accepts_user_context: false,
        auth_modes: leak_auth_modes(vec![
            AuthModeConfig {
                mode_id: "none".into(),
                display_name: "No Authentication".into(),
                description: "FlareDB does not require authentication".into(),
                credential_type: "none".into(),
                oauth_provider: None,
                oauth_global: false,
                credential_scope: "workspace".into(),
                preference_tracking: "preference".into(),
                credential_fields: vec![],
                sensitive_fields: vec![],
                is_default: true,
                supports_shared_credentials: false,
                supports_headless_indexing: false,
            },
        ]),
        catalog_container_label: "schema",
        catalog_config_keys: &["catalog_schemas"],
        supports_catalog_discovery: true,
        tree_level1_type: "schema",
        tree_level2_type: "",
        skip_empty_project_wrapper: true,
        skip_single_project_wrapper: true,
        supports_ssh_tunnel: false,
    });

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Look up static metadata for a datasource type.
///
/// Returns a `&'static DatasourceTypeMetadata` reference — zero allocation.
pub fn get_metadata(ds_type: &DatasourceType) -> &'static DatasourceTypeMetadata {
    match ds_type {
        DatasourceType::BigQuery => &BIGQUERY_META,
        DatasourceType::ClickHouse => &CLICKHOUSE_META,
        DatasourceType::Snowflake => &SNOWFLAKE_META,
        DatasourceType::Databricks => &DATABRICKS_META,
        DatasourceType::Redshift => &REDSHIFT_META,
        DatasourceType::Postgres => &POSTGRES_META,
        DatasourceType::MySQL => &MYSQL_META,
        DatasourceType::SqlServer => &SQLSERVER_META,
        DatasourceType::Synapse => &SYNAPSE_META,
        DatasourceType::FlareDb => &FLAREDB_META,
    }
}

/// Look up static metadata by the string form of the type.
///
/// Returns `None` if the type string is not recognised.
pub fn get_metadata_by_str(ds_type: &str) -> Option<&'static DatasourceTypeMetadata> {
    DatasourceType::from_str(ds_type).ok().map(|t| get_metadata(&t))
}

/// Return all registered types with their metadata, for the `/types` endpoint.
///
/// The order matches `ALL_TYPES` (same as Python's `SUPPORTED_DATASOURCE_TYPES`).
pub fn all_metadata() -> Vec<(&'static str, &'static DatasourceTypeMetadata)> {
    ALL_TYPES
        .iter()
        .map(|t| (t.as_str(), get_metadata(t)))
        .collect()
}

/// Check whether a type string is a supported datasource type.
pub fn is_supported_type(ds_type: &str) -> bool {
    DatasourceType::from_str(ds_type).is_ok()
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn all_types_round_trip_from_str() {
        for ds_type in &ALL_TYPES {
            let s = ds_type.as_str();
            let parsed: DatasourceType = s.parse().expect("parse should succeed");
            assert_eq!(&parsed, ds_type);
        }
    }

    #[test]
    fn from_str_unknown_type_is_err() {
        assert!(DatasourceType::from_str("unknown").is_err());
    }

    #[test]
    fn display_matches_as_str() {
        for ds_type in &ALL_TYPES {
            assert_eq!(ds_type.to_string(), ds_type.as_str());
        }
    }

    #[test]
    fn all_metadata_returns_10_types() {
        let all = all_metadata();
        assert_eq!(all.len(), 10);
    }

    #[test]
    fn metadata_type_ids_match() {
        for ds_type in &ALL_TYPES {
            let meta = get_metadata(ds_type);
            assert_eq!(meta.type_id, ds_type.as_str());
        }
    }

    #[test]
    fn metadata_display_names_match() {
        for ds_type in &ALL_TYPES {
            let meta = get_metadata(ds_type);
            assert_eq!(meta.display_name, ds_type.display_name());
        }
    }

    #[test]
    fn metadata_default_ports_match() {
        for ds_type in &ALL_TYPES {
            let meta = get_metadata(ds_type);
            assert_eq!(meta.default_port, ds_type.default_port());
        }
    }

    // --- BigQuery auth modes ---

    #[test]
    fn bigquery_has_three_auth_modes() {
        let meta = get_metadata(&DatasourceType::BigQuery);
        assert_eq!(meta.auth_modes.len(), 3);
        assert_eq!(meta.auth_modes[0].mode_id, "kyomi_oauth");
        assert_eq!(meta.auth_modes[0].credential_type, "oauth_global");
        assert_eq!(meta.auth_modes[0].oauth_provider.as_deref(), Some("google"));
        assert!(meta.auth_modes[0].oauth_global);
        assert!(meta.auth_modes[0].is_default);
        assert_eq!(meta.auth_modes[0].credential_scope, "global_oauth");
        assert_eq!(meta.auth_modes[0].preference_tracking, "preference");

        assert_eq!(meta.auth_modes[1].mode_id, "enterprise_oauth");
        assert_eq!(meta.auth_modes[1].credential_type, "oauth_per_datasource");
        assert!(!meta.auth_modes[1].is_default);

        assert_eq!(meta.auth_modes[2].mode_id, "service_account");
        assert_eq!(meta.auth_modes[2].credential_type, "service_account");
        assert_eq!(meta.auth_modes[2].credential_scope, "workspace");
        assert_eq!(meta.auth_modes[2].preference_tracking, "preference");
    }

    // --- PostgreSQL auth modes ---

    #[test]
    fn postgres_has_password_auth_mode() {
        let meta = get_metadata(&DatasourceType::Postgres);
        assert_eq!(meta.auth_modes.len(), 1);
        assert_eq!(meta.auth_modes[0].mode_id, "password");
        assert_eq!(meta.auth_modes[0].credential_type, "password");
        assert!(meta.auth_modes[0].is_default);
        assert!(meta.auth_modes[0].supports_shared_credentials);
        assert_eq!(meta.auth_modes[0].credential_scope, "user");
        assert_eq!(meta.auth_modes[0].preference_tracking, "credential");
        assert_eq!(meta.auth_modes[0].credential_fields, vec!["username", "password"]);
        assert_eq!(meta.auth_modes[0].sensitive_fields, vec!["password"]);
    }

    // --- Snowflake auth modes ---

    #[test]
    fn snowflake_has_three_auth_modes() {
        let meta = get_metadata(&DatasourceType::Snowflake);
        assert_eq!(meta.auth_modes.len(), 3);
        assert_eq!(meta.auth_modes[0].mode_id, "password");
        assert!(meta.auth_modes[0].is_default);
        assert_eq!(meta.auth_modes[1].mode_id, "oauth");
        assert_eq!(meta.auth_modes[1].credential_type, "oauth_per_datasource");
        assert_eq!(meta.auth_modes[1].oauth_provider.as_deref(), Some("snowflake"));

        // Key-pair auth (KYO-274) — see the long comment on this entry's
        // construction for why it was added and why
        // `supports_headless_indexing` is `true`.
        assert_eq!(meta.auth_modes[2].mode_id, "keypair");
        assert_eq!(meta.auth_modes[2].credential_type, "keypair");
        assert!(!meta.auth_modes[2].is_default);
        assert!(meta.auth_modes[2].supports_shared_credentials);
        assert!(meta.auth_modes[2].supports_headless_indexing);
        assert_eq!(
            meta.auth_modes[2].credential_fields,
            vec!["username", "password", "private_key"]
        );
        assert_eq!(
            meta.auth_modes[2].sensitive_fields,
            vec!["password", "private_key"]
        );
    }

    // --- Synapse auth modes ---

    #[test]
    fn synapse_has_three_auth_modes() {
        let meta = get_metadata(&DatasourceType::Synapse);
        assert_eq!(meta.auth_modes.len(), 3);
        assert_eq!(meta.auth_modes[0].mode_id, "sql");
        assert_eq!(meta.auth_modes[0].display_name, "SQL Authentication");
        assert!(meta.auth_modes[0].is_default);
        assert_eq!(
            meta.auth_modes[0].credential_fields,
            vec!["username", "password"]
        );

        assert_eq!(meta.auth_modes[1].mode_id, "service_principal");
        assert_eq!(meta.auth_modes[1].display_name, "Service Principal");
        assert_eq!(
            meta.auth_modes[1].credential_fields,
            vec!["tenant_id", "client_id", "client_secret"]
        );
        assert_eq!(meta.auth_modes[1].sensitive_fields, vec!["client_secret"]);

        // The plain "Microsoft Account" oauth mode that used to sit here was
        // removed as an unwired dead entry (KYO-274) — see the comment left
        // in its place in the registry.
        assert_eq!(meta.auth_modes[2].mode_id, "enterprise_oauth");
        assert_eq!(
            meta.auth_modes[2].display_name,
            "Microsoft OAuth (Enterprise)"
        );
    }

    // --- Databricks auth modes ---

    #[test]
    fn databricks_has_token_and_oauth() {
        let meta = get_metadata(&DatasourceType::Databricks);
        assert_eq!(meta.auth_modes.len(), 2);
        assert_eq!(meta.auth_modes[0].mode_id, "token");
        assert_eq!(meta.auth_modes[0].credential_type, "token");
        assert_eq!(meta.auth_modes[0].display_name, "Personal Access Token");
        assert!(meta.auth_modes[0].is_default);
        assert_eq!(
            meta.auth_modes[0].credential_fields,
            vec!["access_token"]
        );
        assert_eq!(
            meta.auth_modes[0].sensitive_fields,
            vec!["access_token"]
        );

        assert_eq!(meta.auth_modes[1].mode_id, "oauth");
        assert_eq!(meta.auth_modes[1].credential_type, "oauth_per_datasource");
        assert_eq!(
            meta.auth_modes[1].oauth_provider.as_deref(),
            Some("databricks")
        );
    }

    // --- SQL Server auth modes ---

    #[test]
    fn sqlserver_has_password_auth_mode() {
        let meta = get_metadata(&DatasourceType::SqlServer);
        assert_eq!(meta.auth_modes.len(), 1);
        assert_eq!(meta.auth_modes[0].mode_id, "password");
        assert_eq!(meta.auth_modes[0].credential_type, "password");
        assert!(meta.auth_modes[0].is_default);
    }

    // --- Sensitive fields ---

    #[test]
    fn sensitive_fields_are_correct() {
        // BigQuery — no sensitive credential fields but has sensitive config
        let bq = get_metadata(&DatasourceType::BigQuery);
        assert!(bq.sensitive_credential_fields.is_empty());
        assert!(bq.sensitive_connection_config_fields.contains(&"oauth_client_secret"));
        assert!(bq.sensitive_connection_config_fields.contains(&"service_account_json"));

        // Postgres — password is sensitive, no type-specific sensitive config
        // (ssh_private_key is in COMMON_SENSITIVE in credential_service.rs)
        let pg = get_metadata(&DatasourceType::Postgres);
        assert!(pg.sensitive_credential_fields.contains(&"password"));
        assert!(pg.sensitive_connection_config_fields.is_empty());

        // Redshift — only password is sensitive (matches Python source)
        let rs = get_metadata(&DatasourceType::Redshift);
        assert_eq!(rs.sensitive_credential_fields, &["password"]);

        // Databricks — only access_token is sensitive (matches Python source)
        let db = get_metadata(&DatasourceType::Databricks);
        assert_eq!(db.sensitive_credential_fields, &["access_token"]);
        assert!(db.sensitive_connection_config_fields.is_empty());

        // Snowflake — password (password auth mode) and private_key
        // (key-pair auth mode, KYO-330) are both sensitive; the Python
        // source predates key-pair auth and only had "password".
        let sf = get_metadata(&DatasourceType::Snowflake);
        assert_eq!(sf.sensitive_credential_fields, &["password", "private_key"]);
        assert!(sf.sensitive_connection_config_fields.is_empty());

        // Synapse — multiple sensitive fields
        let sy = get_metadata(&DatasourceType::Synapse);
        assert!(sy.sensitive_credential_fields.contains(&"password"));
        assert!(sy.sensitive_credential_fields.contains(&"client_secret"));
        assert!(sy.sensitive_credential_fields.contains(&"oauth_access_token"));
        assert!(sy.sensitive_credential_fields.contains(&"oauth_refresh_token"));
        assert!(sy.sensitive_connection_config_fields.contains(&"oauth_client_secret"));
    }

    #[test]
    fn get_metadata_by_str_works() {
        let meta = get_metadata_by_str("postgres");
        assert!(meta.is_some());
        assert_eq!(meta.expect("should be Some").type_id, "postgres");

        let none = get_metadata_by_str("unknown");
        assert!(none.is_none());
    }

    #[test]
    fn is_supported_type_works() {
        assert!(is_supported_type("postgres"));
        assert!(is_supported_type("bigquery"));
        assert!(!is_supported_type("oracle"));
        assert!(!is_supported_type(""));
    }

    #[test]
    fn serde_roundtrip() {
        let ds = DatasourceType::BigQuery;
        let json = serde_json::to_string(&ds).expect("serialize should succeed");
        assert_eq!(json, "\"bigquery\"");
        let parsed: DatasourceType =
            serde_json::from_str(&json).expect("deserialize should succeed");
        assert_eq!(parsed, ds);
    }

    #[test]
    fn catalog_config_keys_are_correct() {
        assert_eq!(
            get_metadata(&DatasourceType::BigQuery).catalog_config_keys,
            &["catalog_projects", "include_public_datasets"]
        );
        assert_eq!(
            get_metadata(&DatasourceType::Postgres).catalog_config_keys,
            &["catalog_schemas"]
        );
        assert_eq!(
            get_metadata(&DatasourceType::MySQL).catalog_config_keys,
            &["catalog_databases"]
        );
        assert_eq!(
            get_metadata(&DatasourceType::Databricks).catalog_config_keys,
            &["catalog_catalogs"]
        );
    }

    // --- AuthModeConfig helper methods ---

    #[test]
    fn auth_mode_is_shared_auth() {
        let bq = get_metadata(&DatasourceType::BigQuery);
        // kyomi_oauth has preference_tracking="preference" -> shared
        assert!(bq.auth_modes[0].is_shared_auth());
        // enterprise_oauth has preference_tracking="credential" -> not shared
        assert!(!bq.auth_modes[1].is_shared_auth());
        // service_account has preference_tracking="preference" -> shared
        assert!(bq.auth_modes[2].is_shared_auth());
    }

    #[test]
    fn auth_mode_requires_oauth() {
        let bq = get_metadata(&DatasourceType::BigQuery);
        assert!(bq.auth_modes[0].requires_oauth()); // oauth_global
        assert!(bq.auth_modes[1].requires_oauth()); // oauth_per_datasource
        assert!(!bq.auth_modes[2].requires_oauth()); // service_account

        let pg = get_metadata(&DatasourceType::Postgres);
        assert!(!pg.auth_modes[0].requires_oauth()); // password
    }

    #[test]
    fn auth_mode_requires_user_credentials() {
        let pg = get_metadata(&DatasourceType::Postgres);
        assert!(pg.auth_modes[0].requires_user_credentials()); // password, user scope

        let bq = get_metadata(&DatasourceType::BigQuery);
        assert!(!bq.auth_modes[0].requires_user_credentials()); // oauth_global, global_oauth scope
        assert!(!bq.auth_modes[2].requires_user_credentials()); // service_account, workspace scope
    }

    // --- DatasourceTypeMetadata helper methods ---

    #[test]
    fn get_auth_mode_finds_by_id() {
        let bq = get_metadata(&DatasourceType::BigQuery);
        let mode = bq.get_auth_mode("service_account");
        assert!(mode.is_some());
        assert_eq!(mode.expect("should exist").credential_type, "service_account");

        assert!(bq.get_auth_mode("nonexistent").is_none());
    }

    #[test]
    fn get_default_auth_mode_returns_default() {
        let bq = get_metadata(&DatasourceType::BigQuery);
        let default = bq.get_default_auth_mode();
        assert!(default.is_some());
        assert_eq!(default.expect("should have default").mode_id, "kyomi_oauth");

        let pg = get_metadata(&DatasourceType::Postgres);
        let default = pg.get_default_auth_mode();
        assert!(default.is_some());
        assert_eq!(default.expect("should have default").mode_id, "password");
    }

    #[test]
    fn get_active_auth_mode_uses_config() {
        let bq = get_metadata(&DatasourceType::BigQuery);

        let mut config = HashMap::new();
        config.insert(
            "auth_mode".into(),
            serde_json::Value::String("service_account".into()),
        );
        let active = bq.get_active_auth_mode(&config);
        assert_eq!(active.expect("should find mode").mode_id, "service_account");

        // Falls back to default when auth_mode not specified
        let empty_config = HashMap::new();
        let active = bq.get_active_auth_mode(&empty_config);
        assert_eq!(active.expect("should fall back").mode_id, "kyomi_oauth");
    }

    #[test]
    fn is_shared_auth_checks_config() {
        let bq = get_metadata(&DatasourceType::BigQuery);

        // service_account is shared
        let mut config = HashMap::new();
        config.insert(
            "auth_mode".into(),
            serde_json::Value::String("service_account".into()),
        );
        assert!(bq.is_shared_auth(&config));

        // enterprise_oauth is not shared
        config.insert(
            "auth_mode".into(),
            serde_json::Value::String("enterprise_oauth".into()),
        );
        assert!(!bq.is_shared_auth(&config));

        // explicit shared_credentials flag overrides
        config.insert(
            "shared_credentials".into(),
            serde_json::Value::Bool(true),
        );
        assert!(bq.is_shared_auth(&config));
    }

    #[test]
    fn supports_ssh_tunnel_is_correct() {
        const SSH_TUNNEL_SUPPORTED: &[&str] =
            &["postgres", "mysql", "redshift", "clickhouse", "sqlserver", "synapse"];

        for (type_id, meta) in all_metadata() {
            let expected = SSH_TUNNEL_SUPPORTED.contains(&type_id);
            assert_eq!(
                meta.supports_ssh_tunnel, expected,
                "supports_ssh_tunnel mismatch for {type_id}"
            );
        }
    }

    // --- Headless indexing auth modes (KYO-187) ---

    #[test]
    fn indexing_auth_modes_match_legacy_client_hardcoded_table() {
        // Mirrors the table that `get_indexing_auth_modes` in
        // `kyomi-ui/src/pages/settings/datasources.rs` used to hardcode before
        // KYO-187 moved this knowledge into the registry. This is the whole
        // safety property of that migration: the derived list must exactly
        // equal what the old client-side `match` produced, including
        // flaredb's empty list (its only auth mode, `none`, has no
        // credentials and cannot back a headless indexing run).
        //
        // Snowflake's row below is the one deliberate exception, and it is
        // an intended fix, not an accommodation: KYO-274 added the `keypair`
        // auth mode to the registry with `supports_headless_indexing: true`.
        // Before that, `keypair` did not exist in the registry at all, so it
        // could never appear here — a workspace using Snowflake key-pair
        // auth had no way to select key-pair credentials for catalog
        // indexing, even though key-pair is exactly the kind of credential
        // that can authenticate a headless background job. This test
        // failing on that line (before the table below was updated) is what
        // proved the gap was real; the updated expectation is the shipped
        // fix.
        let expected: &[(&str, &[(&str, &str)])] = &[
            ("bigquery", &[("service_account", "Service Account")]),
            ("clickhouse", &[("password", "Password")]),
            (
                "snowflake",
                &[("password", "Password"), ("keypair", "Key Pair")],
            ),
            ("databricks", &[("token", "Personal Access Token")]),
            ("redshift", &[("password", "Password")]),
            ("postgres", &[("password", "Password")]),
            ("mysql", &[("password", "Password")]),
            ("sqlserver", &[("password", "Password")]),
            (
                "synapse",
                &[
                    ("sql", "SQL Authentication"),
                    ("service_principal", "Service Principal"),
                ],
            ),
            ("flaredb", &[]),
        ];

        assert_eq!(
            expected.len(),
            ALL_TYPES.len(),
            "test table must cover every registered type"
        );

        for (type_id, expected_modes) in expected {
            let meta = get_metadata_by_str(type_id)
                .unwrap_or_else(|| panic!("unknown datasource type {type_id}"));
            let actual: Vec<(&str, &str)> = meta
                .indexing_auth_modes()
                .map(|m| (m.mode_id.as_str(), m.display_name.as_str()))
                .collect();
            assert_eq!(
                &actual, expected_modes,
                "indexing auth modes mismatch for {type_id}"
            );
        }
    }

    #[test]
    fn indexing_auth_modes_excludes_oauth_by_construction() {
        // The filter is `supports_headless_indexing`, not an allowlist of
        // known-safe mode_ids -- so a future OAuth mode added to any type
        // (e.g. a new enterprise_oauth variant) can never silently appear in
        // the indexing selector just by existing in `auth_modes`.
        for (type_id, meta) in all_metadata() {
            for mode in meta.indexing_auth_modes() {
                assert!(
                    !mode.requires_oauth(),
                    "{type_id}'s indexing_auth_modes yielded an OAuth mode: {:?}",
                    mode.mode_id
                );
            }
        }
    }

    #[test]
    fn get_auth_mode_ids_returns_all() {
        let bq = get_metadata(&DatasourceType::BigQuery);
        let ids = bq.get_auth_mode_ids();
        assert_eq!(ids, vec!["kyomi_oauth", "enterprise_oauth", "service_account"]);

        let sy = get_metadata(&DatasourceType::Synapse);
        let ids = sy.get_auth_mode_ids();
        assert_eq!(ids, vec!["sql", "service_principal", "enterprise_oauth"]);
    }

    // --- Connection auth-mode ids (KYO-274) ---

    /// After KYO-274, `kyomi-ui`'s four `*AuthModeSection` components
    /// (BigQuery, Snowflake, Databricks, Synapse) no longer hardcode auth
    /// mode ids/labels/descriptions — they render whatever
    /// `DatasourceTypeInfo::connection_auth_modes` returns, which
    /// `get_datasource_types()` populates directly from `meta.auth_modes`
    /// for the type (see `kyomi-ui/src/server_fns/datasources.rs`). That
    /// means the UI's mode list is, by construction, always exactly this
    /// registry's `auth_modes` — there is no longer a second copy that can
    /// drift out of sync the way the BigQuery label text did before this
    /// ticket.
    ///
    /// This test pins today's known-good id list per provider as a
    /// regression guard: a future edit to any of these four types'
    /// `auth_modes` (add, remove, or reorder a mode) is still a real,
    /// user-visible change to what admins can select — this test forces
    /// that change to be a deliberate, reviewed edit here rather than a
    /// silent drift discovered later in the UI.
    #[test]
    fn connection_auth_mode_ids_match_expected_table() {
        let expected: &[(&str, &[&str])] = &[
            ("bigquery", &["kyomi_oauth", "enterprise_oauth", "service_account"]),
            ("snowflake", &["password", "oauth", "keypair"]),
            ("databricks", &["token", "oauth"]),
            ("synapse", &["sql", "service_principal", "enterprise_oauth"]),
        ];

        for (type_id, expected_ids) in expected {
            let meta = get_metadata_by_str(type_id)
                .unwrap_or_else(|| panic!("unknown datasource type {type_id}"));
            let actual: Vec<&str> = meta.auth_modes.iter().map(|m| m.mode_id.as_str()).collect();
            assert_eq!(
                &actual, expected_ids,
                "connection auth mode ids mismatch for {type_id}"
            );
        }
    }
}
