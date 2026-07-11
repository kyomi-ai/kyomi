// SPDX-License-Identifier: AGPL-3.0-or-later

//! Credential masking helpers.
//!
//! Masking functions use the datasource type registry to determine which
//! fields are sensitive and should be replaced with `MASKED_VALUE`.
//!
//! For encrypting/decrypting credentials use `encryption::encrypt_json` /
//! `encryption::decrypt_json` directly.

use kyomi_core::datasource_registry;
use serde_json::Value;

/// The placeholder string that replaces sensitive fields in API responses.
pub const MASKED_VALUE: &str = "********";

/// Connection config fields that are always sensitive, regardless of
/// datasource type. Masked on read by [`mask_connection_config`] and
/// restored on write by [`preserve_masked_connection_config`].
pub(crate) const COMMON_SENSITIVE: &[&str] = &["shared_password", "ssh_private_key"];

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
/// fields. Also always masks `shared_password` and `ssh_private_key` regardless
/// of type, as these are common sensitive fields across all datasource types.
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

    Value::Object(masked)
}

/// Restore masked/omitted sensitive `connection_config` fields from the
/// stored config on update.
///
/// This is the write-side counterpart to [`mask_connection_config`]. Sensitive
/// fields are never sent to the client in real form — they come back either
/// masked as [`MASKED_VALUE`] or omitted entirely (e.g. a UI that only
/// resubmits fields it actually loaded). Without this step, a wholesale
/// replace of `connection_config` on update would clobber the real stored
/// secret with the placeholder or silently drop it.
///
/// For each field in [`COMMON_SENSITIVE`], one of three things happens:
///
/// - `incoming[field]` is explicit JSON `null` — this is an **explicit clear**
///   (e.g. disabling an SSH tunnel drops its stored key). The field is
///   *removed* from `incoming` entirely; it is never restored from `existing`.
/// - `incoming[field]` is missing, or equal to [`MASKED_VALUE`] — this is the
///   normal edit case where the UI never resupplies a secret it only ever
///   received masked. `incoming[field]` is overwritten with the value from
///   `existing`, if any.
/// - `incoming[field]` holds any other real value — a freshly-provided value
///   always passes through unchanged; this function only ever fills gaps or
///   honors explicit clears, never overrides a genuine new value.
///
/// No-ops if either `incoming` or `existing` is not a JSON object.
pub fn preserve_masked_connection_config(incoming: &mut Value, existing: &Value) {
    let Some(existing_obj) = existing.as_object() else {
        return;
    };
    let Some(incoming_obj) = incoming.as_object_mut() else {
        return;
    };

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
        if !is_masked_or_absent {
            continue;
        }

        if let Some(Value::String(existing_val)) = existing_obj.get(field)
            && !existing_val.is_empty()
        {
            incoming_obj.insert(field.to_string(), Value::String(existing_val.clone()));
        }
    }
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

    // -- preserve_masked_connection_config ---

    #[test]
    fn preserve_masked_restores_omitted_and_masked_sensitive_fields() {
        let existing = json!({
            "host": "db.example.com",
            "ssh_private_key": "-----BEGIN OPENSSH PRIVATE KEY-----\nreal-key\n-----END OPENSSH PRIVATE KEY-----",
            "shared_password": "real-shared-pass"
        });

        // Incoming omits ssh_private_key entirely and sends the masked
        // placeholder for shared_password.
        let mut incoming = json!({
            "host": "db.example.com",
            "shared_password": MASKED_VALUE
        });

        preserve_masked_connection_config(&mut incoming, &existing);

        assert_eq!(incoming["ssh_private_key"], existing["ssh_private_key"]);
        assert_eq!(incoming["shared_password"], existing["shared_password"]);
    }

    #[test]
    fn preserve_masked_does_not_clobber_new_real_value() {
        let existing = json!({
            "ssh_private_key": "-----BEGIN OPENSSH PRIVATE KEY-----\nold-key\n-----END OPENSSH PRIVATE KEY-----"
        });

        let mut incoming = json!({
            "ssh_private_key": "-----BEGIN OPENSSH PRIVATE KEY-----\nnew-key\n-----END OPENSSH PRIVATE KEY-----"
        });

        preserve_masked_connection_config(&mut incoming, &existing);

        // A freshly-provided real value overrides — it must NOT be replaced
        // with the old stored value.
        assert_eq!(
            incoming["ssh_private_key"],
            "-----BEGIN OPENSSH PRIVATE KEY-----\nnew-key\n-----END OPENSSH PRIVATE KEY-----"
        );
    }

    #[test]
    fn preserve_masked_explicit_null_clears_instead_of_restoring() {
        let existing = json!({
            "host": "db.example.com",
            "ssh_enabled": true,
            "ssh_private_key": "-----BEGIN OPENSSH PRIVATE KEY-----\nreal-key\n-----END OPENSSH PRIVATE KEY-----"
        });

        // Incoming explicitly clears ssh_private_key (e.g. SSH tunnel disabled),
        // rather than merely omitting it.
        let mut incoming = json!({
            "host": "db.example.com",
            "ssh_enabled": false,
            "ssh_private_key": Value::Null
        });

        preserve_masked_connection_config(&mut incoming, &existing);

        // The field must be absent from the result, not restored from existing.
        assert!(
            incoming.get("ssh_private_key").is_none(),
            "explicit null must clear the field, not restore the old value"
        );
    }

    #[test]
    fn preserve_masked_leaves_non_sensitive_fields_untouched_and_absent_stays_absent() {
        let existing = json!({
            "host": "old-host.example.com",
            "port": 5432
        });

        let mut incoming = json!({
            "host": "new-host.example.com",
            "port": 5433
        });

        preserve_masked_connection_config(&mut incoming, &existing);

        // Non-sensitive fields pass through untouched.
        assert_eq!(incoming["host"], "new-host.example.com");
        assert_eq!(incoming["port"], 5433);

        // A sensitive field absent from both existing and incoming stays
        // absent — nothing to restore from.
        assert!(incoming.get("ssh_private_key").is_none());
        assert!(incoming.get("shared_password").is_none());
    }
}
