//! The datasource **list**'s own Connect/Reconnect button (`DatasourceCard`)
//! routes through `list_connect_action` — a thin wrapper around
//! `oauth_url_for_datasource` that lets the click handler dispatch on a
//! `ListConnectAction` (`LaunchPopup`/`Unsupported`) instead of branching on
//! an empty-string sentinel itself (KYO-442).
//!
//! This file used to also cover the KYO-408/KYO-499 Google-OAuth-allowlist
//! attestation gate `list_connect_action` folded in alongside the URL
//! choice — KYO-705
//! removed that gate entirely (Kyomi's Google OAuth app left Testing
//! publishing status, so Google no longer refuses un-allowlisted accounts
//! and there is nothing left to attest to). The attestation-specific tests
//! were deleted with it; what remains below covers `list_connect_action`'s
//! URL-routing behavior, which is unrelated to that gate and survives the
//! removal unchanged.
//!
//! KYO-704 then retired the BigQuery `kyomi_oauth` auth mode itself: an
//! explicit `Some("kyomi_oauth")` (a legacy row on disk nobody has
//! re-saved) and a null `auth_mode` (which now resolves to the new
//! default, `"service_account"`) both resolve to
//! `ListConnectAction::Unsupported`, since neither mode has an OAuth flow
//! to launch.

use super::super::{ListConnectAction, list_connect_action, oauth_url_for_datasource};
use super::{SRC, extract_between};

// ── `list_connect_action` — URL routing per datasource type/mode ────────

#[test]
fn kyomi_oauth_resolves_to_unsupported_not_the_retired_google_popup() {
    // KYO-704 retired the kyomi_oauth auth mode — it no longer grants
    // credentials, so oauth_url_for_datasource now resolves an explicit
    // `Some("kyomi_oauth")` (still possible on a pre-KYO-704 row nobody
    // has re-saved) to the empty no-OAuth-flow sentinel, the same as any
    // other non-OAuth mode.
    let action = list_connect_action("bigquery", "my-ds", Some("kyomi_oauth"));
    assert_eq!(
        action,
        ListConnectAction::Unsupported,
        "a legacy kyomi_oauth Connect click must resolve to Unsupported, not launch the \
         now-dead Google OAuth popup — got {action:?}"
    );
}

#[test]
fn null_auth_mode_resolves_to_unsupported_because_it_resolves_to_service_account() {
    // The load-bearing detail this ticket calls out: `auth_mode: None` must
    // resolve to the SAME effective mode that oauth_url_for_datasource uses
    // for its URL choice — BIGQUERY_DEFAULT_AUTH_MODE, now "service_account"
    // (KYO-704; it used to be "kyomi_oauth"). The action below is
    // Unsupported because "service_account" itself has no OAuth flow — not
    // because BigQuery has no OAuth at all; enterprise_oauth still launches
    // a popup (see `enterprise_oauth_launches_its_own_popup` below).
    let action = list_connect_action("bigquery", "my-ds", None);
    assert_eq!(
        action,
        ListConnectAction::Unsupported,
        "a BigQuery row with auth_mode: None must resolve to the same effective mode as \
         oauth_url_for_datasource (\"service_account\", which has no OAuth flow) — \
         got {action:?}"
    );
}

#[test]
fn enterprise_oauth_launches_its_own_popup() {
    let action = list_connect_action("bigquery", "my-ds", Some("enterprise_oauth"));
    assert_eq!(
        action,
        ListConnectAction::LaunchPopup(
            "/api/v1/auth/oauth/bigquery-enterprise/connect?datasource_slug=my-ds".to_string()
        ),
        "bigquery enterprise_oauth must launch its own popup — got {action:?}"
    );
}

#[test]
fn non_bigquery_oauth_providers_launch_their_own_popups() {
    let cases: &[(&str, Option<&str>, &str)] = &[
        (
            "snowflake",
            None,
            "/api/v1/auth/oauth/snowflake/connect?datasource_slug=my-ds",
        ),
        (
            "databricks",
            Some("oauth"),
            "/api/v1/auth/oauth/databricks/connect?datasource_slug=my-ds",
        ),
        (
            "synapse",
            Some("enterprise_oauth"),
            "/api/v1/auth/oauth/microsoft-enterprise/connect?datasource_slug=my-ds",
        ),
    ];
    for (ds_type, auth_mode, expected_url) in cases {
        let action = list_connect_action(ds_type, "my-ds", *auth_mode);
        assert_eq!(
            action,
            ListConnectAction::LaunchPopup((*expected_url).to_string()),
            "{ds_type} must launch its own popup — got {action:?}"
        );
    }
}

#[test]
fn unknown_datasource_type_is_unsupported() {
    let action = list_connect_action("postgres", "my-ds", None);
    assert_eq!(
        action,
        ListConnectAction::Unsupported,
        "a datasource type with no OAuth connect endpoint must resolve to Unsupported — \
         got {action:?}"
    );
}

/// The refactor must not alter any URL: for every case that still resolves
/// to `LaunchPopup`, the URL must be byte-identical to what
/// `oauth_url_for_datasource` alone would produce for the same inputs —
/// `list_connect_action` calls it, rather than re-deriving the URL, so this
/// pins that it never diverges.
///
/// KYO-704 retired BigQuery kyomi_oauth, so `oauth_url_for_datasource` now
/// resolves every BigQuery case except enterprise_oauth to the empty
/// no-OAuth-flow sentinel — the popup-URL correspondence above is only half
/// of what `list_connect_action` promises. The other half is asserted
/// below: wherever `oauth_url_for_datasource` yields that empty sentinel,
/// `list_connect_action` must resolve to `Unsupported`, not silently to
/// `LaunchPopup` — without this second direction, a regression that turned
/// a non-popup case back into a `LaunchPopup` would go unnoticed.
#[test]
fn launch_popup_url_always_matches_oauth_url_for_datasource() {
    // Cases that still resolve to LaunchPopup: the URL must be
    // byte-identical to oauth_url_for_datasource's own output.
    let popup_cases: &[(&str, Option<&str>)] = &[
        ("bigquery", Some("enterprise_oauth")),
        ("snowflake", None),
        ("databricks", Some("oauth")),
        ("synapse", Some("enterprise_oauth")),
    ];
    for (ds_type, auth_mode) in popup_cases {
        let action = list_connect_action(ds_type, "my-ds", *auth_mode);
        let expected_url = oauth_url_for_datasource(ds_type, "my-ds", *auth_mode);
        assert!(
            !expected_url.is_empty(),
            "test bug: {ds_type}/{auth_mode:?} belongs in popup_cases only if \
             oauth_url_for_datasource actually returns a URL for it"
        );
        assert_eq!(
            action,
            ListConnectAction::LaunchPopup(expected_url),
            "{ds_type}/{auth_mode:?} — list_connect_action's URL must be byte-identical \
             to oauth_url_for_datasource's own output, not a re-derived copy that could \
             silently diverge from it"
        );
    }

    // Cases where oauth_url_for_datasource yields the empty sentinel: the
    // action must be the exact non-popup variant, not merely "anything but
    // LaunchPopup".
    let non_popup_cases: &[(&str, Option<&str>, ListConnectAction)] = &[
        // Legacy "kyomi_oauth" on disk (a pre-KYO-704 row nobody has
        // re-saved): the mode itself has no OAuth flow left.
        (
            "bigquery",
            Some("kyomi_oauth"),
            ListConnectAction::Unsupported,
        ),
        // auth_mode: None resolves to the new default, "service_account",
        // which has no OAuth flow either.
        ("bigquery", None, ListConnectAction::Unsupported),
    ];
    for (ds_type, auth_mode, expected_action) in non_popup_cases {
        let action = list_connect_action(ds_type, "my-ds", *auth_mode);
        let url = oauth_url_for_datasource(ds_type, "my-ds", *auth_mode);
        assert!(
            url.is_empty(),
            "test bug: {ds_type}/{auth_mode:?} belongs in non_popup_cases only if \
             oauth_url_for_datasource returns the empty sentinel for it — got {url:?}"
        );
        assert_eq!(
            &action, expected_action,
            "{ds_type}/{auth_mode:?} — expected {expected_action:?}, got {action:?}"
        );
    }
}

// ── Source-marker guard: on_oauth_click must dispatch on list_connect_action ──

/// Regression guard for the actual KYO-442 bug: `on_oauth_click` must
/// decide via `list_connect_action` (the single function that owns the URL
/// choice), not call `oauth_url_for_datasource` directly. Anchored on the
/// function-call sites themselves (not on any UI copy), matching
/// docs/standards/testing/anchor-source-text-markers-on-code-not-copy.md —
/// both markers are structure a future edit would have to touch
/// deliberately, not text a copy change could delete as a side effect.
#[test]
fn on_oauth_click_dispatches_on_list_connect_action_not_the_url_helper_directly() {
    let body = extract_between(
        SRC,
        "let on_oauth_click = move |_: leptos::ev::MouseEvent| {",
        "view! {\n                <Button",
    );
    assert!(
        body.contains("list_connect_action("),
        "on_oauth_click must dispatch on list_connect_action's ListConnectAction — found \
         no call site in: {body:?}"
    );
    assert!(
        !body.contains("oauth_url_for_datasource("),
        "on_oauth_click must not call oauth_url_for_datasource directly — doing so would \
         bypass list_connect_action's ListConnectAction dispatch (KYO-442) — found a \
         direct call in: {body:?}"
    );
}
