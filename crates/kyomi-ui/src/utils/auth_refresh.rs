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
                // Refresh token is also invalid — redirect to login.
                // Preserve the current path so the user is returned here after login.
                let path = window.location().pathname().unwrap_or_default();
                let login_url = if path.is_empty() || path == "/" {
                    "/login".to_string()
                } else {
                    format!("/login?redirect={path}")
                };
                let _ = window.location().set_href(&login_url);
            }
        }
    });
}

/// Call `/api/v1/auth/refresh` and return whether it succeeded.
///
/// This must NOT build JavaScript from a string (`js_sys::Function::new_*`,
/// `js_sys::eval`): the frontend Content-Security-Policy deliberately omits
/// `'unsafe-eval'` (KYO-849), so the browser throws an uncatchable
/// `EvalError` and the page dies instead of redirecting to `/login`. Use the
/// typed `fetch` API like every other client request. Enforced by
/// `scripts/lint/check-client-eval.sh`.
#[cfg(target_arch = "wasm32")]
pub async fn try_refresh() -> bool {
    use wasm_bindgen::JsCast;
    use wasm_bindgen_futures::JsFuture;

    let Some(window) = web_sys::window() else {
        return false;
    };

    let opts = web_sys::RequestInit::new();
    opts.set_method("POST");
    opts.set_credentials(web_sys::RequestCredentials::Include);

    let Ok(response) =
        JsFuture::from(window.fetch_with_str_and_init("/api/v1/auth/refresh", &opts)).await
    else {
        return false;
    };

    response
        .dyn_into::<web_sys::Response>()
        .map(|resp| resp.ok())
        .unwrap_or(false)
}
