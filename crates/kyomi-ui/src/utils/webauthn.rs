// SPDX-License-Identifier: AGPL-3.0-or-later

//! WebAuthn utility functions for passkey registration and authentication.
//!
//! Wraps the browser's `navigator.credentials` API for use from WASM,
//! mirroring the behaviour of `@simplewebauthn/browser` used by the React
//! frontend. On non-WASM targets, stub implementations are provided so
//! the crate compiles for SSR.

// ---------------------------------------------------------------------------
// WASM implementation (browser)
// ---------------------------------------------------------------------------
#[cfg(target_arch = "wasm32")]
mod inner {
    use base64::engine::general_purpose::URL_SAFE_NO_PAD;
    use base64::Engine;
    use js_sys::{Array, ArrayBuffer, Object, Reflect, Uint8Array};
    use wasm_bindgen::prelude::*;
    use wasm_bindgen_futures::JsFuture;
    use web_sys::PublicKeyCredential;

    // -- Base64url <-> ArrayBuffer helpers ------------------------------------

    /// Decode a base64url string into a JS `ArrayBuffer`.
    fn base64url_to_array_buffer(encoded: &str) -> Result<ArrayBuffer, String> {
        let bytes = URL_SAFE_NO_PAD
            .decode(encoded)
            .map_err(|e| format!("base64url decode error: {e}"))?;
        let u8arr = Uint8Array::new_with_length(bytes.len() as u32);
        u8arr.copy_from(&bytes);
        Ok(u8arr.buffer())
    }

    /// Encode a JS `ArrayBuffer` as a base64url string (no padding).
    fn array_buffer_to_base64url(buf: &ArrayBuffer) -> String {
        let u8arr = Uint8Array::new(buf);
        let mut bytes = vec![0u8; u8arr.length() as usize];
        u8arr.copy_to(&mut bytes);
        URL_SAFE_NO_PAD.encode(&bytes)
    }

    // -- Helpers for JS object manipulation -----------------------------------

    /// Shorthand for `Reflect::get`, returning `Err(String)` on failure.
    fn js_get(target: &JsValue, key: &str) -> Result<JsValue, String> {
        Reflect::get(target, &JsValue::from_str(key))
            .map_err(|e| format!("failed to read '{key}': {e:?}"))
    }

    /// Shorthand for `Reflect::set`, returning `Err(String)` on failure.
    fn js_set(target: &JsValue, key: &str, val: &JsValue) -> Result<(), String> {
        Reflect::set(target, &JsValue::from_str(key), val)
            .map(|_| ())
            .map_err(|e| format!("failed to set '{key}': {e:?}"))
    }

    /// Format a `JsValue` error into a readable `String`.
    fn js_err(val: JsValue) -> String {
        val.as_string().unwrap_or_else(|| format!("{val:?}"))
    }

    // -- Convert credential descriptors (allowCredentials / excludeCredentials)

    /// Convert an array of `PublicKeyCredentialDescriptor`-like objects,
    /// replacing each `id` (base64url string) with an `ArrayBuffer`.
    fn convert_credential_descriptors(arr: &JsValue) -> Result<Array, String> {
        let src = Array::from(arr);
        let dst = Array::new();
        for i in 0..src.length() {
            let desc = src.get(i);
            // Clone the descriptor object so we don't mutate the original
            let out = Object::assign(&Object::new(), desc.unchecked_ref());
            let id_b64 = js_get(&out, "id")?;
            if let Some(id_str) = id_b64.as_string() {
                let buf = base64url_to_array_buffer(&id_str)?;
                js_set(&out, "id", &buf)?;
            }
            dst.push(&out);
        }
        Ok(dst)
    }

    // -- Public API -----------------------------------------------------------

    /// Check if WebAuthn/passkeys are available in this browser.
    ///
    /// Returns `true` when `window.PublicKeyCredential` exists and the browser
    /// reports that a user-verifying platform authenticator is available (or at
    /// least does not throw when asked).
    pub async fn is_webauthn_available() -> bool {
        let window = match web_sys::window() {
            Some(w) => w,
            None => return false,
        };

        // Check that PublicKeyCredential constructor exists on window.
        let pkc_exists = js_get(&window, "PublicKeyCredential")
            .map(|v| !v.is_undefined() && !v.is_null())
            .unwrap_or(false);
        if !pkc_exists {
            return false;
        }

        // Call PublicKeyCredential.isUserVerifyingPlatformAuthenticatorAvailable()
        // and resolve the promise. If it rejects or returns false we still
        // report `true` to allow external authenticators (matching React).
        let promise = PublicKeyCredential::is_user_verifying_platform_authenticator_available();
        match JsFuture::from(promise).await {
            Ok(_) => true, // promise resolved (value is a boolean, but we allow either)
            Err(_) => true, // fallback: allow external authenticators
        }
    }

    /// Start a WebAuthn authentication (login with passkey).
    ///
    /// `options_json` is the `publicKey` options object from the server,
    /// JSON-serialised. Fields like `challenge` and `allowCredentials[].id`
    /// are base64url-encoded strings that will be converted to `ArrayBuffer`
    /// before passing to `navigator.credentials.get()`.
    ///
    /// Returns the assertion response as a JSON string matching the format
    /// expected by the server (same shape as `@simplewebauthn/browser`).
    pub async fn start_authentication(options_json: &str) -> Result<String, String> {
        let opts: serde_json::Value =
            serde_json::from_str(options_json).map_err(|e| format!("invalid JSON: {e}"))?;

        // The server returns {"publicKey": {...}} — unwrap to get the inner options
        let inner_opts = opts.get("publicKey").unwrap_or(&opts);

        // Build the publicKey JS object from the inner options
        let inner_json = serde_json::to_string(inner_opts)
            .map_err(|e| format!("JSON serialization error: {e}"))?;
        let public_key = js_sys::JSON::parse(&inner_json).map_err(js_err)?;

        // Convert challenge: base64url string -> ArrayBuffer
        if let Some(challenge_str) = inner_opts.get("challenge").and_then(|v| v.as_str()) {
            let buf = base64url_to_array_buffer(challenge_str)?;
            js_set(&public_key, "challenge", &buf)?;
        }

        // Convert allowCredentials[].id: base64url string -> ArrayBuffer
        let allow_creds = js_get(&public_key, "allowCredentials")?;
        if !allow_creds.is_undefined() && !allow_creds.is_null() {
            let arr = Array::from(&allow_creds);
            if arr.length() > 0 {
                let converted = convert_credential_descriptors(&allow_creds)?;
                js_set(&public_key, "allowCredentials", &converted)?;
            }
        }

        // Build CredentialRequestOptions = { publicKey }
        let request_options = Object::new();
        js_set(&request_options, "publicKey", &public_key)?;

        // Call navigator.credentials.get(options)
        let window = web_sys::window().ok_or("no window")?;
        let navigator = window.navigator();
        let credentials = navigator.credentials();
        let promise = credentials
            .get_with_options(request_options.unchecked_ref())
            .map_err(js_err)?;
        let result = JsFuture::from(promise).await.map_err(js_err)?;

        if result.is_null() || result.is_undefined() {
            return Err("Authentication was not completed".into());
        }

        // Cast to PublicKeyCredential
        let cred: PublicKeyCredential = result.unchecked_into();

        // Extract fields
        let id = web_sys::Credential::id(&cred);
        let raw_id = cred.raw_id();
        let response: web_sys::AuthenticatorAssertionResponse =
            cred.response().unchecked_into();

        let authenticator_data = response.authenticator_data();
        let client_data_json = web_sys::AuthenticatorResponse::client_data_json(
            response.unchecked_ref(),
        );
        let signature = response.signature();
        let user_handle = response.user_handle();

        // Build the response JSON matching @simplewebauthn/browser format
        let mut resp = serde_json::json!({
            "id": id,
            "rawId": array_buffer_to_base64url(&raw_id),
            "response": {
                "authenticatorData": array_buffer_to_base64url(&authenticator_data),
                "clientDataJSON": array_buffer_to_base64url(&client_data_json),
                "signature": array_buffer_to_base64url(&signature),
            },
            "type": "public-key",
            "clientExtensionResults": {},
        });

        if let Some(uh) = user_handle {
            resp["response"]["userHandle"] = serde_json::Value::String(
                array_buffer_to_base64url(&uh),
            );
        }

        serde_json::to_string(&resp).map_err(|e| format!("JSON serialization error: {e}"))
    }

    /// Start a WebAuthn registration (create a new passkey).
    ///
    /// `options_json` is the `publicKey` creation options from the server,
    /// JSON-serialised. Fields like `challenge`, `user.id`, and
    /// `excludeCredentials[].id` are base64url-encoded strings that will
    /// be converted to `ArrayBuffer` before passing to
    /// `navigator.credentials.create()`.
    ///
    /// Returns the attestation response as a JSON string matching the format
    /// expected by the server (same shape as `@simplewebauthn/browser`).
    pub async fn start_registration(options_json: &str) -> Result<String, String> {
        let opts: serde_json::Value =
            serde_json::from_str(options_json).map_err(|e| format!("invalid JSON: {e}"))?;

        // The server returns {"publicKey": {...}} — unwrap to get the inner options
        let inner_opts = opts.get("publicKey").unwrap_or(&opts);

        // Build the publicKey JS object from the inner options
        let inner_json = serde_json::to_string(inner_opts)
            .map_err(|e| format!("JSON serialization error: {e}"))?;
        let public_key = js_sys::JSON::parse(&inner_json).map_err(js_err)?;

        // Convert challenge: base64url string -> ArrayBuffer
        if let Some(challenge_str) = inner_opts.get("challenge").and_then(|v| v.as_str()) {
            let buf = base64url_to_array_buffer(challenge_str)?;
            js_set(&public_key, "challenge", &buf)?;
        }

        // Convert user.id: base64url string -> ArrayBuffer
        if let Some(user_id_str) = inner_opts
            .get("user")
            .and_then(|u| u.get("id"))
            .and_then(|v| v.as_str())
        {
            let user_obj = js_get(&public_key, "user")?;
            let buf = base64url_to_array_buffer(user_id_str)?;
            js_set(&user_obj, "id", &buf)?;
        }

        // Convert excludeCredentials[].id: base64url string -> ArrayBuffer
        let exclude_creds = js_get(&public_key, "excludeCredentials")?;
        if !exclude_creds.is_undefined() && !exclude_creds.is_null() {
            let converted = convert_credential_descriptors(&exclude_creds)?;
            js_set(&public_key, "excludeCredentials", &converted)?;
        }

        // Build CredentialCreationOptions = { publicKey }
        let create_options = Object::new();
        js_set(&create_options, "publicKey", &public_key)?;

        // Call navigator.credentials.create(options)
        let window = web_sys::window().ok_or("no window")?;
        let navigator = window.navigator();
        let credentials = navigator.credentials();
        let promise = credentials
            .create_with_options(create_options.unchecked_ref())
            .map_err(js_err)?;
        let result = JsFuture::from(promise).await.map_err(js_err)?;

        if result.is_null() || result.is_undefined() {
            return Err("Registration was not completed".into());
        }

        // Cast to PublicKeyCredential
        let cred: PublicKeyCredential = result.unchecked_into();

        // Extract fields
        let id = web_sys::Credential::id(&cred);
        let raw_id = cred.raw_id();
        let response: web_sys::AuthenticatorAttestationResponse =
            cred.response().unchecked_into();

        let attestation_object = response.attestation_object();
        let client_data_json = web_sys::AuthenticatorResponse::client_data_json(
            response.unchecked_ref(),
        );

        // Collect transports (may not be available in all browsers)
        let transports_arr = response.get_transports();
        let transports: Vec<String> = transports_arr
            .iter()
            .filter_map(|v| v.as_string())
            .collect();

        // Collect optional public key algorithm
        let public_key_algorithm = response.get_public_key_algorithm().ok();

        // Collect optional public key
        let public_key_b64 = response
            .get_public_key()
            .ok()
            .flatten()
            .map(|buf| array_buffer_to_base64url(&buf));

        // Collect optional authenticator data
        let authenticator_data_b64 = response
            .get_authenticator_data()
            .ok()
            .map(|buf| array_buffer_to_base64url(&buf));

        // Build the response JSON matching @simplewebauthn/browser format
        let mut resp_inner = serde_json::json!({
            "attestationObject": array_buffer_to_base64url(&attestation_object),
            "clientDataJSON": array_buffer_to_base64url(&client_data_json),
        });

        if !transports.is_empty() {
            resp_inner["transports"] = serde_json::json!(transports);
        }
        if let Some(alg) = public_key_algorithm {
            resp_inner["publicKeyAlgorithm"] = serde_json::json!(alg);
        }
        if let Some(pk) = public_key_b64 {
            resp_inner["publicKey"] = serde_json::Value::String(pk);
        }
        if let Some(ad) = authenticator_data_b64 {
            resp_inner["authenticatorData"] = serde_json::Value::String(ad);
        }

        let resp = serde_json::json!({
            "id": id,
            "rawId": array_buffer_to_base64url(&raw_id),
            "response": resp_inner,
            "type": "public-key",
            "clientExtensionResults": {},
        });

        serde_json::to_string(&resp).map_err(|e| format!("JSON serialization error: {e}"))
    }
}

// ---------------------------------------------------------------------------
// Non-WASM stubs (server-side rendering)
// ---------------------------------------------------------------------------
#[cfg(not(target_arch = "wasm32"))]
mod inner {
    /// WebAuthn is not available outside a browser environment.
    pub async fn is_webauthn_available() -> bool {
        false
    }

    /// WebAuthn authentication is not available outside a browser environment.
    pub async fn start_authentication(_options_json: &str) -> Result<String, String> {
        Err("WebAuthn is only available in a browser environment".into())
    }

    /// WebAuthn registration is not available outside a browser environment.
    pub async fn start_registration(_options_json: &str) -> Result<String, String> {
        Err("WebAuthn is only available in a browser environment".into())
    }
}

// Re-export the public API from the appropriate inner module.
pub use inner::*;
