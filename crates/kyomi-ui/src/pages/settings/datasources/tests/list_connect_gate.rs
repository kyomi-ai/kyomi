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

use super::super::{list_connect_action, oauth_url_for_datasource, ListConnectAction};
use super::{extract_between, SRC};

// ── `list_connect_action` — URL routing per datasource type/mode ────────

#[test]
fn kyomi_oauth_launches_the_google_popup() {
    let action = list_connect_action("bigquery", "my-ds", Some("kyomi_oauth"));
    assert_eq!(
        action,
        ListConnectAction::LaunchPopup("/api/v1/auth/google-oauth/connect".to_string()),
        "a kyomi_oauth Connect click must launch the Google OAuth popup — got {action:?}"
    );
}

#[test]
fn kyomi_oauth_null_auth_mode_resolves_to_the_same_url_as_explicit_kyomi_oauth() {
    // The load-bearing detail this ticket calls out: `auth_mode: None` must
    // resolve to the SAME effective mode (BIGQUERY_DEFAULT_AUTH_MODE =
    // "kyomi_oauth") that oauth_url_for_datasource uses for its URL choice.
    let action = list_connect_action("bigquery", "my-ds", None);
    assert_eq!(
        action,
        ListConnectAction::LaunchPopup("/api/v1/auth/google-oauth/connect".to_string()),
        "a BigQuery row with auth_mode: None must resolve to the same URL as an explicit \
         kyomi_oauth row — got {action:?}"
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

/// `list_connect_action` calls `oauth_url_for_datasource` rather than
/// re-deriving the URL, so this pins that it never diverges.
#[test]
fn launch_popup_url_always_matches_oauth_url_for_datasource() {
    let cases: &[(&str, Option<&str>)] = &[
        ("bigquery", Some("kyomi_oauth")),
        ("bigquery", Some("enterprise_oauth")),
        ("bigquery", None),
        ("snowflake", None),
        ("databricks", Some("oauth")),
        ("synapse", Some("enterprise_oauth")),
    ];
    for (ds_type, auth_mode) in cases {
        let action = list_connect_action(ds_type, "my-ds", *auth_mode);
        let ListConnectAction::LaunchPopup(url) = action else {
            panic!(
                "expected {ds_type}/{auth_mode:?} to resolve to LaunchPopup for this \
                 assertion to be meaningful — got {action:?}"
            );
        };
        assert_eq!(
            url,
            oauth_url_for_datasource(ds_type, "my-ds", *auth_mode),
            "{ds_type}/{auth_mode:?} — list_connect_action's URL must be byte-identical \
             to oauth_url_for_datasource's own output, not a re-derived copy that could \
             silently diverge from it"
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
