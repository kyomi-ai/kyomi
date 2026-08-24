//! The datasource **list**'s own Connect/Reconnect button (`DatasourceCard`)
//! must be gated by the same KYO-408/KYO-499 beta-access attestation the
//! settings modal already enforces (KYO-427/KYO-477) — before this ticket
//! it called `oauth_url_for_datasource` directly and launched the same
//! Google OAuth popup with no gate and no allowlist notice anywhere on the
//! list surface. `list_connect_action` folds the gate and the URL choice
//! into one pure, unit-testable function so a click handler calling them
//! separately can't let the two drift (KYO-442).

use super::super::{list_connect_action, oauth_url_for_datasource, ListConnectAction};
use super::{extract_between, SRC};

// ── `list_connect_action` — the gate must actually block the URL, not just
// exist alongside it ────────────────────────────────────────────────────

#[test]
fn kyomi_oauth_not_attested_opens_modal_not_the_gated_google_popup() {
    let action = list_connect_action("bigquery", "my-ds", Some("kyomi_oauth"), false);
    // Checked ahead of the pinning `assert_eq!` below (not after — a
    // `!contains`/mismatch check placed after an `assert_eq!` on the same
    // value is dead, since the equality already decides it either way; see
    // docs/standards/testing/contains-after-assert-eq-is-dead.md) so this
    // failure mode gets its own diagnostic if the gate regresses to a
    // `LaunchPopup` that happens to carry the real Google URL.
    assert!(
        !matches!(
            &action,
            ListConnectAction::LaunchPopup(url) if url == "/api/v1/auth/google-oauth/connect"
        ),
        "an unconfirmed kyomi_oauth Connect click must never launch the gated Google \
         OAuth popup at /api/v1/auth/google-oauth/connect — got {action:?}"
    );
    assert_eq!(
        action,
        ListConnectAction::OpenModal,
        "an unconfirmed kyomi_oauth Connect click must route into the settings modal \
         instead, where the allowlist notice, the attestation checkbox, and the \
         \"Request beta access\" link live (KYO-442) — got {action:?}"
    );
}

#[test]
fn kyomi_oauth_null_auth_mode_still_opens_modal_not_a_silent_bypass() {
    // The load-bearing detail this ticket calls out: `auth_mode: None` must
    // resolve to the SAME effective mode (BIGQUERY_DEFAULT_AUTH_MODE =
    // "kyomi_oauth") that oauth_url_for_datasource uses for its URL choice.
    // If the gate used a different default, a row with a null auth_mode
    // would silently skip the attestation gate while still producing the
    // gated Google URL underneath it.
    let action = list_connect_action("bigquery", "my-ds", None, false);
    assert_eq!(
        action,
        ListConnectAction::OpenModal,
        "a BigQuery row with auth_mode: None and no attestation must open the modal, \
         not bypass the gate by resolving to a different effective mode than \
         oauth_url_for_datasource does — got {action:?}"
    );
}

#[test]
fn kyomi_oauth_attested_launches_the_google_popup() {
    let action = list_connect_action("bigquery", "my-ds", Some("kyomi_oauth"), true);
    assert_eq!(
        action,
        ListConnectAction::LaunchPopup("/api/v1/auth/google-oauth/connect".to_string()),
        "once the attestation is confirmed, the Connect click must launch the real \
         Google OAuth popup exactly as before this ticket — got {action:?}"
    );
}

#[test]
fn enterprise_oauth_launches_unchanged_regardless_of_attestation() {
    // enterprise_oauth is a different endpoint with its own per-datasource
    // consent flow — the KYO-477 Connect gate is defined as bigquery +
    // kyomi_oauth only, and must stay that way here too.
    let action = list_connect_action("bigquery", "my-ds", Some("enterprise_oauth"), false);
    assert_eq!(
        action,
        ListConnectAction::LaunchPopup(
            "/api/v1/auth/oauth/bigquery-enterprise/connect?datasource_slug=my-ds".to_string()
        ),
        "bigquery enterprise_oauth must launch its popup unchanged, ungated by the \
         kyomi_oauth attestation — got {action:?}"
    );
}

#[test]
fn non_bigquery_oauth_providers_launch_unchanged_regardless_of_attestation() {
    // Acceptance criterion 3: snowflake, databricks, and synapse have no
    // kyomi_oauth-style attestation gate at all and must launch their
    // popups exactly as before this ticket, attested or not.
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
        let action = list_connect_action(ds_type, "my-ds", *auth_mode, false);
        assert_eq!(
            action,
            ListConnectAction::LaunchPopup((*expected_url).to_string()),
            "{ds_type} has no attestation gate and must launch its popup unchanged \
             even when not attested — got {action:?}"
        );
    }
}

#[test]
fn unknown_datasource_type_is_unsupported() {
    let action = list_connect_action("postgres", "my-ds", None, true);
    assert_eq!(
        action,
        ListConnectAction::Unsupported,
        "a datasource type with no OAuth connect endpoint must resolve to Unsupported \
         even when attested — got {action:?}"
    );
}

/// The refactor must not alter any URL: for every case that still resolves
/// to `LaunchPopup`, the URL must be byte-identical to what
/// `oauth_url_for_datasource` alone would produce for the same inputs —
/// `list_connect_action` calls it, rather than re-deriving the URL, so this
/// pins that it never diverges.
#[test]
fn launch_popup_url_always_matches_oauth_url_for_datasource() {
    let cases: &[(&str, Option<&str>, bool)] = &[
        ("bigquery", Some("kyomi_oauth"), true),
        ("bigquery", Some("enterprise_oauth"), false),
        ("bigquery", None, true),
        ("snowflake", None, false),
        ("databricks", Some("oauth"), false),
        ("synapse", Some("enterprise_oauth"), false),
    ];
    for (ds_type, auth_mode, access_confirmed) in cases {
        let action = list_connect_action(ds_type, "my-ds", *auth_mode, *access_confirmed);
        let ListConnectAction::LaunchPopup(url) = action else {
            panic!(
                "expected {ds_type}/{auth_mode:?}/{access_confirmed} to resolve to \
                 LaunchPopup for this assertion to be meaningful — got {action:?}"
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
/// decide via `list_connect_action` (the single function that folds the
/// gate and the URL choice together), not call `oauth_url_for_datasource`
/// directly and skip the gate the way it did before this ticket. Anchored
/// on the function-call sites themselves (not on any UI copy), matching
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
        "on_oauth_click must dispatch on list_connect_action's ListConnectAction so the \
         KYO-408/KYO-499 attestation gate and the URL choice cannot disagree — found no \
         call site in: {body:?}"
    );
    assert!(
        !body.contains("oauth_url_for_datasource("),
        "on_oauth_click must not call oauth_url_for_datasource directly — doing so would \
         reintroduce the ungated path this ticket closes (KYO-442), bypassing \
         list_connect_action's gate entirely — found a direct call in: {body:?}"
    );
}
