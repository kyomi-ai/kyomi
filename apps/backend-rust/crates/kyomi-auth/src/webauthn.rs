// SPDX-License-Identifier: AGPL-3.0-or-later

//! WebAuthn (passkey) configuration and operations.
//!
//! Wraps `webauthn-rs` v0.5.x for passkey registration and authentication.
//! Wire-compatible with Python's `py_webauthn`-based implementation.

use url::Url;
use webauthn_rs::prelude::*;
use webauthn_rs::Webauthn;
use webauthn_rs_proto::ResidentKeyRequirement;

/// Build a `Webauthn` instance from config.
///
/// This is called once at startup and stored in `AppState`.
pub fn build_webauthn(rp_id: &str, rp_name: &str, rp_origin: &Url) -> kyomi_core::Result<Webauthn> {
    let builder = WebauthnBuilder::new(rp_id, rp_origin)
        .map_err(|e| kyomi_core::Error::Internal(format!("WebAuthn builder error: {e}")))?
        .rp_name(rp_name);

    builder
        .build()
        .map_err(|e| kyomi_core::Error::Internal(format!("WebAuthn build error: {e}")))
}

/// Start passkey registration for a user.
///
/// Returns (creation challenge JSON, PasskeyRegistration state to store in Redis).
///
/// Forces `residentKey: "required"` so the browser creates a discoverable credential
/// that appears in the passkey picker during sign-in.
pub fn start_registration(
    webauthn: &Webauthn,
    user_unique_id: Uuid,
    user_name: &str,
    user_display_name: &str,
    exclude_credentials: Option<Vec<CredentialID>>,
) -> kyomi_core::Result<(CreationChallengeResponse, PasskeyRegistration)> {
    let (mut ccr, reg_state) = webauthn
        .start_passkey_registration(
            user_unique_id,
            user_name,
            user_display_name,
            exclude_credentials,
        )
        .map_err(|e| kyomi_core::Error::Internal(format!("WebAuthn registration start: {e}")))?;

    // Override: force discoverable credential so passkeys appear in browser picker.
    // webauthn-rs defaults to require_resident_key(false), but we need true for
    // passkeys to show up in the browser's credential selector during sign-in.
    if let Some(ref mut auth_sel) = ccr.public_key.authenticator_selection {
        auth_sel.resident_key = Some(ResidentKeyRequirement::Required);
        auth_sel.require_resident_key = true;
    }

    Ok((ccr, reg_state))
}

/// Complete passkey registration — verify the credential.
///
/// Returns the verified `Passkey` to store in the database.
pub fn finish_registration(
    webauthn: &Webauthn,
    credential: &RegisterPublicKeyCredential,
    registration_state: &PasskeyRegistration,
) -> kyomi_core::Result<Passkey> {
    webauthn
        .finish_passkey_registration(credential, registration_state)
        .map_err(|e| kyomi_core::Error::BadRequest(format!("WebAuthn registration failed: {e}")))
}

/// Start passkey authentication for a user.
///
/// Returns (request challenge JSON, PasskeyAuthentication state to store in Redis).
pub fn start_authentication(
    webauthn: &Webauthn,
    credentials: &[Passkey],
) -> kyomi_core::Result<(RequestChallengeResponse, PasskeyAuthentication)> {
    webauthn
        .start_passkey_authentication(credentials)
        .map_err(|e| kyomi_core::Error::Internal(format!("WebAuthn authentication start: {e}")))
}

/// Complete passkey authentication — verify the assertion.
///
/// Returns the `AuthenticationResult` (contains updated sign count, credential ID, etc.).
pub fn finish_authentication(
    webauthn: &Webauthn,
    credential: &PublicKeyCredential,
    authentication_state: &PasskeyAuthentication,
) -> kyomi_core::Result<AuthenticationResult> {
    webauthn
        .finish_passkey_authentication(credential, authentication_state)
        .map_err(|e| kyomi_core::Error::BadRequest(format!("WebAuthn authentication failed: {e}")))
}

/// Start discoverable (conditional-ui) authentication.
///
/// Used when the user has no known credentials at login_start time.
/// Returns (request challenge JSON, DiscoverableAuthentication state to store in Redis).
/// The response has empty `allowCredentials` so the browser uses resident keys.
pub fn start_discoverable_authentication(
    webauthn: &Webauthn,
) -> kyomi_core::Result<(RequestChallengeResponse, DiscoverableAuthentication)> {
    webauthn
        .start_discoverable_authentication()
        .map_err(|e| kyomi_core::Error::Internal(format!("WebAuthn discoverable auth start: {e}")))
}

/// Complete discoverable authentication — verify the assertion with the identified user's credentials.
///
/// Injects the real credentials into the auth state (which was created without them)
/// and verifies the signature against the challenge that was originally sent.
pub fn finish_discoverable_authentication(
    webauthn: &Webauthn,
    credential: &PublicKeyCredential,
    authentication_state: DiscoverableAuthentication,
    passkeys: &[Passkey],
) -> kyomi_core::Result<AuthenticationResult> {
    let discoverable_keys: Vec<DiscoverableKey> =
        passkeys.iter().map(DiscoverableKey::from).collect();
    webauthn
        .finish_discoverable_authentication(credential, authentication_state, &discoverable_keys)
        .map_err(|e| kyomi_core::Error::BadRequest(format!("WebAuthn discoverable auth failed: {e}")))
}
