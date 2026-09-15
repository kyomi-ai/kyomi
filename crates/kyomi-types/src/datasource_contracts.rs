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

/// What happened to the OAuth grant **at the provider** when a datasource
/// credential was disconnected (KYO-714).
///
/// Deleting Kyomi's stored credential and revoking the grant at the provider
/// are two different things, and only one provider Kyomi supports exposes a
/// revocation endpoint it can call. Reporting them as one ("Account
/// disconnected") told users their access had been revoked when for three of
/// four providers it had not. This enum is what lets the caller — the
/// settings page toast, and the REST response body — say only what actually
/// happened.
///
/// Lives here rather than in `kyomi_auth` because it is part of a server_fn
/// response and so must serialize into the WASM client.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DatasourceOAuthRevocationOutcome {
    /// The provider confirmed the grant was live and has now revoked it.
    /// Kyomi's stored credential is deleted *and* the provider-side access is
    /// gone — nothing further for the user to do.
    Revoked,

    /// The provider reported the token as already invalid (Google answers
    /// HTTP 400 for this). The grant was already gone before Kyomi asked, so
    /// this is the desired end state and counts as a success, not a failure.
    AlreadyInvalid,

    /// The provider exposes no revocation mechanism Kyomi can call from an
    /// OAuth-authenticated session, so only the locally stored credential was
    /// deleted. **The grant is still live at the provider** and the user must
    /// remove Kyomi's access from that provider's own account settings to
    /// fully revoke it. See
    /// `kyomi_auth::datasource_oauth::OAuthProvider::revocation_capability`
    /// for the per-provider evidence.
    NotSupported,

    /// There was no stored token to revoke — either no credential row existed
    /// at all (in which case `already_disconnected` is also `true`), or the
    /// row held no `oauth_access_token`/`oauth_refresh_token`. Nothing was
    /// sent to the provider.
    NoStoredToken,
}

impl DatasourceOAuthRevocationOutcome {
    /// Whether Kyomi is left holding nothing that represents live access at
    /// the provider — i.e. whether plain "disconnected" wording is honest.
    ///
    /// This is the single place that decides it. Both user-facing surfaces
    /// derive their wording from this one predicate — the settings-page toast
    /// (`kyomi_ui::pages::settings::datasources::datasource_disconnect_message`)
    /// and the `POST /api/v1/auth/oauth/{provider}/disconnect` response
    /// `message` — so the two cannot drift into disagreeing about whether
    /// access was actually revoked, which is the defect KYO-714 exists to fix.
    ///
    /// [`Self::NoStoredToken`] counts as cleared: Kyomi held no token, so
    /// there is nothing it could revoke and nothing it retains. The dominant
    /// case is a disconnect of a datasource that had no credential row at
    /// all, where warning about a live grant would be nonsense.
    ///
    /// Exhaustive with no wildcard arm on purpose: a variant added later must
    /// be classified here before this compiles.
    pub fn grant_cleared_at_provider(self) -> bool {
        match self {
            Self::Revoked | Self::AlreadyInvalid | Self::NoStoredToken => true,
            Self::NotSupported => false,
        }
    }
}

/// Result of `kyomi_auth::datasource_oauth::datasource_oauth_disconnect_service`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct DatasourceOAuthDisconnectResult {
    pub success: bool,
    pub already_disconnected: bool,

    /// What happened to the grant at the provider (KYO-714). `success: true`
    /// only ever means "Kyomi's stored credential is gone"; this field is the
    /// only thing that says whether the provider-side grant went with it, so
    /// callers must key any "revoked" wording off this rather than off
    /// `success`.
    pub revocation: DatasourceOAuthRevocationOutcome,
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

    /// `grant_cleared_at_provider` is what both user-facing surfaces key
    /// their wording off, so every variant is enumerated here rather than
    /// spot-checked: a new variant that defaulted to "cleared" would quietly
    /// reintroduce the over-claim KYO-714 removed.
    #[test]
    fn only_not_supported_leaves_the_grant_live_at_the_provider() {
        use DatasourceOAuthRevocationOutcome as Outcome;

        for outcome in [Outcome::Revoked, Outcome::AlreadyInvalid, Outcome::NoStoredToken] {
            assert!(
                outcome.grant_cleared_at_provider(),
                "{outcome:?} leaves nothing live at the provider, so plain \
                 \"disconnected\" wording is honest for it"
            );
        }

        assert!(
            !Outcome::NotSupported.grant_cleared_at_provider(),
            "NotSupported means the grant is still live at the provider — the \
             one outcome that must never be described as a disconnect"
        );
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

    /// The disconnect result gained a `revocation` field in KYO-714, and the
    /// WASM settings page keys its toast wording off it. Asserting the whole
    /// object pins both the new key's name and the enum's serialized
    /// spelling — a `#[serde(rename_all)]` change or a variant rename would
    /// otherwise silently degrade the client to its `_ =>` fallback wording.
    #[test]
    fn datasource_oauth_disconnect_wire_shape_is_stable() {
        let value = serde_json::to_value(DatasourceOAuthDisconnectResult {
            success: true,
            already_disconnected: false,
            revocation: DatasourceOAuthRevocationOutcome::Revoked,
        })
        .expect("DatasourceOAuthDisconnectResult must serialize");

        assert_eq!(
            value,
            serde_json::json!({
                "success": true,
                "already_disconnected": false,
                "revocation": "revoked",
            })
        );
    }

    /// Every variant's spelling is part of the wire contract, not just the
    /// one the happy path produces.
    #[test]
    fn revocation_outcome_variants_serialize_as_snake_case() {
        let cases = [
            (DatasourceOAuthRevocationOutcome::Revoked, "revoked"),
            (
                DatasourceOAuthRevocationOutcome::AlreadyInvalid,
                "already_invalid",
            ),
            (DatasourceOAuthRevocationOutcome::NotSupported, "not_supported"),
            (
                DatasourceOAuthRevocationOutcome::NoStoredToken,
                "no_stored_token",
            ),
        ];

        for (variant, expected) in cases {
            let value = serde_json::to_value(variant).expect("variant must serialize");
            assert_eq!(value, serde_json::Value::String(expected.to_string()));

            let back: DatasourceOAuthRevocationOutcome =
                serde_json::from_value(value).expect("variant must round-trip");
            assert_eq!(back, variant);
        }
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
