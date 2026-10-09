// SPDX-License-Identifier: AGPL-3.0-or-later

use leptos::prelude::*;

use crate::pages::auth::components::GoogleSignInButton;

/// The "Sign in with Google" section: the visibility gate, the button, and
/// the wrapper markup around it — the part of the Google option that both
/// the login and signup forms need identically.
///
/// The loading signal, the disabled signal, and the click handler (which
/// redirects the browser to `/api/v1/auth/google/login`) are owned by the
/// caller and passed in as props. `login.rs` wires both its `CredentialsView`
/// and `SignupView` call sites to the same `google_loading` signal and
/// `on_google_click` handler defined once in `LoginPage`, so there is one
/// Google OAuth redirect flow in the codebase, not a copy per view (KYO-728).
///
/// Deliberately does not render any divider that may sit between this section
/// and what follows it. Whether a divider appears, and on what condition, is
/// part of the caller's own layout: `CredentialsView` renders two, each gated
/// on a different combination of its passkey- and Google-section visibility,
/// while `SignupView` renders none. A divider prop here would have to change
/// shape every time a caller's layout did, so ownership stays with the caller.
#[component]
pub fn GoogleSignInSection(
    /// Whether Google OAuth is enabled for this deployment
    /// (`auth_config.google_oauth`) — nothing renders when this is false.
    show: impl Fn() -> bool + Copy + Send + Sync + 'static,
    #[prop(into)] loading: Signal<bool>,
    /// True while a competing auth method (e.g. passkey, on the login form)
    /// is mid-flight, so the two can't be triggered concurrently. Callers
    /// with no competing method pass the default (`false`).
    #[prop(into, default = Signal::stored(false))] disabled: Signal<bool>,
    on_click: Callback<()>,
) -> impl IntoView {
    view! {
        <Show when=show>
            <div class="space-y-3">
                <GoogleSignInButton loading=loading disabled=disabled on_click=on_click/>
            </div>
        </Show>
    }
}
