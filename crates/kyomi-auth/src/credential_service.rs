// SPDX-License-Identifier: AGPL-3.0-or-later

//! Credential masking, encryption-at-rest, and decryption helpers.
//!
//! Masking functions use the datasource type registry to determine which
//! fields are sensitive and should be replaced with `MASKED_VALUE`.
//!
//! [`COMMON_SENSITIVE`] fields are additionally encrypted at rest in
//! `connection_config` (see [`finalize_connection_config_secrets`]) and must
//! be decrypted before use by a datasource driver (see
//! [`decrypt_connection_config_secrets`]).
//!
//! For encrypting/decrypting arbitrary credential JSON (e.g. per-user
//! `user_datasource_credentials.credentials`) use `encryption::encrypt_json` /
//! `encryption::decrypt_json` directly.

use base64::engine::general_purpose::URL_SAFE;
use base64::Engine;
use kyomi_core::datasource_registry;
use serde_json::Value;

/// The placeholder string that replaces sensitive fields in API responses.
pub const MASKED_VALUE: &str = "********";

/// Connection config fields that are always sensitive, regardless of
/// datasource type. Masked on read by [`mask_connection_config`], encrypted
/// at rest on write by [`finalize_connection_config_secrets`], and decrypted
/// just-in-time before a datasource provider is built by
/// [`decrypt_connection_config_secrets`].
pub(crate) const COMMON_SENSITIVE: &[&str] =
    &["shared_password", "ssh_private_key", "ssh_passphrase"];

/// Mask sensitive credential fields for API responses.
///
/// Looks up the datasource type in the registry to determine which credential
/// fields are sensitive, then replaces their values with [`MASKED_VALUE`].
///
/// Non-sensitive fields are preserved as-is. If the type is unknown or the
/// credentials are not an object, the value is returned unchanged.
pub fn mask_credentials(credentials: &Value, ds_type: &str) -> Value {
    let Some(obj) = credentials.as_object() else {
        return credentials.clone();
    };

    let sensitive_fields: &[&str] = datasource_registry::get_metadata_by_str(ds_type)
        .map(|m| m.sensitive_credential_fields)
        .unwrap_or(&[]);

    let mut masked = obj.clone();
    for &field in sensitive_fields {
        mask_field_if_present(&mut masked, field);
    }

    Value::Object(masked)
}

/// Mask sensitive connection config fields for API responses.
///
/// Looks up the datasource type in the registry for type-specific sensitive
/// fields. Also always masks every [`COMMON_SENSITIVE`] field (currently
/// `shared_password`, `ssh_private_key`, `ssh_passphrase`) regardless of
/// type, as these are common sensitive fields across all datasource types.
///
/// If the type is unknown or the config is not an object, the value is returned
/// unchanged.
pub fn mask_connection_config(config: &Value, ds_type: &str) -> Value {
    let Some(obj) = config.as_object() else {
        return config.clone();
    };

    let type_specific_fields: &[&str] =
        datasource_registry::get_metadata_by_str(ds_type)
            .map(|m| m.sensitive_connection_config_fields)
            .unwrap_or(&[]);

    let mut masked = obj.clone();

    // Mask type-specific sensitive fields
    for &field in type_specific_fields {
        mask_field_if_present(&mut masked, field);
    }

    // Mask common sensitive fields
    for &field in COMMON_SENSITIVE {
        mask_field_if_present(&mut masked, field);
    }

    // Mask indexing_credentials if present as a non-null/non-empty value.
    // It's stored as an encrypted JSON string, but may arrive as an object
    // (after decryption) or as a non-empty string (encrypted blob).
    if let Some(val) = masked.get("indexing_credentials") {
        let should_mask = match val {
            Value::Object(_) => true,
            Value::String(s) => !s.is_empty(),
            _ => false,
        };
        if should_mask {
            masked.insert(
                "indexing_credentials".to_string(),
                Value::String(MASKED_VALUE.into()),
            );
        }
    }

    Value::Object(masked)
}

/// Restore masked/omitted sensitive `connection_config` fields from the
/// stored config, and encrypt any freshly-provided plaintext secret, before
/// a `connection_config` is written to the database.
///
/// This is the write-side counterpart to [`mask_connection_config`]. Sensitive
/// fields are never sent to the client in real form — they come back either
/// masked as [`MASKED_VALUE`] or omitted entirely (e.g. a UI that only
/// resubmits fields it actually loaded). Without this step, a wholesale
/// replace of `connection_config` on update would clobber the real stored
/// secret with the placeholder or silently drop it — and a plaintext value
/// typed by the user would be written to the database unencrypted.
///
/// For each field in [`COMMON_SENSITIVE`], one of three things happens:
///
/// - `incoming[field]` is explicit JSON `null` — this is an **explicit clear**
///   (e.g. disabling an SSH tunnel drops its stored key). The field is
///   *removed* from `incoming` entirely; it is never restored from `existing`.
/// - `incoming[field]` is missing, or equal to [`MASKED_VALUE`] — this is the
///   normal edit case where the UI never resupplies a secret it only ever
///   received masked. `incoming[field]` is overwritten with the *already
///   encrypted* value from `existing` verbatim — it is never re-encrypted.
///   If `existing` is `None` (create) or has no stored value, the field is
///   left absent.
/// - `incoming[field]` holds a real, non-empty string — this is fresh
///   plaintext supplied by the client. It is encrypted with `key` before
///   being written into `incoming`. Any other real (non-string or empty
///   string) value passes through unchanged.
///
/// No-ops if `incoming` is not a JSON object.
pub fn finalize_connection_config_secrets(
    incoming: &mut Value,
    existing: Option<&Value>,
    key: &[u8; 32],
) -> kyomi_core::Result<()> {
    let existing_obj = existing.and_then(Value::as_object);
    let Some(incoming_obj) = incoming.as_object_mut() else {
        return Ok(());
    };

    // Handle indexing_credentials as a nested object before COMMON_SENSITIVE.
    // It's serialized to a JSON string and encrypted as an opaque blob.
    if let Some(ic_value) = incoming_obj.remove("indexing_credentials") {
        match ic_value {
            Value::Null => {
                // Explicit clear — already removed, nothing to restore.
            }
            Value::String(s) if s == MASKED_VALUE => {
                // Restore the existing encrypted blob.
                if let Some(existing_val) =
                    existing_obj.and_then(|eo| eo.get("indexing_credentials"))
                {
                    if existing_val.is_null() {
                        // Nothing to restore.
                    } else {
                        incoming_obj
                            .insert("indexing_credentials".to_string(), existing_val.clone());
                    }
                }
                // else: no existing value — field stays removed.
            }
            Value::Object(_) => {
                let json_str = serde_json::to_string(&ic_value).map_err(|e| {
                    kyomi_core::Error::Internal(format!(
                        "failed to serialize indexing_credentials: {e}"
                    ))
                })?;
                let encrypted = crate::encryption::encrypt(&json_str, key)?;
                incoming_obj
                    .insert("indexing_credentials".to_string(), Value::String(encrypted));
            }
            _ => {
                // Any other type — remove it.
            }
        }
    }

    for &field in COMMON_SENSITIVE {
        let is_masked_or_absent = match incoming_obj.get(field) {
            Some(Value::Null) => {
                // Explicit clear — remove the field rather than restoring it.
                incoming_obj.remove(field);
                continue;
            }
            None => true,
            Some(Value::String(s)) => s == MASKED_VALUE,
            Some(_) => false,
        };

        if is_masked_or_absent {
            match existing_obj.and_then(|eo| eo.get(field)) {
                Some(Value::String(existing_val)) if !existing_val.is_empty() => {
                    incoming_obj.insert(field.to_string(), Value::String(existing_val.clone()));
                }
                // Nothing to restore — e.g. `existing` is `None` on create,
                // or the field was never set. A masked placeholder must
                // never be persisted literally, so drop it rather than
                // leaving `MASKED_VALUE` sitting in the stored config.
                _ => {
                    incoming_obj.remove(field);
                }
            }
            continue;
        }

        // A real, non-masked value was provided. If it's a non-empty
        // string, it's fresh plaintext from the client — encrypt it before
        // it's persisted.
        if let Some(Value::String(s)) = incoming_obj.get(field)
            && !s.is_empty()
        {
            let encrypted = crate::encryption::encrypt(s, key)?;
            incoming_obj.insert(field.to_string(), Value::String(encrypted));
        }
    }

    Ok(())
}

/// Heuristic check for whether `s` looks like ciphertext produced by
/// [`crate::encryption::encrypt`] (base64url of `version_byte + nonce + tag +
/// ciphertext`, version `0x02`).
///
/// Used by [`decrypt_connection_config_secrets`] to distinguish freshly
/// encrypted secrets from legacy plaintext values that predate this
/// encryption layer (e.g. a `shared_password` written before this feature
/// shipped) — legacy plaintext must pass through unchanged rather than
/// erroring or being treated as garbage ciphertext.
pub(crate) fn looks_encrypted(s: &str) -> bool {
    match URL_SAFE.decode(s) {
        // version byte (1) + nonce (12) + at least the 16-byte GCM tag.
        Ok(bytes) => bytes.len() >= 1 + 12 + 16 && bytes[0] == 0x02,
        Err(_) => false,
    }
}

/// Decrypt `s` with `key` if it looks like our ciphertext; otherwise return
/// it unchanged, treating it as legacy plaintext or a non-secret placeholder.
///
/// # Errors
///
/// Returns [`kyomi_core::Error::CredentialDecryptionFailed`] if `s` passed
/// [`looks_encrypted`] (so it really is Kyomi ciphertext) but failed to
/// decrypt — a rotated/mismatched encryption key or corrupted/tampered data.
/// That condition is never caller-recoverable and must never be handed to a
/// datasource driver as if it were the plaintext secret (KYO-221). The
/// `!looks_encrypted` passthrough above is unaffected — it remains the
/// deliberate legacy-plaintext path.
fn decrypt_or_passthrough(field: &str, s: &str, key: &[u8; 32]) -> kyomi_core::Result<String> {
    if !looks_encrypted(s) {
        return Ok(s.to_string());
    }

    match crate::encryption::decrypt(s, key) {
        Ok(plaintext) => Ok(plaintext),
        Err(e) => {
            // Looked like our ciphertext but failed to decrypt (wrong/rotated
            // key, corrupted data) — never a legacy-plaintext case, and never
            // something the caller can recover from. Must fail loudly rather
            // than passing the ciphertext through as if it were the secret;
            // never log the ciphertext or plaintext, field name and error
            // only.
            tracing::error!(
                field,
                error = %e,
                "connection_config field looked encrypted but failed to decrypt — check the encryption key"
            );
            Err(kyomi_core::Error::CredentialDecryptionFailed(format!(
                "credential could not be decrypted — check the encryption key (field: {field})"
            )))
        }
    }
}

/// Decrypt all [`COMMON_SENSITIVE`] fields in `config`, returning a clone
/// with plaintext values.
///
/// **Migration-safe**: fields are only decrypted if [`looks_encrypted`]
/// recognizes them as our ciphertext format. Legacy rows written before this
/// encryption layer shipped (plaintext `shared_password`, for example) pass
/// through unchanged instead of erroring.
///
/// Call this immediately before building any datasource provider —
/// `connection_config` is encrypted at rest, but every driver must receive
/// plaintext.
///
/// # Errors
///
/// Returns [`kyomi_core::Error::CredentialDecryptionFailed`], identifying the
/// field, if any field looks like Kyomi ciphertext but fails to decrypt (see
/// [`decrypt_or_passthrough`]). No partial config containing ciphertext is
/// ever returned — the first undecryptable field short-circuits the whole
/// call.
pub fn decrypt_connection_config_secrets(config: &Value, key: &[u8; 32]) -> kyomi_core::Result<Value> {
    let Some(obj) = config.as_object() else {
        return Ok(config.clone());
    };

    let mut result = obj.clone();
    for &field in COMMON_SENSITIVE {
        if let Some(Value::String(s)) = result.get(field) {
            let decrypted = decrypt_or_passthrough(field, s, key)?;
            result.insert(field.to_string(), Value::String(decrypted));
        }
    }

    // Decrypt indexing_credentials — stored as an encrypted JSON string,
    // restore to a Value::Object for the datasource driver.
    if let Some(Value::String(s)) = result.get("indexing_credentials") {
        let decrypted = decrypt_or_passthrough("indexing_credentials", s, key)?;
        match serde_json::from_str::<Value>(&decrypted) {
            Ok(obj @ Value::Object(_)) => {
                result.insert("indexing_credentials".to_string(), obj);
            }
            _ => {
                // Not valid JSON or not an object — leave as decrypted string.
                result.insert("indexing_credentials".to_string(), Value::String(decrypted));
            }
        }
    }

    Ok(Value::Object(result))
}

/// Decrypt a datasource's `connection_config` secrets AND an optional
/// encrypted credential blob together, for provider construction.
///
/// Returns `(plaintext_connection_config, plaintext_credentials)`.
///
/// - `connection_config` is decrypted via [`decrypt_connection_config_secrets`]
///   (migration-safe: legacy plaintext / masked values pass through unchanged).
/// - `encrypted_credentials`, if present, is decrypted via
///   `encryption::decrypt_json`. Missing (`None`) or undecryptable credentials
///   yield an empty JSON object rather than erroring — callers building a
///   datasource provider with empty credentials will simply fail to
///   authenticate, which surfaces the problem without crashing the request.
///
/// This is the single entry point `#[server]` fns should call before building
/// a datasource provider — consolidating both decryptions into one call keeps
/// callout-heavy server_fns under the service-callout lint budget (see
/// `scripts/lint/check-server-fns.sh`).
///
/// # Errors
///
/// Returns [`kyomi_core::Error::CredentialDecryptionFailed`] if
/// `connection_config` fails to decrypt — see
/// [`decrypt_connection_config_secrets`]. `encrypted_credentials` failing to
/// decrypt is deliberately **not** an error here (see above): it degrades to
/// an empty object, matching this function's existing documented contract.
pub fn decrypt_provider_secrets(
    connection_config: &Value,
    encrypted_credentials: Option<&str>,
    key: &[u8; 32],
) -> kyomi_core::Result<(Value, Value)> {
    let plaintext_config = decrypt_connection_config_secrets(connection_config, key)?;
    let plaintext_credentials = match encrypted_credentials {
        Some(enc) => crate::encryption::decrypt_json(enc, key).unwrap_or_else(|_| serde_json::json!({})),
        None => serde_json::json!({}),
    };
    Ok((plaintext_config, plaintext_credentials))
}

/// Replace a field value with [`MASKED_VALUE`] if it is a non-empty string.
///
/// Only masks values that are non-null, non-empty strings. Non-string values
/// (numbers, booleans, objects, arrays) and null/empty strings are left as-is.
fn mask_field_if_present(obj: &mut serde_json::Map<String, Value>, field: &str) {
    if let Some(val) = obj.get(field)
        && let Some(s) = val.as_str()
        && !s.is_empty()
    {
        obj.insert(field.to_string(), Value::String(MASKED_VALUE.into()));
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::encryption;
    use serde_json::json;

    fn test_key() -> [u8; 32] {
        let mut key = [0u8; 32];
        key[..16].copy_from_slice(b"test-key-1234567");
        key[16..].copy_from_slice(b"8901234567890123");
        key
    }

    // -- encrypt / decrypt roundtrip ---

    #[test]
    fn encrypt_decrypt_roundtrip_simple() {
        let key = test_key();
        let creds = json!({"username": "admin", "password": "secret123"});

        let encrypted = encryption::encrypt_json(&creds, &key).unwrap();
        let decrypted = encryption::decrypt_json(&encrypted, &key).unwrap();

        assert_eq!(decrypted, creds);
    }

    #[test]
    fn encrypt_decrypt_roundtrip_nested() {
        let key = test_key();
        let creds = json!({
            "username": "admin",
            "password": "secret123",
            "oauth_data": {
                "access_token": "tok-abc",
                "refresh_token": "ref-xyz"
            }
        });

        let encrypted = encryption::encrypt_json(&creds, &key).unwrap();
        let decrypted = encryption::decrypt_json(&encrypted, &key).unwrap();

        assert_eq!(decrypted, creds);
    }

    #[test]
    fn encrypt_decrypt_roundtrip_empty_object() {
        let key = test_key();
        let creds = json!({});

        let encrypted = encryption::encrypt_json(&creds, &key).unwrap();
        let decrypted = encryption::decrypt_json(&encrypted, &key).unwrap();

        assert_eq!(decrypted, creds);
    }

    #[test]
    fn each_encryption_is_unique() {
        let key = test_key();
        let creds = json!({"password": "same"});

        let enc1 = encryption::encrypt_json(&creds, &key).unwrap();
        let enc2 = encryption::encrypt_json(&creds, &key).unwrap();

        assert_ne!(enc1, enc2, "different nonces should produce different ciphertext");

        // Both decrypt to the same value
        assert_eq!(
            encryption::decrypt_json(&enc1, &key).unwrap(),
            encryption::decrypt_json(&enc2, &key).unwrap()
        );
    }

    // -- mask_credentials ---

    #[test]
    fn mask_credentials_replaces_password_for_postgres() {
        let creds = json!({"username": "admin", "password": "secret123"});
        let masked = mask_credentials(&creds, "postgres");

        assert_eq!(masked["username"], "admin");
        assert_eq!(masked["password"], MASKED_VALUE);
    }

    #[test]
    fn mask_credentials_replaces_password_for_clickhouse() {
        let creds = json!({"username": "default", "password": "ch-pass"});
        let masked = mask_credentials(&creds, "clickhouse");

        assert_eq!(masked["username"], "default");
        assert_eq!(masked["password"], MASKED_VALUE);
    }

    #[test]
    fn mask_credentials_replaces_access_token_for_databricks() {
        let creds = json!({"access_token": "dapi-secret-token"});
        let masked = mask_credentials(&creds, "databricks");

        assert_eq!(masked["access_token"], MASKED_VALUE);
    }

    #[test]
    fn mask_credentials_preserves_non_sensitive_for_bigquery() {
        let creds = json!({"billing_project": "my-project", "default_project": "my-project"});
        let masked = mask_credentials(&creds, "bigquery");

        // BigQuery has no sensitive credential fields
        assert_eq!(masked["billing_project"], "my-project");
        assert_eq!(masked["default_project"], "my-project");
    }

    #[test]
    fn mask_credentials_handles_unknown_type() {
        let creds = json!({"username": "admin", "password": "secret"});
        let masked = mask_credentials(&creds, "unknown_type");

        // Unknown type — nothing is masked
        assert_eq!(masked["username"], "admin");
        assert_eq!(masked["password"], "secret");
    }

    #[test]
    fn mask_credentials_handles_non_object() {
        let creds = json!("just a string");
        let masked = mask_credentials(&creds, "postgres");
        assert_eq!(masked, creds);
    }

    #[test]
    fn mask_credentials_skips_null_and_empty_values() {
        let creds = json!({"username": "admin", "password": null});
        let masked = mask_credentials(&creds, "postgres");
        assert!(masked["password"].is_null(), "null values should not be masked");

        let creds2 = json!({"username": "admin", "password": ""});
        let masked2 = mask_credentials(&creds2, "postgres");
        assert_eq!(masked2["password"], "", "empty strings should not be masked");
    }

    // -- mask_connection_config ---

    #[test]
    fn mask_connection_config_masks_bigquery_sensitive_fields() {
        let config = json!({
            "auth_mode": "enterprise_oauth",
            "oauth_client_id": "client-123",
            "oauth_client_secret": "super-secret",
            "service_account_json": "{\"type\":\"service_account\"}",
            "catalog_projects": ["my-project"]
        });

        let masked = mask_connection_config(&config, "bigquery");

        assert_eq!(masked["auth_mode"], "enterprise_oauth");
        assert_eq!(masked["oauth_client_id"], "client-123");
        assert_eq!(masked["oauth_client_secret"], MASKED_VALUE);
        assert_eq!(masked["service_account_json"], MASKED_VALUE);
        assert_eq!(masked["catalog_projects"], json!(["my-project"]));
    }

    #[test]
    fn mask_connection_config_always_masks_shared_password() {
        let config = json!({
            "host": "db.example.com",
            "port": 5432,
            "shared_credentials": true,
            "shared_password": "shared-secret"
        });

        let masked = mask_connection_config(&config, "postgres");

        assert_eq!(masked["host"], "db.example.com");
        assert_eq!(masked["shared_password"], MASKED_VALUE);
    }

    #[test]
    fn mask_connection_config_always_masks_ssh_private_key() {
        let config = json!({
            "host": "db.example.com",
            "ssh_enabled": true,
            "ssh_private_key": "-----BEGIN OPENSSH PRIVATE KEY-----\nblah\n-----END OPENSSH PRIVATE KEY-----"
        });

        // ssh_private_key is in COMMON_SENSITIVE, so it's masked for any type
        let masked = mask_connection_config(&config, "postgres");
        assert_eq!(masked["ssh_private_key"], MASKED_VALUE);

        let masked2 = mask_connection_config(&config, "clickhouse");
        assert_eq!(masked2["ssh_private_key"], MASKED_VALUE);
    }

    #[test]
    fn mask_connection_config_handles_unknown_type() {
        let config = json!({
            "host": "db.example.com",
            "shared_password": "should-be-masked"
        });

        let masked = mask_connection_config(&config, "unknown_type");

        // Unknown type — common fields are still masked
        assert_eq!(masked["host"], "db.example.com");
        assert_eq!(masked["shared_password"], MASKED_VALUE);
    }

    #[test]
    fn mask_connection_config_snowflake_no_type_specific_sensitive() {
        // Snowflake has no type-specific sensitive_connection_config_fields
        // (matches Python source). Common fields (shared_password, ssh_private_key)
        // are still masked.
        let config = json!({
            "account": "xy12345.us-east-1",
            "oauth_client_secret": "snow-secret",
            "shared_password": "shared-pw"
        });

        let masked = mask_connection_config(&config, "snowflake");
        assert_eq!(masked["account"], "xy12345.us-east-1");
        // oauth_client_secret is NOT in Snowflake's type-specific fields
        assert_eq!(masked["oauth_client_secret"], "snow-secret");
        // shared_password is always masked via COMMON_SENSITIVE
        assert_eq!(masked["shared_password"], MASKED_VALUE);
    }

    #[test]
    fn mask_connection_config_preserves_non_sensitive() {
        let config = json!({
            "host": "localhost",
            "port": 5432,
            "database": "mydb",
            "ssl_mode": "require"
        });

        let masked = mask_connection_config(&config, "postgres");
        assert_eq!(masked, config, "no sensitive fields present — should be unchanged");
    }

    #[test]
    fn mask_redshift_sensitive_credential_fields() {
        // Redshift sensitive_credential_fields is ["password"] (matches Python source).
        // access_key_id and secret_access_key are NOT in the sensitive list.
        let creds = json!({
            "username": "admin",
            "password": "pass123",
            "access_key_id": "AKIA...",
            "secret_access_key": "secret..."
        });

        let masked = mask_credentials(&creds, "redshift");
        assert_eq!(masked["username"], "admin");
        assert_eq!(masked["password"], MASKED_VALUE);
        // These are NOT in Redshift's sensitive_credential_fields
        assert_eq!(masked["access_key_id"], "AKIA...");
        assert_eq!(masked["secret_access_key"], "secret...");
    }

    #[test]
    fn mask_snowflake_sensitive_credential_fields() {
        // Snowflake's key-pair auth mode carries a PEM private_key alongside
        // username/password (KYO-330). Both password and private_key are
        // sensitive_credential_fields and must be masked; username must not.
        let creds = json!({
            "username": "admin",
            "password": "pass123",
            "private_key": "-----BEGIN PRIVATE KEY-----\nMIIEvQ...\n-----END PRIVATE KEY-----"
        });

        let masked = mask_credentials(&creds, "snowflake");
        assert_eq!(masked["username"], "admin");
        assert_eq!(masked["password"], MASKED_VALUE);
        assert_eq!(masked["private_key"], MASKED_VALUE);
    }

    #[test]
    fn mask_credentials_skips_non_string_values() {
        // Non-string values (numbers, booleans) should NOT be masked
        let creds = json!({
            "username": "admin",
            "password": 12345,
        });
        let masked = mask_credentials(&creds, "postgres");
        assert_eq!(masked["password"], 12345, "numeric values should not be masked");

        let creds2 = json!({
            "username": "admin",
            "password": true,
        });
        let masked2 = mask_credentials(&creds2, "postgres");
        assert_eq!(masked2["password"], true, "boolean values should not be masked");
    }

    #[test]
    fn mask_synapse_sensitive_credential_fields() {
        let creds = json!({
            "auth_type": "sql",
            "username": "admin",
            "password": "pass123",
            "client_secret": "az-secret",
            "oauth_access_token": "tok",
            "oauth_refresh_token": "ref"
        });

        let masked = mask_credentials(&creds, "synapse");
        assert_eq!(masked["auth_type"], "sql");
        assert_eq!(masked["username"], "admin");
        assert_eq!(masked["password"], MASKED_VALUE);
        assert_eq!(masked["client_secret"], MASKED_VALUE);
        assert_eq!(masked["oauth_access_token"], MASKED_VALUE);
        assert_eq!(masked["oauth_refresh_token"], MASKED_VALUE);
    }

    // -- finalize_connection_config_secrets ---

    #[test]
    fn finalize_restores_omitted_and_masked_sensitive_fields_verbatim() {
        let key = test_key();
        let existing_ciphertext = encryption::encrypt("real-shared-pass", &key).unwrap();
        let existing = json!({
            "host": "db.example.com",
            "ssh_private_key": "already-ciphertext-blob",
            "shared_password": existing_ciphertext
        });

        // Incoming omits ssh_private_key entirely and sends the masked
        // placeholder for shared_password.
        let mut incoming = json!({
            "host": "db.example.com",
            "shared_password": MASKED_VALUE
        });

        finalize_connection_config_secrets(&mut incoming, Some(&existing), &key).unwrap();

        // Restored verbatim — NOT re-encrypted (still the exact stored ciphertext).
        assert_eq!(incoming["ssh_private_key"], existing["ssh_private_key"]);
        assert_eq!(incoming["shared_password"], existing["shared_password"]);
    }

    #[test]
    fn finalize_encrypts_a_fresh_plaintext_value_and_does_not_clobber_it() {
        let key = test_key();
        let existing = json!({
            "ssh_private_key": "old-ciphertext-blob"
        });

        let new_plaintext = "-----BEGIN OPENSSH PRIVATE KEY-----\nnew-key\n-----END OPENSSH PRIVATE KEY-----";
        let mut incoming = json!({ "ssh_private_key": new_plaintext });

        finalize_connection_config_secrets(&mut incoming, Some(&existing), &key).unwrap();

        // The freshly-provided value must be encrypted, not passed through
        // as plaintext, and must NOT be replaced with the old stored value.
        let stored = incoming["ssh_private_key"].as_str().unwrap();
        assert_ne!(stored, new_plaintext, "plaintext must not be stored as-is");
        assert!(looks_encrypted(stored), "new value should be encrypted at rest");
        assert_eq!(encryption::decrypt(stored, &key).unwrap(), new_plaintext);
    }

    #[test]
    fn finalize_explicit_null_clears_instead_of_restoring() {
        let key = test_key();
        let existing = json!({
            "host": "db.example.com",
            "ssh_enabled": true,
            "ssh_private_key": "old-ciphertext-blob"
        });

        // Incoming explicitly clears ssh_private_key (e.g. SSH tunnel disabled),
        // rather than merely omitting it.
        let mut incoming = json!({
            "host": "db.example.com",
            "ssh_enabled": false,
            "ssh_private_key": Value::Null
        });

        finalize_connection_config_secrets(&mut incoming, Some(&existing), &key).unwrap();

        // The field must be absent from the result, not restored from existing.
        assert!(
            incoming.get("ssh_private_key").is_none(),
            "explicit null must clear the field, not restore the old value"
        );
    }

    #[test]
    fn finalize_leaves_non_sensitive_fields_untouched_and_absent_stays_absent() {
        let key = test_key();
        let existing = json!({
            "host": "old-host.example.com",
            "port": 5432
        });

        let mut incoming = json!({
            "host": "new-host.example.com",
            "port": 5433
        });

        finalize_connection_config_secrets(&mut incoming, Some(&existing), &key).unwrap();

        // Non-sensitive fields pass through untouched.
        assert_eq!(incoming["host"], "new-host.example.com");
        assert_eq!(incoming["port"], 5433);

        // A sensitive field absent from both existing and incoming stays
        // absent — nothing to restore from.
        assert!(incoming.get("ssh_private_key").is_none());
        assert!(incoming.get("shared_password").is_none());
    }

    #[test]
    fn finalize_on_create_with_no_existing_config_encrypts_fresh_values() {
        let key = test_key();
        let mut incoming = json!({
            "host": "db.example.com",
            "shared_password": "brand-new-password"
        });

        // `existing = None` is the create-mode case — there is nothing to
        // restore from, but a freshly-provided secret must still be encrypted.
        finalize_connection_config_secrets(&mut incoming, None, &key).unwrap();

        let stored = incoming["shared_password"].as_str().unwrap();
        assert!(looks_encrypted(stored));
        assert_eq!(encryption::decrypt(stored, &key).unwrap(), "brand-new-password");
    }

    #[test]
    fn finalize_on_create_with_masked_or_absent_value_leaves_field_absent() {
        let key = test_key();

        // Masked placeholder with nothing to restore from (existing = None).
        let mut incoming = json!({ "shared_password": MASKED_VALUE });
        finalize_connection_config_secrets(&mut incoming, None, &key).unwrap();
        assert!(incoming.get("shared_password").is_none());
    }

    // -- looks_encrypted / decrypt_connection_config_secrets ---

    #[test]
    fn looks_encrypted_recognizes_our_ciphertext_format() {
        let key = test_key();
        let ciphertext = encryption::encrypt("some-secret", &key).unwrap();
        assert!(looks_encrypted(&ciphertext));
    }

    #[test]
    fn looks_encrypted_rejects_masked_placeholder() {
        assert!(!looks_encrypted(MASKED_VALUE));
    }

    #[test]
    fn looks_encrypted_rejects_legacy_plaintext() {
        assert!(!looks_encrypted("hunter2"));
        assert!(!looks_encrypted("-----BEGIN OPENSSH PRIVATE KEY-----\nabc\n-----END OPENSSH PRIVATE KEY-----"));
    }

    #[test]
    fn decrypt_connection_config_secrets_round_trips_finalize_encrypted_values() {
        let key = test_key();
        let mut config = json!({
            "host": "db.example.com",
            "ssh_private_key": "-----BEGIN OPENSSH PRIVATE KEY-----\nreal-key\n-----END OPENSSH PRIVATE KEY-----",
            "shared_password": "real-shared-pass"
        });
        finalize_connection_config_secrets(&mut config, None, &key).unwrap();

        // Sanity: the finalized config really is ciphertext now.
        assert!(looks_encrypted(config["ssh_private_key"].as_str().unwrap()));
        assert!(looks_encrypted(config["shared_password"].as_str().unwrap()));

        let decrypted = decrypt_connection_config_secrets(&config, &key).unwrap();

        assert_eq!(
            decrypted["ssh_private_key"],
            "-----BEGIN OPENSSH PRIVATE KEY-----\nreal-key\n-----END OPENSSH PRIVATE KEY-----"
        );
        assert_eq!(decrypted["shared_password"], "real-shared-pass");
        // Non-sensitive fields pass through unchanged.
        assert_eq!(decrypted["host"], "db.example.com");
    }

    #[test]
    fn decrypt_connection_config_secrets_passes_through_legacy_plaintext() {
        let key = test_key();
        // A row written before this encryption layer shipped — plaintext,
        // not our ciphertext format.
        let config = json!({
            "host": "db.example.com",
            "shared_password": "legacy-plaintext-password"
        });

        let decrypted = decrypt_connection_config_secrets(&config, &key).unwrap();

        assert_eq!(decrypted["shared_password"], "legacy-plaintext-password");
    }

    #[test]
    fn decrypt_connection_config_secrets_leaves_masked_value_as_is() {
        let key = test_key();
        let config = json!({ "shared_password": MASKED_VALUE });

        let decrypted = decrypt_connection_config_secrets(&config, &key).unwrap();

        assert_eq!(decrypted["shared_password"], MASKED_VALUE);
    }

    #[test]
    fn decrypt_connection_config_secrets_handles_non_object() {
        let key = test_key();
        let config = json!("not an object");
        assert_eq!(decrypt_connection_config_secrets(&config, &key).unwrap(), config);
    }

    // -- KYO-221: decrypt failure must error, never pass through ciphertext --

    #[test]
    fn decrypt_or_passthrough_wrong_key_errors_and_does_not_return_ciphertext() {
        let key_a = test_key();
        let mut key_b = [0u8; 32];
        key_b[..16].copy_from_slice(b"other-test-key-1");
        key_b[16..].copy_from_slice(b"2345678901234567");
        assert_ne!(key_a, key_b, "test fixture sanity: keys must differ");

        let ciphertext = encryption::encrypt("real-shared-pass", &key_a).unwrap();

        let err = decrypt_or_passthrough("shared_password", &ciphertext, &key_b)
            .expect_err("decrypting with the wrong key must error, not fall back to the raw value");

        // The whole point of KYO-221: the ciphertext must never come back out
        // disguised as a successfully-resolved value.
        assert!(
            matches!(err, kyomi_core::Error::CredentialDecryptionFailed(_)),
            "wrong-key failure must surface as CredentialDecryptionFailed, got: {err:?}"
        );
        let msg = err.to_string();
        assert!(
            !msg.contains(&ciphertext),
            "error message must never contain the ciphertext: {msg}"
        );
        assert!(
            msg.contains("check the encryption key"),
            "message must point at the encryption key, distinct from an auth failure: {msg}"
        );
        assert!(!msg.contains("authentication"), "must not read like an auth failure: {msg}");
    }

    #[test]
    fn decrypt_or_passthrough_legacy_plaintext_passes_through_unchanged_no_error() {
        // The regression guard: `looks_encrypted("hunter2")` is false, so this
        // must take the deliberate legacy-plaintext passthrough — never an
        // error, and the value must come back byte-for-byte unchanged.
        let key = test_key();
        assert!(!looks_encrypted("hunter2"));

        let result = decrypt_or_passthrough("shared_password", "hunter2", &key);

        assert_eq!(result.unwrap(), "hunter2");
    }

    #[test]
    fn decrypt_connection_config_secrets_legacy_plaintext_field_passes_through_unchanged() {
        // Same regression guard at the `decrypt_connection_config_secrets`
        // level (the actual pre-provider-construction entry point).
        let key = test_key();
        let config = json!({ "host": "db.example.com", "shared_password": "hunter2" });

        let decrypted = decrypt_connection_config_secrets(&config, &key).unwrap();

        assert_eq!(decrypted["shared_password"], "hunter2");
    }

    #[test]
    fn decrypt_or_passthrough_round_trip_with_correct_key_returns_plaintext() {
        let key = test_key();
        let ciphertext = encryption::encrypt("correct-key-plaintext", &key).unwrap();

        let result = decrypt_or_passthrough("shared_password", &ciphertext, &key);

        assert_eq!(result.unwrap(), "correct-key-plaintext");
    }

    #[test]
    fn decrypt_or_passthrough_tampered_tag_errors() {
        // Flip a byte inside the AEAD tag (the last 16 bytes of the decoded
        // payload) so the ciphertext still passes `looks_encrypted` (correct
        // version byte, correct minimum length) but AEAD verification must
        // reject it — the same error arm as a wrong key.
        let key = test_key();
        let ciphertext = encryption::encrypt("tamper-me", &key).unwrap();

        let mut bytes = URL_SAFE.decode(&ciphertext).unwrap();
        let last = bytes.len() - 1;
        bytes[last] ^= 0xFF;
        let tampered = URL_SAFE.encode(&bytes);
        assert!(looks_encrypted(&tampered), "tampered value must still look like our ciphertext");

        let err = decrypt_or_passthrough("shared_password", &tampered, &key)
            .expect_err("tag-tampered ciphertext must error, not decrypt to garbage or pass through");

        assert!(matches!(err, kyomi_core::Error::CredentialDecryptionFailed(_)));
    }

    #[test]
    fn decrypt_connection_config_secrets_one_bad_field_errors_and_identifies_it() {
        let key_a = test_key();
        let mut key_b = [0u8; 32];
        key_b[..16].copy_from_slice(b"other-test-key-1");
        key_b[16..].copy_from_slice(b"2345678901234567");

        // shared_password is encrypted with a DIFFERENT key than the one
        // used to decrypt — everything else in the config is fine.
        let bad_ciphertext = encryption::encrypt("will-not-decrypt", &key_a).unwrap();
        let good_ciphertext = encryption::encrypt("real-ssh-key", &key_b).unwrap();
        let config = json!({
            "host": "db.example.com",
            "shared_password": bad_ciphertext,
            "ssh_private_key": good_ciphertext,
        });

        let err = decrypt_connection_config_secrets(&config, &key_b)
            .expect_err("one undecryptable field must fail the whole call");

        let msg = err.to_string();
        assert!(
            msg.contains("shared_password"),
            "error must identify which field failed to decrypt: {msg}"
        );
        // No partial/successful `Value` exists to inspect — `?` short-circuits
        // before any ciphertext could be inserted into a returned config.
        // (If this compiled as `Value` instead of `Result<Value, _>`, that
        // alone would mean the fix regressed — see the type signature above.)
    }

    // -- end-to-end-ish: finalize then decrypt round trip for a fresh SSH key ---

    #[test]
    fn ssh_private_key_survives_finalize_then_decrypt_round_trip() {
        let key = test_key();
        let pem = "-----BEGIN OPENSSH PRIVATE KEY-----\nb3BlbnNzaC1rZXktdjEA\n-----END OPENSSH PRIVATE KEY-----";

        // Simulates create_datasource: brand-new plaintext PEM from the
        // client, no existing stored config.
        let mut connection_config = json!({
            "host": "db.example.com",
            "ssh_enabled": true,
            "ssh_private_key": pem
        });
        finalize_connection_config_secrets(&mut connection_config, None, &key).unwrap();

        // What's "persisted" is ciphertext, not the PEM.
        let stored = connection_config["ssh_private_key"].as_str().unwrap();
        assert_ne!(stored, pem);
        assert!(looks_encrypted(stored));

        // What the driver receives just before provider creation is plaintext again.
        let for_driver = decrypt_connection_config_secrets(&connection_config, &key).unwrap();
        assert_eq!(for_driver["ssh_private_key"], pem);
    }

    // -- decrypt_provider_secrets -------------------------------------------

    #[test]
    fn decrypt_provider_secrets_decrypts_both_config_and_credentials() {
        let key = test_key();
        let mut connection_config = json!({ "host": "db.example.com", "shared_password": "s3cr3t" });
        finalize_connection_config_secrets(&mut connection_config, None, &key).unwrap();

        let creds = json!({ "username": "alice", "password": "hunter2" });
        let encrypted_creds = encryption::encrypt_json(&creds, &key).unwrap();

        let (config, credentials) =
            decrypt_provider_secrets(&connection_config, Some(&encrypted_creds), &key).unwrap();

        assert_eq!(config["shared_password"], "s3cr3t");
        assert_eq!(config["host"], "db.example.com");
        assert_eq!(credentials, creds);
    }

    #[test]
    fn decrypt_provider_secrets_yields_empty_object_when_no_credentials() {
        let key = test_key();
        let connection_config = json!({ "host": "db.example.com" });

        let (config, credentials) = decrypt_provider_secrets(&connection_config, None, &key).unwrap();

        assert_eq!(config["host"], "db.example.com");
        assert_eq!(credentials, json!({}));
    }

    #[test]
    fn decrypt_provider_secrets_yields_empty_object_when_credentials_undecryptable() {
        let key = test_key();
        let connection_config = json!({ "host": "db.example.com" });

        let (_, credentials) =
            decrypt_provider_secrets(&connection_config, Some("not valid ciphertext"), &key).unwrap();

        assert_eq!(credentials, json!({}));
    }

    // -- indexing_credentials -----------------------------------------------

    #[test]
    fn indexing_credentials_finalize_encrypts_object_at_rest() {
        let key = test_key();
        let ic = json!({
            "type": "password",
            "username": "readonly",
            "password": "secret123"
        });
        let mut incoming = json!({
            "host": "db.example.com",
            "indexing_credentials": ic
        });

        finalize_connection_config_secrets(&mut incoming, None, &key).unwrap();

        let stored = incoming["indexing_credentials"].as_str().unwrap();
        assert!(
            looks_encrypted(stored),
            "indexing_credentials must be encrypted at rest"
        );
        assert_ne!(
            stored,
            serde_json::to_string(&json!({
                "type": "password",
                "username": "readonly",
                "password": "secret123"
            }))
            .unwrap(),
            "DB row must not contain plaintext JSON"
        );
    }

    #[test]
    fn indexing_credentials_masking_replaces_with_placeholder() {
        let config = json!({
            "host": "db.example.com",
            "indexing_credentials": {
                "type": "service_account",
                "service_account_json": "{\"type\":\"service_account\"}"
            }
        });

        let masked = mask_connection_config(&config, "bigquery");
        assert_eq!(masked["indexing_credentials"], MASKED_VALUE);
        assert_eq!(masked["host"], "db.example.com");
    }

    #[test]
    fn indexing_credentials_masking_masks_encrypted_string() {
        let key = test_key();
        let mut config = json!({
            "host": "db.example.com",
            "indexing_credentials": {
                "type": "password",
                "username": "ro",
                "password": "pw"
            }
        });
        finalize_connection_config_secrets(&mut config, None, &key).unwrap();

        let masked = mask_connection_config(&config, "bigquery");
        assert_eq!(masked["indexing_credentials"], MASKED_VALUE);
    }

    #[test]
    fn indexing_credentials_round_trip_finalize_mask_finalize_preserves_encrypted() {
        let key = test_key();
        let ic = json!({
            "type": "password",
            "username": "readonly",
            "password": "secret123"
        });
        let mut config = json!({
            "host": "db.example.com",
            "indexing_credentials": ic
        });

        // First finalize — encrypts the object.
        finalize_connection_config_secrets(&mut config, None, &key).unwrap();
        let first_encrypted = config["indexing_credentials"].as_str().unwrap().to_string();

        // Mask it (simulates API response to client).
        let masked = mask_connection_config(&config, "bigquery");
        assert_eq!(masked["indexing_credentials"], MASKED_VALUE);

        // Finalize again with masked value and existing = first_encrypted
        // (simulates client resubmitting without changes).
        let existing = config.clone();
        let mut incoming = masked.clone();
        finalize_connection_config_secrets(&mut incoming, Some(&existing), &key).unwrap();

        assert_eq!(
            incoming["indexing_credentials"].as_str().unwrap(),
            first_encrypted,
            "round-trip must preserve the existing encrypted blob, not re-encrypt"
        );
    }

    #[test]
    fn indexing_credentials_decryption_restores_object() {
        let key = test_key();
        let ic = json!({
            "type": "service_account",
            "service_account_json": "{\"type\":\"service_account\",\"client_email\":\"a@b.iam\"}"
        });
        let mut config = json!({
            "host": "db.example.com",
            "indexing_credentials": ic.clone()
        });

        finalize_connection_config_secrets(&mut config, None, &key).unwrap();

        let decrypted = decrypt_connection_config_secrets(&config, &key).unwrap();
        assert_eq!(decrypted["indexing_credentials"], ic);
        assert_eq!(decrypted["host"], "db.example.com");
    }

    #[test]
    fn indexing_credentials_explicit_null_removes_field() {
        let key = test_key();
        let existing = json!({
            "host": "db.example.com",
            "indexing_credentials": {
                "type": "password",
                "username": "ro",
                "password": "pw"
            }
        });

        let mut incoming = json!({
            "host": "db.example.com",
            "indexing_credentials": Value::Null
        });

        finalize_connection_config_secrets(&mut incoming, Some(&existing), &key).unwrap();

        assert!(
            incoming.get("indexing_credentials").is_none(),
            "explicit null must remove indexing_credentials entirely"
        );
    }
}
