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

/// Result of `kyomi_auth::google_oauth::google_oauth_status_service`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct GoogleOAuthStatus {
    pub connected: bool,
    pub google_email: Option<String>,
    pub has_bigquery_scopes: bool,
    pub needs_bigquery_connect: bool,
    pub token_expired: bool,
    pub has_refresh_token: bool,
}

/// Result of `kyomi_auth::datasource_oauth::datasource_oauth_status_service`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct DatasourceOAuthStatus {
    pub connected: bool,
    pub provider_email: Option<String>,
    pub token_expired: bool,
    pub needs_reconnect: bool,
    pub connect_url: String,
    pub disconnect_url: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    /// These two types cross the client/server boundary, so their serialized
    /// shape *is* the wire contract. KYO-222 relocated them here from two
    /// separate declarations that had drifted apart in name; these tests make
    /// the field set self-checking, so a future rename or removal fails here
    /// rather than silently leaving the client unable to read a field.
    ///
    /// Asserting the whole JSON object (not just presence) is deliberate: it
    /// catches an *added* field too, which is the direction a hand-written
    /// conversion would previously have missed.
    #[test]
    fn google_oauth_status_wire_shape_is_stable() {
        let value = serde_json::to_value(GoogleOAuthStatus {
            connected: true,
            google_email: Some("user@example.com".to_string()),
            has_bigquery_scopes: false,
            needs_bigquery_connect: true,
            token_expired: false,
            has_refresh_token: true,
        })
        .expect("GoogleOAuthStatus must serialize");

        assert_eq!(
            value,
            serde_json::json!({
                "connected": true,
                "google_email": "user@example.com",
                "has_bigquery_scopes": false,
                "needs_bigquery_connect": true,
                "token_expired": false,
                "has_refresh_token": true,
            })
        );
    }

    /// A `None` email must still be present as `null` rather than omitted —
    /// the client distinguishes "connected but no email" from a missing key.
    #[test]
    fn google_oauth_status_absent_email_serializes_as_null() {
        let value = serde_json::to_value(GoogleOAuthStatus {
            connected: false,
            google_email: None,
            has_bigquery_scopes: false,
            needs_bigquery_connect: false,
            token_expired: false,
            has_refresh_token: false,
        })
        .expect("GoogleOAuthStatus must serialize");

        assert_eq!(value["google_email"], serde_json::Value::Null);
    }

    #[test]
    fn datasource_oauth_status_wire_shape_is_stable() {
        let value = serde_json::to_value(DatasourceOAuthStatus {
            connected: true,
            provider_email: Some("user@example.com".to_string()),
            token_expired: false,
            needs_reconnect: true,
            connect_url: "/oauth/connect".to_string(),
            disconnect_url: "/oauth/disconnect".to_string(),
        })
        .expect("DatasourceOAuthStatus must serialize");

        assert_eq!(
            value,
            serde_json::json!({
                "connected": true,
                "provider_email": "user@example.com",
                "token_expired": false,
                "needs_reconnect": true,
                "connect_url": "/oauth/connect",
                "disconnect_url": "/oauth/disconnect",
            })
        );
    }

    /// Round-trip guards the `Deserialize` side: the client parses what the
    /// server produced, so a field the server writes but the client cannot
    /// read would fail here.
    #[test]
    fn both_status_types_round_trip() {
        let google = GoogleOAuthStatus {
            connected: true,
            google_email: None,
            has_bigquery_scopes: true,
            needs_bigquery_connect: false,
            token_expired: true,
            has_refresh_token: false,
        };
        let json = serde_json::to_string(&google).expect("serialize");
        let back: GoogleOAuthStatus = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(back, google);

        let datasource = DatasourceOAuthStatus {
            connected: false,
            provider_email: Some("a@b.c".to_string()),
            token_expired: false,
            needs_reconnect: false,
            connect_url: String::new(),
            disconnect_url: String::new(),
        };
        let json = serde_json::to_string(&datasource).expect("serialize");
        let back: DatasourceOAuthStatus = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(back, datasource);
    }
}
