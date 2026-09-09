// SPDX-License-Identifier: AGPL-3.0-or-later

use leptos::prelude::*;
use phosphor_leptos::Icon;

use crate::components::{Alert, AlertDescription, AlertVariant, Checkbox};
use crate::pages::auth::components::GoogleSignInButton;
use crate::utils::beta_access;

/// Google sign-in section — the `GoogleSignInButton` plus the KYO-478/499
/// beta-access attestation notice (an `Alert` with an inline "Request beta
/// access" link and a confirmation checkbox).
///
/// Extracted out of `login.rs`'s `CredentialsView` (KYO-683 Phase 2) so
/// `SignupView` can offer Google sign-in too without duplicating the notice
/// markup — the whole reason the notice exists (Kyomi's Google OAuth app is
/// in Testing mode; see the constant's own doc comment) applies identically
/// on both surfaces, so it must say and gate the same thing on both. Before
/// this extraction the notice existed only inside `CredentialsView`; the
/// `google_access_confirmed` signal and its `localStorage["hasBetaAccess"]`
/// persistence are still owned by the caller (both `CredentialsView` and
/// `SignupView` share the same `LoginPage`-level signal) so ticking the box
/// on one view carries over to the other, matching KYO-499's requirement
/// that this and the datasource modal's identical notice never drift.
///
/// `disabled` is computed by the caller (via `google_sign_in_disabled`) so
/// each view's own mutual-exclusion rules (e.g. CredentialsView disabling
/// Google while passkey sign-in is loading) stay in the caller, not baked
/// into this shared shell.
#[component]
pub fn GoogleSignInSection(
    #[prop(into)] loading: Signal<bool>,
    #[prop(into)] disabled: Signal<bool>,
    /// Whether the user has ticked "I have beta access" — shared across
    /// every surface that renders this section via `utils::beta_access`.
    google_access_confirmed: ReadSignal<bool>,
    /// Setter for the checkbox above.
    set_google_access_confirmed: WriteSignal<bool>,
    on_click: Callback<()>,
) -> impl IntoView {
    view! {
        <div class="space-y-3">
            <GoogleSignInButton loading=loading disabled=disabled on_click=on_click/>
            <Alert variant=AlertVariant::Warning>
                <Icon icon=phosphor_leptos::WARNING_CIRCLE attr:class="h-4 w-4" />
                <AlertDescription>
                    <p class="mb-3">
                        "Google sign-in requires beta access. "
                        <a
                            href=beta_access::BETA_ACCESS_REQUEST_HREF
                            class="text-primary hover:underline font-medium"
                        >
                            "Request beta access"
                        </a>
                    </p>
                    <label class="flex items-center gap-2 cursor-pointer">
                        <Checkbox
                            checked=Signal::derive(move || google_access_confirmed.get())
                            on_change=Callback::new(move |v: bool| {
                                // KYO-499 — persist to localStorage["hasBetaAccess"]
                                // alongside the in-memory signal, same as the
                                // pre-extraction call site.
                                beta_access::write_beta_access(v);
                                set_google_access_confirmed.set(v)
                            })
                        />
                        <span class="text-sm">
                            "I have beta access"
                        </span>
                    </label>
                </AlertDescription>
            </Alert>
        </div>
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Tests (KYO-683 — moved here from login.rs's content-oriented assertions,
// see login.rs's own test module for the wiring/gating assertions that stay
// there)
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use crate::test_support::extract_between;

    const SRC: &str = include_str!("google_section.rs");

    /// The KYO-478/499 notice must render inside `GoogleSignInSection`
    /// itself (rather than each caller re-typing it), and must still say
    /// what it always said: mirrors the pre-extraction
    /// `google_sign_in_checkbox_renders_inside_show_google_section_block`
    /// test in `login.rs`, whose job (guard the notice's copy/link/checkbox)
    /// moved here when the markup did. `login.rs` keeps a companion test
    /// asserting that every `<GoogleSignInSection` call site sits inside a
    /// `<Show when=show_google_section>` block, which is the "never render
    /// ungated" half of the original test's intent.
    #[test]
    fn notice_copy_and_checkbox_present() {
        let component_block = extract_between(
            SRC,
            "pub fn GoogleSignInSection(",
            "\n}\n\n// ",
        );
        assert!(
            component_block.contains("requires beta access"),
            "GoogleSignInSection must render the KYO-499 access notice sentence"
        );
        assert!(
            component_block.contains("\"Request beta access\""),
            "the notice must include a \"Request beta access\" link (KYO-499 copy)"
        );
        assert!(
            component_block.contains("beta_access::BETA_ACCESS_REQUEST_HREF"),
            "the \"Request beta access\" link must point at the shared \
             utils::beta_access::BETA_ACCESS_REQUEST_HREF target (KYO-499), not an \
             independently hardcoded mailto href that could silently diverge"
        );
        assert!(
            component_block.contains("\"I have beta access\""),
            "the notice must render the KYO-499 confirmation checkbox with the exact \
             copy \"I have beta access\""
        );
        assert!(
            component_block.contains("<GoogleSignInButton"),
            "sanity check on the extract_between bounds: the block must still contain \
             the Google sign-in button itself"
        );
    }
}
