//! Shared identity edit/save behavior. These exercise the form's pure state
//! helpers; they cannot observe the external datasource driver's resolver.

use super::super::{
    MASKED_SHARED_PASSWORD, SHARED_PASSWORD_CONFIG_KEY, SHARED_USERNAME_CONFIG_KEY,
    shared_identity_from_config, shared_mode_deactivates, write_shared_credentials_config,
};
use serde_json::{Map, Value, json};

#[test]
fn shared_keys_pin_only_the_ui_half_of_the_published_driver_contract() {
    // The external driver's behavior cannot be verified by these local tests.
    assert_eq!(SHARED_USERNAME_CONFIG_KEY, "shared_username");
    assert_eq!(SHARED_PASSWORD_CONFIG_KEY, "shared_password");
    assert_eq!(
        MASKED_SHARED_PASSWORD,
        kyomi_auth::credential_service::MASKED_VALUE
    );
}

#[test]
fn reopening_and_disabling_shared_access_preserves_the_masked_identity() {
    let stored = json!({
        "shared_credentials": true,
        "shared_username": "workspace_reader",
        "shared_password": MASKED_SHARED_PASSWORD,
    });
    let (username, password) = shared_identity_from_config(&stored);
    let mut submitted = Map::new();
    write_shared_credentials_config(&mut submitted, false, true, username, password);
    assert_eq!(
        Value::Object(submitted),
        json!({
            "shared_credentials": false,
            "shared_username": "workspace_reader",
            "shared_password": MASKED_SHARED_PASSWORD,
        })
    );
}

#[test]
fn enabling_or_rotating_shared_credentials_writes_workspace_identity() {
    let mut submitted = Map::new();
    write_shared_credentials_config(
        &mut submitted,
        true,
        true,
        "reader".into(),
        "rotated-password".into(),
    );
    assert_eq!(
        Value::Object(submitted),
        json!({
            "shared_credentials": true,
            "shared_username": "reader",
            "shared_password": "rotated-password",
        })
    );
}

#[test]
fn unsupported_mode_deactivates_access_without_dropping_identity() {
    let mut submitted = Map::new();
    write_shared_credentials_config(
        &mut submitted,
        true,
        false,
        "reader".into(),
        MASKED_SHARED_PASSWORD.into(),
    );
    assert_eq!(submitted.get("shared_credentials"), Some(&json!(false)));
    assert_eq!(
        shared_identity_from_config(&Value::Object(submitted)),
        ("reader".to_string(), MASKED_SHARED_PASSWORD.to_string())
    );
    assert!(shared_mode_deactivates(false, false, true, false));
}

#[test]
fn loading_settings_keeps_activation_but_user_mode_changes_deactivate_it() {
    assert!(!shared_mode_deactivates(true, true, true, true));
    assert!(!shared_mode_deactivates(false, false, true, true));
    assert!(!shared_mode_deactivates(false, false, false, false));
    assert!(shared_mode_deactivates(false, true, true, true));
}

#[test]
fn absent_identity_loads_empty_and_does_not_create_secret_placeholders() {
    let (username, password) = shared_identity_from_config(&json!({}));
    let mut submitted = Map::new();
    write_shared_credentials_config(&mut submitted, false, true, username, password);
    assert_eq!(
        Value::Object(submitted),
        json!({"shared_credentials": false})
    );
}
