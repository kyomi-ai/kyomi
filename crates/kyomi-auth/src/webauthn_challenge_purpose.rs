// SPDX-License-Identifier: AGPL-3.0-or-later

//! Purpose binding for WebAuthn challenge blobs (KYO-279).
//!
//! Every WebAuthn challenge — login, signup, recovery, or authenticated
//! device-add — is minted by a different flow but stored under the *same*
//! shared KV namespace (`redis_ops::store_webauthn_challenge`), keyed only
//! by a random `challenge_id` with no type discriminator. A completion
//! endpoint that trusts a field out of the blob (e.g. `user_id`) without
//! checking *why* the challenge was minted can be handed a challenge minted
//! by a different, less-trusted flow — e.g. an unauthenticated registration
//! challenge completed through the authenticated add-device path, or vice
//! versa.
//!
//! Every mint site MUST set a `purpose` field to exactly one of the
//! constants below. Every consumer MUST call [`has_purpose`] immediately
//! after fetching the challenge and reject — via its own existing "invalid
//! or expired challenge" error path, so a purpose mismatch is indistinguishable
//! from a missing challenge — on a missing, unrecognised, or wrong purpose.
//! This is fail-closed by construction: a future mint site that forgets to
//! set `purpose` produces a blob no consumer will ever accept.

/// `purpose` for a passkey login (authentication) challenge.
///
/// Minted by `kyomi-ui/src/server_fns/auth.rs::passkey_login_start`.
/// Consumed by `auth_service::passkey_login_complete_service`.
pub const PASSKEY_LOGIN: &str = "passkey_login";

/// `purpose` for a passkey registration challenge minted as part of the
/// email-verification-token-gated signup flow.
///
/// Minted by `auth_service::passkey_signup_complete_service`.
/// Consumed by `auth_service::passkey_register_complete_service`.
pub const PASSKEY_SIGNUP: &str = "passkey_signup";

/// `purpose` for a passkey registration challenge minted as part of the
/// recovery-token-gated account recovery flow.
///
/// Minted by `auth_service::passkey_recovery_verify_service`.
/// Consumed by `auth_service::passkey_recovery_complete_service` (KYO-284
/// split this off `passkey_register_complete_service`, which now rejects
/// this purpose — recovery completion additionally requires the HttpOnly
/// `recovery_session` cookie minted alongside this challenge).
pub const PASSKEY_RECOVERY: &str = "passkey_recovery";

/// `purpose` for a passkey registration challenge minted on behalf of an
/// already-authenticated user adding a new device.
///
/// Minted by `security_service::start_passkey_registration`.
/// Consumed by `security_service::complete_passkey_registration`.
pub const PASSKEY_ADD_DEVICE: &str = "passkey_add_device";

/// Read-and-validate the `purpose` field of a stored challenge blob.
///
/// Returns `true` only if `purpose` is present *and* its value is one of
/// `allowed`. Returns `false` for a missing field or any value not in
/// `allowed` — including values from this very module that the caller
/// simply didn't list, and unrecognised strings from neither this nor any
/// past version of this module (fail closed).
///
/// Callers must map `false` to the same rejection they already use for "no
/// challenge found at this id" — do not surface a distinct message, or the
/// error response becomes an oracle for probing which purpose a given
/// `challenge_id` was minted with.
pub fn has_purpose(challenge_data: &serde_json::Value, allowed: &[&str]) -> bool {
    challenge_data["purpose"]
        .as_str()
        .is_some_and(|purpose| allowed.contains(&purpose))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_its_own_listed_purpose() {
        let data = serde_json::json!({"purpose": PASSKEY_LOGIN});
        assert!(has_purpose(&data, &[PASSKEY_LOGIN]));
    }

    #[test]
    fn accepts_any_purpose_in_a_multi_value_allowlist() {
        let data = serde_json::json!({"purpose": PASSKEY_RECOVERY});
        assert!(has_purpose(&data, &[PASSKEY_SIGNUP, PASSKEY_RECOVERY]));
    }

    #[test]
    fn rejects_a_purpose_minted_for_a_different_flow() {
        // Cross-flow confusion in every direction: each real purpose value,
        // checked against every allowlist that does not contain it.
        let all = [
            PASSKEY_LOGIN,
            PASSKEY_SIGNUP,
            PASSKEY_RECOVERY,
            PASSKEY_ADD_DEVICE,
        ];
        for minted in all {
            for allowed in all {
                if minted == allowed {
                    continue;
                }
                let data = serde_json::json!({"purpose": minted});
                assert!(
                    !has_purpose(&data, &[allowed]),
                    "purpose {minted:?} must not satisfy allowlist [{allowed:?}]"
                );
            }
        }
    }

    #[test]
    fn rejects_missing_purpose_field() {
        let data = serde_json::json!({"user_id": "u1"});
        assert!(!has_purpose(&data, &[PASSKEY_LOGIN]));
    }

    #[test]
    fn rejects_unrecognised_purpose_value() {
        let data = serde_json::json!({"purpose": "totally_made_up"});
        assert!(!has_purpose(
            &data,
            &[
                PASSKEY_LOGIN,
                PASSKEY_SIGNUP,
                PASSKEY_RECOVERY,
                PASSKEY_ADD_DEVICE
            ]
        ));
    }

    #[test]
    fn rejects_null_purpose_field() {
        let data = serde_json::json!({"purpose": null});
        assert!(!has_purpose(&data, &[PASSKEY_LOGIN]));
    }
}
