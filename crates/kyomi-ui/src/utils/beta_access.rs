// SPDX-License-Identifier: AGPL-3.0-or-later

//! Shared "I have beta access" attestation state (KYO-499).
//!
//! Two surfaces gate a Google-account-allowlist-dependent action behind a
//! "beta access" attestation checkbox: the BigQuery `kyomi_oauth` notice in
//! `pages/settings/datasources.rs` and the pre-auth Google sign-in notice in
//! `pages/auth/login.rs`. The React original
//! (`AuthModeSelector.jsx` at `ee16f48a^`) persisted this checkbox to
//! `localStorage["hasBetaAccess"]` — read on mount inside a `try`/`catch`
//! that falls back to `false`, written on every change, and kept in sync
//! across tabs/mounts via a `window.addEventListener('storage', ...)`
//! listener — so a user doesn't have to re-tick it every time they open the
//! modal or reload the login page.
//!
//! KYO-478 shipped the login page's copy of this checkbox *without*
//! persistence, on the reasoning that persisting it would make the
//! attestation "look real but be invisibly pre-satisfied." That reasoning
//! was wrong: React always persisted it, the attestation was never a
//! security control on either surface (Google's own allowlist is the actual
//! enforcement — see `bq_kyomi_oauth_connect_allowed`'s doc comment in
//! `pages/settings/datasources.rs`), and shipping it non-persisted was itself
//! the copy/behavior divergence KYO-499 exists to fix. This module is the
//! ONE place that reads, writes, and subscribes to the flag, so the two
//! surfaces can't drift apart the way they did before (KYO-477/478).

/// The `localStorage` key, matching the React original verbatim.
#[cfg(target_arch = "wasm32")]
const STORAGE_KEY: &str = "hasBetaAccess";

/// Pure parse of the raw `localStorage` string into the checkbox's boolean
/// state — split out of [`read_beta_access`] so it's directly unit-testable
/// on the host target even though `localStorage` itself is WASM-only.
/// Mirrors React's `localStorage.getItem('hasBetaAccess') === 'true'`
/// inside a `try { } catch { return false }`: any value other than the
/// literal string `"true"` — including `None` (key never set) and a
/// corrupted/unexpected value — reads as `false`, never as an error.
pub fn parse_stored_flag(raw: Option<&str>) -> bool {
    raw == Some("true")
}

/// Read the current "I have beta access" attestation from `localStorage`.
/// Returns `false` on the server, or if storage is unavailable/blocked —
/// matches React's try/catch fallback.
#[cfg(target_arch = "wasm32")]
pub fn read_beta_access() -> bool {
    let raw = web_sys::window()
        .and_then(|w| w.local_storage().ok().flatten())
        .and_then(|s| s.get_item(STORAGE_KEY).ok().flatten());
    parse_stored_flag(raw.as_deref())
}

/// Server-side stub — always `false`. `DatasourceModal` and `LoginPage`
/// both call this unconditionally at signal-creation time (see their own
/// doc comments), so the non-wasm arm must exist rather than being cfg'd
/// out of existence, matching `kyomi-ui-components`'s `theme.rs` pattern.
#[cfg(not(target_arch = "wasm32"))]
pub fn read_beta_access() -> bool {
    false
}

/// Persist the "I have beta access" attestation to `localStorage`. A no-op
/// on the server.
#[cfg(target_arch = "wasm32")]
pub fn write_beta_access(value: bool) {
    if let Some(storage) = web_sys::window().and_then(|w| w.local_storage().ok().flatten()) {
        let _ = storage.set_item(STORAGE_KEY, if value { "true" } else { "false" });
    }
}

#[cfg(not(target_arch = "wasm32"))]
pub fn write_beta_access(_value: bool) {}

/// Install a `storage` event listener that re-reads the flag and reports it
/// to `on_change` whenever it changes in another tab or window — the same
/// role React's `window.addEventListener('storage', ...)` played, keeping
/// the datasource modal and the login page (and multiple tabs of either) in
/// sync without a page reload. The `storage` event only fires in *other*
/// browsing contexts than the one that wrote the value (per spec), which is
/// exactly the case this exists to cover — a same-tab write already updates
/// its own signal directly at the call site.
///
/// Returns a `FnOnce()` cleanup, same convention as
/// `utils::oauth_popup::install_oauth_listener` — call it (e.g. via
/// `on_cleanup`) to remove the listener and release the backing JS closure.
///
/// # Usage
/// ```rust,ignore
/// let cleanup = install_beta_access_listener(move |value| {
///     set_access_confirmed.try_set(value);
/// });
/// on_cleanup(move || cleanup());
/// ```
#[cfg(target_arch = "wasm32")]
pub fn install_beta_access_listener(on_change: impl Fn(bool) + 'static) -> impl FnOnce() {
    use wasm_bindgen::prelude::*;
    use wasm_bindgen::JsCast;

    let closure = Closure::<dyn Fn(web_sys::Event)>::new(move |_event: web_sys::Event| {
        on_change(read_beta_access());
    });

    let listener_fn = closure.as_ref().unchecked_ref::<js_sys::Function>().clone();

    if let Some(window) = web_sys::window() {
        let _ = window.add_event_listener_with_callback("storage", &listener_fn);
    }

    move || {
        if let Some(window) = web_sys::window() {
            let _ = window.remove_event_listener_with_callback("storage", &listener_fn);
        }
        drop(closure);
    }
}

/// Shared `mailto:` target for both surfaces' "Request beta access" link
/// (KYO-499). Neither surface can reach the React original's `/beta-signup`
/// page — it was never ported (`apps/frontend/src/pages/BetaSignup.jsx`,
/// 363 lines, posts to `/api/v1/subscribe`; porting it is out of scope
/// here) — and the two surfaces need ONE identical target so they can't
/// drift the way KYO-477/478 did. `pages/auth/login.rs` is pre-auth (no
/// `Layout` context at all) and already used this address;
/// `pages/settings/datasources.rs` previously opened the in-app
/// `FeedbackModal` instead, which login can't reach — mailto is the only
/// target reachable from both. KYO-504 later removed that `FeedbackModal`
/// access-request wiring entirely, since this was its only caller.
///
/// Hardcoded rather than read from `kyomi_core::Config::support_email`
/// (which defaults to this same address — see `config.rs`): `Config` lives
/// on `ServerContext`, `#[cfg(feature = "ssr")]`-only, and does not exist on
/// either surface's `hydrate`/wasm32 build. Keep this literal in sync with
/// `config.rs`'s default until a client-reachable config server fn exists.
pub const BETA_ACCESS_REQUEST_HREF: &str = "mailto:support@kyomi.ai?subject=Request%20Beta%20Access";

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::extract_between;

    // ── `parse_stored_flag` — pure predicate, directly testable on host ──

    #[test]
    fn parse_stored_flag_true_only_for_the_literal_string_true() {
        assert!(
            parse_stored_flag(Some("true")),
            "the literal string \"true\" must parse as attested — matches React's \
             `localStorage.getItem('hasBetaAccess') === 'true'`"
        );
    }

    #[test]
    fn parse_stored_flag_false_when_never_set() {
        assert!(
            !parse_stored_flag(None),
            "a key that was never written (localStorage.getItem returns null) must \
             read as false, not panic or default true"
        );
    }

    #[test]
    fn parse_stored_flag_false_for_any_other_value() {
        for bogus in ["false", "TRUE", "1", "", "true "] {
            assert!(
                !parse_stored_flag(Some(bogus)),
                "only the exact literal \"true\" may parse as attested — {bogus:?} must \
                 read as false, matching React's strict `=== 'true'` check (no truthy \
                 coercion)"
            );
        }
    }

    // ── `BETA_ACCESS_REQUEST_HREF` — pins the real support domain ────────
    //
    // Ported from a check that used to live in `pages/auth/login.rs`'s own
    // test module, back when the mailto href was inlined there directly.
    // Now that both surfaces reference this one constant instead of their
    // own literal, the domain-pinning check belongs here, once.

    #[test]
    fn beta_access_request_href_targets_the_real_support_domain() {
        // Pins the real support domain (kyomi.ai, not kyomi.dev — confirmed
        // against kyomi_core::Config::support_email's default in
        // config.rs). A prior draft of this link pointed at a domain Kyomi
        // doesn't own, which is the exact dead end this link exists to
        // prevent: a user who can't sign in clicks "Request beta access"
        // and mails an address that bounces or goes nowhere.
        assert!(
            BETA_ACCESS_REQUEST_HREF.starts_with("mailto:support@kyomi.ai"),
            "BETA_ACCESS_REQUEST_HREF must point at support@kyomi.ai — the address \
             kyomi_core::Config::support_email defaults to — found: \
             {BETA_ACCESS_REQUEST_HREF:?}"
        );
        assert!(
            !BETA_ACCESS_REQUEST_HREF.contains("kyomi.dev"),
            "BETA_ACCESS_REQUEST_HREF must not use the kyomi.dev domain — Kyomi's \
             support address is on kyomi.ai (see config.rs)"
        );
    }

    // ── Structural guard: both surfaces share this module ────────────────
    //
    // `localStorage` itself is WASM-only and can't be exercised on the host
    // target — the behavior that CAN'T be host-tested is documented in the
    // KYO-499 implementation report. What CAN be pinned here is that both
    // consuming surfaces route through this one module's public API rather
    // than reimplementing the read/write/subscribe logic inline, which is
    // exactly how the two surfaces drifted apart before (KYO-477/478).
    //
    // Every assertion below is scoped to the actual notice markup via
    // `extract_between` — never a raw `include_str!(...).contains(...)`
    // over the whole file. An unscoped whole-file scan is satisfied by ANY
    // occurrence of the literal anywhere in the file, including this
    // module's own test assertions (`include_str!` pulls in the file's
    // `#[cfg(test)]` block too) and doc comments elsewhere in the
    // production file that happen to quote the same copy in prose. Both
    // failure modes were demonstrated by mutation during KYO-499 review:
    // mutating the real checkbox span left `both_surfaces_use_the_same_checkbox_label`
    // passing because `datasources.rs` had a doc comment quoting the same
    // string; mutating the real `href` left
    // `both_surfaces_link_to_the_same_shared_target` passing because
    // `login.rs`'s own scoped test assertion (which necessarily quotes the
    // constant name) satisfied the whole-file scan regardless of what the
    // markup rendered. Scoping to the notice block — the same
    // `extract_between` pattern `pages/settings/datasources/tests/oauth.rs`
    // and `pages/auth/login.rs`'s own test module already use — excludes
    // both the test module and any comment outside the notice itself.

    const DATASOURCES_SRC: &str = include_str!("../pages/settings/datasources.rs");
    const LOGIN_SRC: &str = include_str!("../pages/auth/login.rs");

    /// The datasource modal's kyomi_oauth notice block — same bounds
    /// `oauth.rs`'s own tests use, so this can only match the real
    /// attestation markup, never the surrounding function body (where the
    /// signal's doc comment lives) or the test module.
    fn datasources_notice_block() -> &'static str {
        extract_between(
            DATASOURCES_SRC,
            "<Show when=move || bq_auth_mode.get() == \"kyomi_oauth\">",
            "<Show when=move || bq_auth_mode.get() == \"enterprise_oauth\">",
        )
    }

    /// The login page's Google sign-in notice block — same bounds
    /// `login.rs`'s own test module uses.
    fn login_notice_block() -> &'static str {
        extract_between(LOGIN_SRC, "<Show when=show_google_section>", "</Show>")
    }

    #[test]
    fn both_surfaces_use_the_shared_beta_access_module() {
        for (name, block) in [
            ("pages/settings/datasources.rs", datasources_notice_block()),
            ("pages/auth/login.rs", login_notice_block()),
        ] {
            assert!(
                block.contains("beta_access::"),
                "{name}'s notice block must read/write the beta-access attestation \
                 through utils::beta_access rather than a hand-rolled copy of the \
                 localStorage logic (KYO-499) — this is exactly how the two \
                 surfaces drifted apart before (KYO-477/478)"
            );
        }
    }

    #[test]
    fn both_surfaces_use_the_same_checkbox_label() {
        for (name, block) in [
            ("pages/settings/datasources.rs", datasources_notice_block()),
            ("pages/auth/login.rs", login_notice_block()),
        ] {
            assert!(
                block.contains("\"I have beta access\""),
                "{name}'s notice block must use the exact checkbox label \"I have \
                 beta access\" (KYO-499, restoring the React original's copy) — \
                 found no match"
            );
        }
    }

    #[test]
    fn both_surfaces_link_to_the_same_shared_target() {
        for (name, block) in [
            ("pages/settings/datasources.rs", datasources_notice_block()),
            ("pages/auth/login.rs", login_notice_block()),
        ] {
            assert!(
                block.contains("beta_access::BETA_ACCESS_REQUEST_HREF"),
                "{name}'s notice block must link \"Request beta access\" via the \
                 shared beta_access::BETA_ACCESS_REQUEST_HREF constant, not a \
                 hardcoded/independent href that could silently diverge from the \
                 other surface (KYO-499)"
            );
        }
    }

    #[test]
    fn both_surfaces_use_the_same_link_text() {
        for (name, block) in [
            ("pages/settings/datasources.rs", datasources_notice_block()),
            ("pages/auth/login.rs", login_notice_block()),
        ] {
            assert!(
                block.contains("\"Request beta access\""),
                "{name}'s notice block must use the exact link text \"Request beta \
                 access\" (KYO-499, restoring the React original's copy) — found no \
                 match"
            );
        }
    }
}
