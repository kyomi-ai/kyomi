// SPDX-License-Identifier: AGPL-3.0-or-later

//! Shared auth token refresh utility.
//!
//! When the access_token cookie expires (15min lifetime), server functions
//! return "Authentication required" / "Unauthorized" errors. This module
//! provides a centralized refresh mechanism that calls `/api/v1/auth/refresh`
//! using the still-valid refresh_token cookie, then reloads the page so all
//! Resources retry with fresh cookies.

/// Returns `true` if the error message indicates an expired/missing access token
/// that can potentially be fixed by refreshing.
pub fn is_auth_error(msg: &str) -> bool {
    msg.contains("Authentication required") || msg.contains("Unauthorized")
}

/// Attempt to refresh the access token by calling the REST refresh endpoint.
///
/// On success, reloads the page so all server function Resources re-execute
/// with the fresh access_token cookie. On failure (e.g. refresh_token also
/// expired), redirects to the login page.
///
/// This is a client-only operation (JS fetch + window.location).
#[cfg(target_arch = "wasm32")]
pub fn refresh_and_reload() {
    leptos::task::spawn_local(async {
        let refreshed = try_refresh().await;

        if let Some(window) = web_sys::window() {
            if refreshed {
                // Reload the current page — fresh cookies will be sent automatically
                let _ = window.location().reload();
            } else {
                // Refresh token is also invalid — redirect to login
                let _ = window.location().set_href("/login");
            }
        }
    });
}

/// Call `/api/v1/auth/refresh` and return whether it succeeded.
#[cfg(target_arch = "wasm32")]
async fn try_refresh() -> bool {
    use wasm_bindgen::prelude::*;

    let promise = js_sys::Function::new_no_args(
        "return fetch('/api/v1/auth/refresh', { method: 'POST', credentials: 'include' }).then(r => r.ok)",
    )
    .call0(&JsValue::NULL);

    match promise {
        Ok(val) => {
            if let Ok(promise) = val.dyn_into::<js_sys::Promise>() {
                wasm_bindgen_futures::JsFuture::from(promise)
                    .await
                    .ok()
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false)
            } else {
                false
            }
        }
        Err(_) => false,
    }
}
