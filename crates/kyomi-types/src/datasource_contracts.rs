// SPDX-License-Identifier: AGPL-3.0-or-later

//! Datasource/OAuth wire contracts shared between `kyomi_auth` (ssr-only) and
//! `kyomi_ui` (compiles to wasm32).
//!
//! `kyomi-auth` re-exports these from `ssh_keygen`, `google_oauth`, and
//! `datasource_oauth` so existing server-side call sites keep working
//! unchanged; `kyomi-ui`'s server_fns modules re-export them too so client
//! call sites keep working unchanged. Defining them once here means the
//! server and the WASM client can never fork the wire format.

use serde::{Deserialize, Serialize};

/// A freshly generated SSH keypair for a datasource's SSH tunnel.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct GeneratedSshKey {
    /// OpenSSH public key line (`ssh-ed25519 AAAA... `), plaintext.
    ///
    /// Shown to the user so they can add it to their server's
    /// `~/.ssh/authorized_keys`.
    pub public_key: String,

    /// OpenSSH private key PEM, **plaintext**. The caller is responsible for
    /// encrypting it before persisting it as part of a datasource's
    /// `connection_config` (handled by
    /// `kyomi_auth::credential_service::finalize_connection_config_secrets`
    /// on the save path).
    ///
    /// The client holds this in memory only long enough to submit it back as
    /// `connection_config.ssh_private_key` on save — `create_datasource` /
    /// `update_datasource_settings` encrypt it with the workspace encryption
    /// key before it is ever written to the database (see
    /// `credential_service::finalize_connection_config_secrets`).
    pub private_key: String,
}

/// A single Google Cloud project.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct GoogleProject {
    pub project_id: String,
    pub name: String,
}

/// Result of `kyomi_auth::google_oauth::google_oauth_projects_service`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct GoogleOAuthProjectsResult {
    pub projects: Vec<GoogleProject>,
    pub message: Option<String>,
}

/// Result of `kyomi_auth::google_oauth::google_oauth_disconnect_service`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct GoogleOAuthDisconnectResult {
    pub success: bool,
    pub already_disconnected: bool,
    pub disconnected_email: Option<String>,
}

/// Result of `kyomi_auth::datasource_oauth::datasource_oauth_disconnect_service`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct DatasourceOAuthDisconnectResult {
    pub success: bool,
    pub already_disconnected: bool,
}
