//! OAuth plumbing shared across providers: the status-refetch hook and
//! its per-provider source mapping (KYO-197), the create-mode status
//! fetch predicate (KYO-411), popup-monitor recovery (KYO-437), the
//! kyomi_oauth access-request notice/gate and Google-error translation
//! (KYO-408), the Connect/Reconnect button wiring (KYO-427, corrected
//! to use its own separate gate rather than Save/Create's in KYO-477),
//! and the shared "OAuth not configured" predicate (KYO-519).

use super::{appears_shortly_before, extract_between, SRC};

// ── KYO-197: shared OAuth status re-fetch hook ─────────────────────
//
// The four provider Auth Mode Sections previously each carried their own
// copy-pasted `Effect::new` re-fetching OAuth status on mode change
// (KYO-13, KYO-17), and Synapse's copy was simply missing — it only took
// `ReadSignal`s, so it had no write signals to re-fetch into, and the
// status panel went stale the moment the user switched to
// `enterprise_oauth` after the modal loaded a different mode. The fix
// consolidates the fetch logic into `use_oauth_status_refetch` plus one
// pure mapping function per provider. The mapping functions are the
// substantive coverage below; the two guard tests that follow ensure a
// fifth provider can't reintroduce a hand-rolled copy.

use super::super::{
    bigquery_oauth_source, databricks_oauth_source, snowflake_oauth_source,
    synapse_oauth_source, OAuthStatusSource,
};

#[test]
fn bigquery_oauth_source_maps_documented_modes() {
    assert_eq!(
        bigquery_oauth_source("kyomi_oauth"),
        Some(OAuthStatusSource::GoogleAccount),
        "kyomi_oauth must fetch the account-level Google OAuth status"
    );
    assert_eq!(
        bigquery_oauth_source("enterprise_oauth"),
        Some(OAuthStatusSource::Datasource("bigquery-enterprise")),
        "enterprise_oauth must fetch per-datasource status for the bigquery-enterprise provider key"
    );
    for mode in ["service_account", "password", "keypair", "sql", "service_principal", "nonsense"] {
        assert_eq!(
            bigquery_oauth_source(mode),
            None,
            "non-OAuth or unrecognised mode {mode:?} must not trigger an OAuth status fetch"
        );
    }
}

#[test]
fn snowflake_oauth_source_maps_documented_modes() {
    assert_eq!(
        snowflake_oauth_source("oauth"),
        Some(OAuthStatusSource::Datasource("snowflake")),
        "oauth must fetch per-datasource status for the snowflake provider key"
    );
    for mode in ["service_account", "password", "keypair", "sql", "service_principal", "nonsense"] {
        assert_eq!(
            snowflake_oauth_source(mode),
            None,
            "non-OAuth or unrecognised mode {mode:?} must not trigger an OAuth status fetch \
             (allow-list, not the old password/keypair deny-list — an unrecognised mode is now a no-op)"
        );
    }
}

#[test]
fn databricks_oauth_source_maps_documented_modes() {
    assert_eq!(
        databricks_oauth_source("oauth"),
        Some(OAuthStatusSource::Datasource("databricks")),
        "oauth must fetch per-datasource status for the databricks provider key"
    );
    for mode in ["service_account", "password", "keypair", "sql", "service_principal", "nonsense"] {
        assert_eq!(
            databricks_oauth_source(mode),
            None,
            "non-OAuth or unrecognised mode {mode:?} must not trigger an OAuth status fetch"
        );
    }
}

#[test]
fn synapse_oauth_source_maps_documented_modes() {
    assert_eq!(
        synapse_oauth_source("enterprise_oauth"),
        Some(OAuthStatusSource::Datasource("microsoft-enterprise")),
        "enterprise_oauth must fetch per-datasource status for the microsoft-enterprise provider key \
         — this is the KYO-197 regression: Synapse never re-fetched at all"
    );
    for mode in ["service_account", "password", "keypair", "sql", "service_principal", "nonsense"] {
        assert_eq!(
            synapse_oauth_source(mode),
            None,
            "non-OAuth or unrecognised mode {mode:?} must not trigger an OAuth status fetch"
        );
    }
}

/// Guard against a fifth provider (or a future edit to one of the four)
/// hand-rolling another copy of the re-fetch `Effect::new` instead of
/// calling the shared hook — the exact mistake this ticket fixes.
#[test]
fn auth_mode_sections_use_the_shared_refetch_hook() {
    let sections: &[(&str, &str, &str)] = &[
        ("BigQuery", "fn BigQueryAuthModeSection(", "fn SnowflakeAuthModeSection("),
        ("Snowflake", "fn SnowflakeAuthModeSection(", "fn DatabricksAuthModeSection("),
        ("Databricks", "fn DatabricksAuthModeSection(", "fn SynapseAuthModeSection("),
        ("Synapse", "fn SynapseAuthModeSection(", "struct ConnectionFieldsSignals"),
    ];
    for (name, start, end) in sections {
        let f = extract_between(SRC, start, end);
        assert!(
            f.contains("use_oauth_status_refetch("),
            "{name}AuthModeSection must re-fetch OAuth status via the shared \
             use_oauth_status_refetch hook, not a hand-rolled Effect"
        );
    }
}

/// The fetch-on-mode-change logic must live only in
/// `use_oauth_status_refetch` — none of the four `*AuthModeSection`
/// bodies may call the OAuth status server_fns directly from their own
/// `Effect::new`. If one does, it has silently reverted to the
/// copy-pasted-Effect pattern this ticket removed.
#[test]
fn auth_mode_sections_do_not_hand_roll_oauth_status_effects() {
    let sections: &[(&str, &str, &str)] = &[
        ("BigQuery", "fn BigQueryAuthModeSection(", "fn SnowflakeAuthModeSection("),
        ("Snowflake", "fn SnowflakeAuthModeSection(", "fn DatabricksAuthModeSection("),
        ("Databricks", "fn DatabricksAuthModeSection(", "fn SynapseAuthModeSection("),
        ("Synapse", "fn SynapseAuthModeSection(", "struct ConnectionFieldsSignals"),
    ];
    for (name, start, end) in sections {
        let f = extract_between(SRC, start, end);
        assert!(
            !f.contains("get_google_oauth_status(") && !f.contains("get_datasource_oauth_status("),
            "{name}AuthModeSection must not call the OAuth status server_fns directly — \
             that call belongs solely inside use_oauth_status_refetch"
        );
    }
}

/// Regression guard for the actual KYO-197 bug: `SynapseAuthModeSection`
/// must accept the three `set_oauth_*` write signals so it can call the
/// re-fetch hook at all. Before this fix it took only `ReadSignal`s and
/// therefore could not re-fetch when the mode selector changed.
#[test]
fn synapse_auth_mode_section_accepts_oauth_status_setters() {
    let f = extract_between(SRC, "fn SynapseAuthModeSection(", "struct ConnectionFieldsSignals");
    for setter in [
        "set_oauth_connected: WriteSignal<bool>",
        "set_oauth_email: WriteSignal<Option<String>>",
        "set_oauth_expired: WriteSignal<bool>",
    ] {
        assert!(
            f.contains(setter),
            "SynapseAuthModeSection must accept `{setter}` — without it, the mode-change \
             re-fetch (KYO-197) cannot update the OAuth status panel"
        );
    }
}

// ── KYO-411: create-mode BigQuery kyomi_oauth status fetch ─────────
//
// `use_oauth_status_refetch` skipped its fetch whenever `slug` was
// empty, applying that guard BEFORE consulting `source_for_mode`. That
// suppressed both status sources indiscriminately — correct for
// `Datasource(_)` (needs a real slug) but wrong for `GoogleAccount`
// (an account-level fetch that takes none). BigQuery kyomi_oauth is the
// only mode mapping to `GoogleAccount`, so in create mode a user whose
// Google account was already linked never had `oauth_connected`
// initialised from the server — the panel showed "not connected" with
// a Connect button despite being connected. The fix moves the guard
// into a pure `oauth_status_source_to_fetch` predicate that consults
// `source_for_mode` first and scopes the slug requirement to the
// `Datasource(_)` arm alone.
//
// Simply letting `GoogleAccount` through was not enough on its own: it
// reintroduces the KYO-404 deadlock for exactly the users this fixes,
// because `can_next` keyed only on `test_result`, and an
// already-connected user has nothing left to click that would ever set
// it. `connection_step_satisfied_from` (extended below) is the other
// half — it must also accept an already-established connection, not
// just a freshly completed popup.

use super::super::{connection_step_satisfied_from, oauth_status_source_to_fetch};

/// `oauth_status_source_to_fetch` — the pure predicate behind the
/// KYO-411/KYO-426/KYO-443 guard logic — exercised directly for all
/// four providers' mapping functions. Covers the actual bug
/// (GoogleAccount must run on an empty slug) and the regression risk
/// the ticket calls out by name (the other three providers, all
/// `Datasource(_)`, must stay blocked). `read_slug` is passed as
/// `move || "...".to_string()` here since the function only ever calls
/// it 0 or 1 times per call in these cases.
#[test]
fn oauth_status_source_to_fetch_lets_google_account_through_empty_slug() {
    assert_eq!(
        oauth_status_source_to_fetch(
            "kyomi_oauth",
            true,
            String::new,
            bigquery_oauth_source
        ),
        Some((OAuthStatusSource::GoogleAccount, String::new())),
        "GoogleAccount is an account-level fetch with no slug parameter — it must run \
         in create mode (empty slug) so an already-linked user's status is fetched at \
         all (KYO-411)"
    );
}

#[test]
fn oauth_status_source_to_fetch_blocks_datasource_source_on_empty_slug() {
    assert_eq!(
        oauth_status_source_to_fetch(
            "enterprise_oauth",
            false,
            String::new,
            bigquery_oauth_source
        ),
        None,
        "Datasource(_) sources need a real slug (get_datasource_oauth_status(key, \
         slug)) — an empty slug must not fire this fetch, even for BigQuery, even in \
         edit mode"
    );
    assert_eq!(
        oauth_status_source_to_fetch("oauth", false, String::new, snowflake_oauth_source),
        None,
        "Snowflake's oauth mode maps to Datasource(_) — must stay blocked on an empty \
         slug (regression guard: the other three providers must not change behavior)"
    );
    assert_eq!(
        oauth_status_source_to_fetch("oauth", false, String::new, databricks_oauth_source),
        None,
        "Databricks's oauth mode maps to Datasource(_) — must stay blocked on an empty \
         slug"
    );
    assert_eq!(
        oauth_status_source_to_fetch(
            "enterprise_oauth",
            false,
            String::new,
            synapse_oauth_source
        ),
        None,
        "Synapse's enterprise_oauth mode maps to Datasource(_) — must stay blocked on \
         an empty slug"
    );
}

/// KYO-426's missing case, and the exact reason it shipped: the old
/// guard was "slug is empty", but `slug` auto-generates from the Name
/// field the instant the user types anything — so by the time a create
/// -mode user has typed a name, `slug` is non-empty and the old guard
/// no longer applied. This asserts the *new* guard (`is_create_mode`)
/// blocks the fetch even though the resolved slug here is non-empty,
/// which is precisely the state a real create-mode user is in.
#[test]
fn oauth_status_source_to_fetch_blocks_datasource_source_in_create_mode_with_a_slug() {
    assert_eq!(
        oauth_status_source_to_fetch(
            "enterprise_oauth",
            true,
            || "e2e-bigquery".to_string(),
            bigquery_oauth_source
        ),
        None,
        "a Datasource(_) source must stay blocked in create mode even with a non-empty \
         slug — get_datasource_oauth_status would 500 against a datasource that hasn't \
         been created yet (KYO-426); the empty-slug check alone doesn't catch this \
         because slug auto-generates from Name as soon as the user types"
    );
    assert_eq!(
        oauth_status_source_to_fetch(
            "oauth",
            true,
            || "my-warehouse".to_string(),
            snowflake_oauth_source
        ),
        None,
        "Snowflake's oauth mode maps to Datasource(_) too — must stay blocked in create \
         mode with a non-empty slug, same as BigQuery enterprise_oauth"
    );
    assert_eq!(
        oauth_status_source_to_fetch(
            "oauth",
            true,
            || "my-catalog".to_string(),
            databricks_oauth_source
        ),
        None,
        "Databricks's oauth mode maps to Datasource(_) too — must stay blocked in \
         create mode with a non-empty slug"
    );
    assert_eq!(
        oauth_status_source_to_fetch(
            "enterprise_oauth",
            true,
            || "my-synapse".to_string(),
            synapse_oauth_source
        ),
        None,
        "Synapse's enterprise_oauth mode maps to Datasource(_) too — must stay blocked \
         in create mode with a non-empty slug"
    );
}

/// Edit mode is unaffected by the KYO-426 fix: once a datasource
/// actually exists (`is_create_mode == false`) and has a real slug, the
/// `Datasource(_)` source must resolve exactly as before — and the
/// returned slug must be the one `read_slug` produced, since callers
/// rely on that to avoid reading it twice.
#[test]
fn oauth_status_source_to_fetch_runs_datasource_source_in_edit_mode_with_a_slug() {
    assert_eq!(
        oauth_status_source_to_fetch(
            "oauth",
            false,
            || "my-slug".to_string(),
            snowflake_oauth_source
        ),
        Some((OAuthStatusSource::Datasource("snowflake"), "my-slug".to_string())),
        "edit mode (is_create_mode == false) plus a non-empty slug must let the \
         Datasource(_) source through unchanged, carrying the resolved slug — edit mode \
         is unaffected by KYO-411/KYO-426"
    );
}

#[test]
fn oauth_status_source_to_fetch_passes_through_none_regardless_of_slug() {
    assert_eq!(
        oauth_status_source_to_fetch(
            "service_account",
            false,
            String::new,
            bigquery_oauth_source
        ),
        None,
        "a mode with no OAuth status source at all must stay None on an empty slug"
    );
    assert_eq!(
        oauth_status_source_to_fetch(
            "service_account",
            false,
            || "my-slug".to_string(),
            bigquery_oauth_source
        ),
        None,
        "a mode with no OAuth status source at all must stay None on a non-empty slug too"
    );
}

/// KYO-443's core claim: the account-level (`GoogleAccount`) path
/// resolves identically no matter what `is_create_mode` is, and does so
/// WITHOUT EVER CALLING `read_slug` — proven directly here, not by
/// grepping source text, by handing it a closure that panics if
/// invoked. If this predicate's `GoogleAccount` arm ever grew a slug or
/// create-mode check, this test would fail with that panic rather than
/// a silently-wrong return value, which is what makes it a genuine
/// regression guard for the property `use_oauth_status_refetch`'s
/// `Memo` depends on to avoid resubscribing to `slug` on every
/// keystroke (KYO-443).
#[test]
fn oauth_status_source_to_fetch_google_account_arm_never_calls_read_slug() {
    for is_create_mode in [true, false] {
        let panics_if_called =
            || -> String { panic!("read_slug must not be called for GoogleAccount") };
        assert_eq!(
            oauth_status_source_to_fetch(
                "kyomi_oauth",
                is_create_mode,
                panics_if_called,
                bigquery_oauth_source
            ),
            Some((OAuthStatusSource::GoogleAccount, String::new())),
            "GoogleAccount must resolve the same way regardless of is_create_mode \
             ({is_create_mode}) and must not call read_slug to do it — it is an \
             account-level fetch that takes no slug parameter at all (KYO-443)"
        );
    }
}

/// The `Datasource(_)` arm's create-mode half of the same property:
/// `is_create_mode` must be checked BEFORE `read_slug` is called, not
/// after. Proven the same way as the test above — a closure that
/// panics if invoked — rather than by checking source-text ordering,
/// so a future change to the match arm's internal structure (e.g.
/// hoisting the slug read above the guard) fails this test with the
/// panic itself instead of silently compiling and passing. Without
/// this ordering, a create-mode `Datasource(_)` branch would still
/// correctly return `None` (covered by the create-mode test above) but
/// would have already subscribed `use_oauth_status_refetch`'s `Memo`
/// to `slug`, reintroducing the keystroke-flicker half of KYO-443 for
/// e.g. BigQuery `enterprise_oauth` even though KYO-426's
/// fetch-blocking itself stayed correct.
#[test]
fn oauth_status_source_to_fetch_create_mode_datasource_arm_never_calls_read_slug() {
    let panics_if_called =
        || -> String { panic!("read_slug must not be called in create mode") };
    assert_eq!(
        oauth_status_source_to_fetch(
            "enterprise_oauth",
            true,
            panics_if_called,
            bigquery_oauth_source
        ),
        None,
        "a create-mode Datasource(_) source must resolve to None without ever calling \
         read_slug — is_create_mode must be checked before the slug read (KYO-426, \
         KYO-443)"
    );
}

/// Wiring guard: `use_oauth_status_refetch`'s `fetch_input` `Memo` must
/// resolve its source and slug via the shared `oauth_status_source_to_fetch`
/// predicate rather than reimplementing (or re-inlining) the guard —
/// the unit tests above only cover the extracted predicate itself, and
/// an inlined copy in the Memo could silently diverge from it, which is
/// the exact duplication this file's own KYO-13/KYO-17/KYO-197 history
/// (see `docs/CODING_STANDARDS.md` §Leptos) warns is likely to recur.
#[test]
fn use_oauth_status_refetch_calls_the_shared_guard_predicate() {
    let body = extract_between(SRC, "fn use_oauth_status_refetch(", "fn build_oauth_recovery_callback(");
    assert!(
        body.contains("oauth_status_source_to_fetch(&current_mode, create_mode, move || slug.get(), source_for_mode)"),
        "use_oauth_status_refetch's Memo must resolve its source via \
         oauth_status_source_to_fetch, passing slug lazily via move || slug.get() — \
         inlining the guard again risks reintroducing the KYO-411/KYO-426/KYO-443 bugs \
         invisibly to the unit tests above, which only cover the extracted predicate"
    );
}

// ── KYO-437: OAuth popup recovery ───────────────────────────────────
//
// The connecting spinner used to be cleared *only* by an OAuth
// postMessage — closing the popup, a dropped handshake, or a lost
// notification (KYO-436) all left it spinning forever. The fix has two
// parts that can't both be exercised as plain unit tests: the
// browser-timing half (poll `popup.closed()`, enforce a timeout) lives
// in `oauth_popup::monitor_oauth_popup` and is covered there by pure
// decision-logic tests (`popup_poll_should_report_closed` /
// `popup_timeout_should_report` / the message + timeout-bound tests).
// The wiring half — that every provider's panel actually arms the
// monitor, re-checks status before reporting failure, and tears the
// monitor down on unmount — is what these source-level guards cover.

/// Every `ModalOAuthStatusPanel` call site must wire `on_recover` — this
/// is what the ticket calls out explicitly: fixing recovery only at one
/// provider's call site (e.g. BigQuery) would leave Snowflake,
/// Databricks, and Microsoft/Synapse still spinning forever, since they
/// all share this same component and click handler.
#[test]
fn every_modal_oauth_status_panel_call_site_wires_on_recover() {
    // KYO-455: this test used to scope to `SRC.split(MOD_TESTS_MARKER).next()`
    // because `SRC` was `include_str!`-ed from this same file, and an
    // unscoped count would also match this test's own source (the string
    // literals a few lines below), making the assertion unable to fail no
    // matter what the production code did. Now that the test module lives
    // in `datasources/tests/` rather than inline in `datasources.rs`, `SRC`
    // contains production code only, so counting against it directly is
    // both simpler and correct without the split/expect workaround.
    let panel_call_sites = SRC.matches("<ModalOAuthStatusPanel").count();
    let recover_wirings = SRC.matches("on_recover=on_oauth_recover").count();
    assert_eq!(
        panel_call_sites, 5,
        "expected exactly 5 ModalOAuthStatusPanel call sites (BigQuery kyomi_oauth + \
         enterprise_oauth, Snowflake, Databricks, Synapse/Microsoft) — this test's \
         markers need updating if that count changed intentionally"
    );
    assert_eq!(
        panel_call_sites, recover_wirings,
        "every ModalOAuthStatusPanel call site must pass on_recover=on_oauth_recover — \
         a call site missing it still has KYO-437's spinner-forever bug"
    );
}

/// `ModalOAuthStatusPanel`'s shared `start_connect` handler must arm the
/// popup monitor (not just open the popup) and stash its cleanup for
/// teardown — this is the actual fix for the ticket's core complaint
/// ("the only thing that can ever clear `oauth_connecting` is a
/// `postMessage`"). A regression that reverts to calling only
/// `open_oauth_popup` would compile and look identical in a diff of the
/// success path alone.
#[test]
fn start_connect_arms_the_popup_monitor_and_stashes_cleanup() {
    let body = extract_between(SRC, "let start_connect = move |_: leptos::ev::MouseEvent| {", "view! {\n        {move || {");
    assert!(
        body.contains("monitor_oauth_popup("),
        "start_connect must call monitor_oauth_popup after a successful open_oauth_popup \
         — without it, a closed/abandoned popup never clears oauth_connecting"
    );
    assert!(
        body.contains("on_recover.try_run(outcome)"),
        "the monitor's outcome must be forwarded to on_recover via try_run — this runs in \
         a deferred gloo_timers callback, so a bare .run() would panic if the panel had \
         already unmounted (disposal-safety standard)"
    );
    assert!(
        body.contains("popup_monitor.update_value"),
        "the monitor's cleanup must be stashed in popup_monitor so on_cleanup can stop it \
         on teardown instead of leaking the interval/timeout"
    );
}

// ── KYO-440: list-level popup recovery (the second connect entry point) ──
//
// KYO-437 fixed the spinner-forever bug for `ModalOAuthStatusPanel` only.
// `DatasourceRow`'s own "Connect"/"Reconnect" button — the list-level entry
// point, reached without ever opening the edit modal — called
// `open_oauth_popup` directly and never armed a monitor, so the identical
// bug survived at that call site. The four guards below pin the four ways
// this wiring differs from the modal's (a shared, row-scoped
// `oauth_connecting: Option<String>` instead of a private `bool`; recovery
// re-checks the "datasources" list query instead of an `OAuthStatusSource`;
// and its own `on_cleanup`/second-connect teardown) plus the shared
// "recheck before reporting failure" ordering `ModalOAuthStatusPanel`
// already established.

use super::super::credential_status_indicates_connected;

#[test]
fn credential_status_indicates_connected_matches_the_cred_action_gate() {
    // The two states `DatasourceRow`'s `cred_action` shows an OAuth
    // Connect/Reconnect button for — anything else must read as
    // "connected" here, or a recovered row's own re-check could disagree
    // with the button that prompted the connect attempt in the first
    // place.
    for status in ["missing", "expired"] {
        assert!(
            !credential_status_indicates_connected(status),
            "{status:?} must not be read as connected — DatasourceRow still shows a \
             Connect/Reconnect button for it"
        );
    }
    for status in ["valid", "shared"] {
        assert!(
            credential_status_indicates_connected(status),
            "{status:?} must be read as connected — DatasourceRow shows no \
             Connect/Reconnect button for it"
        );
    }
}

/// Mirrors `start_connect_arms_the_popup_monitor_and_stashes_cleanup`
/// above for the list-level entry point: `on_oauth_click` must arm the
/// popup monitor after a successful `open_popup`, not just open the
/// popup, and stash its cleanup for teardown. A regression that reverts
/// to calling only `open_popup` would compile and look identical in a
/// diff of the success path alone.
#[test]
fn on_oauth_click_arms_the_popup_monitor_and_stashes_cleanup() {
    let body = extract_between(
        SRC,
        "let on_oauth_click = move |_: leptos::ev::MouseEvent| {",
        "view! {\n                <Button",
    );
    assert!(
        body.contains("monitor_oauth_popup("),
        "on_oauth_click must call monitor_oauth_popup after a successful open_popup — \
         without it, a closed/abandoned popup at the list level never clears \
         oauth_connecting (KYO-440)"
    );
    assert!(
        body.contains("popup_monitor.update_value"),
        "the monitor's cleanup must be stashed in popup_monitor so this row's \
         on_cleanup can stop it on teardown instead of leaking the interval/timeout"
    );
}

/// KYO-440 cycle 4 (Option A): the popup monitor's `still_connecting`
/// closure must read this row's own private per-attempt token
/// (`connect_attempt_live`), never the list-shared `oauth_connecting`
/// signal in any form — not even scoped by this row's own id, which is
/// what cycle 3 did and which the cycle-4 review found still reachable by
/// a writer outside this row (the settings modal's own, independent
/// `start_connect`, which never touches `oauth_connecting` at all — see
/// `credential_status_indicates_connected`'s neighbouring doc comment in
/// production for the full trace). A signal read that depends on
/// `oauth_connecting` in any way can be desynced by a writer this row has
/// no way to know about; only a token private to this row's own click
/// handler, drain-before-arm, and `on_cleanup` is immune to all of them at
/// once.
#[test]
fn still_connecting_does_not_read_the_shared_oauth_connecting_signal() {
    let still_connecting = extract_between(
        SRC,
        "let cleanup = monitor_oauth_popup(",
        "move |outcome| {",
    );
    assert!(
        !still_connecting.contains("oauth_connecting"),
        "the still_connecting closure passed to monitor_oauth_popup must not reference \
         oauth_connecting at all — comparing the list-shared signal, even scoped by this \
         row's own id, is reachable by writers outside this row (the settings modal's \
         independent start_connect, and any future writer) that have no reason to know \
         this row's popup even exists (KYO-440 cycle 4) — found: {still_connecting:?}"
    );
    assert!(
        still_connecting.contains("connect_attempt_live"),
        "the still_connecting closure must read this row's own private \
         connect_attempt_live token instead — found: {still_connecting:?}"
    );
}

/// `on_oauth_click`'s click guard and the button's `disabled` prop must
/// both read this row's own `is_connecting` (KYO-440 cycle 4 reverts cycle
/// 3's "any row connecting" widening — see the neighbouring doc comments
/// in production for why that widening's justification collapsed: the
/// settings modal, one of the writers it was meant to guard against, never
/// reads or writes `oauth_connecting` at all, so the wider guard paid a
/// real UX cost — locking every other row's button while any one row
/// connects — for an invariant that never actually held). Concurrent
/// per-row connects are allowed again; correctness no longer depends on
/// this guard at all, since `still_connecting` now reads a token private
/// to each row (see the test above) — this guard is purely a UI throttle.
#[test]
fn on_oauth_click_guard_and_disabled_prop_are_scoped_to_this_rows_state() {
    let guard = extract_between(
        SRC,
        "let on_oauth_click = move |_: leptos::ev::MouseEvent| {",
        "let url = oauth_url_for_datasource(",
    );
    assert!(
        guard.contains("is_connecting.get_untracked()"),
        "on_oauth_click's guard must check is_connecting.get_untracked() (this row's own \
         state) — found guard body: {guard:?}"
    );
    assert!(
        !guard.contains("oauth_connecting.get_untracked().is_some()"),
        "the guard must not compare the shared oauth_connecting signal directly — that \
         was cycle 3's Option B, reverted in cycle 4 because it never closed the gap it \
         was meant to (the settings modal never touches this signal) while still costing \
         every other row's button its availability during a connect"
    );
    assert!(
        appears_shortly_before(
            SRC,
            "disabled=Signal::derive(move || is_connecting.get() || is_deleting.get())",
            "on:click=on_oauth_click",
            200,
        ),
        "the OAuth Connect/Reconnect button's disabled prop must be \
         is_connecting.get() || is_deleting.get() (this row's own state), matching \
         on_oauth_click's guard — not oauth_connecting.get().is_some(), which would \
         disable every row's button while any one row is connecting even though \
         concurrent per-row connects are supported again (KYO-440 cycle 4)"
    );
}

/// `on_cleanup` must actually *call* the stashed popup-monitor cleanup
/// for this row, not merely drop it — same reasoning as
/// `popup_monitor_cleanup_is_invoked_not_merely_dropped_on_teardown`
/// below, pinned separately here because `DatasourceRow` stashes and
/// tears down its own monitor rather than sharing `ModalOAuthStatusPanel`'s.
/// A row unmounting mid-connect (the list refetches and re-renders
/// constantly) must stop the timers immediately, not on the next poll
/// tick.
#[test]
fn datasource_row_on_cleanup_calls_the_stashed_popup_monitor_cleanup() {
    let row = extract_between(SRC, "fn DatasourceRow(", "fn DatasourceModal(");
    let cleanup_block = extract_between(row, "on_cleanup(move || {", "\n    });");
    assert!(
        cleanup_block.contains("cleanup.take()()"),
        "DatasourceRow's on_cleanup must call the stashed cleanup (SendWrapper::take() \
         then invoke it), not just drop it — a dropped-not-called Box<dyn FnOnce()> \
         leaves the popup's gloo_timers Interval/Timeout still armed, since they're kept \
         alive by the running JS timers rather than by this Box (KYO-440)"
    );
}

/// The list row has no `fetch_oauth_status_once` / `OAuthStatusSource`
/// wiring like `ModalOAuthStatusPanel` — its only source of truth is the
/// "datasources" list query, so recovery must re-fetch that list and
/// inspect this row's own credential_status *before* it can report a
/// failure, mirroring `recovery_callback_rechecks_status_before_reporting_failure`
/// below. A recovered connection (postMessage lost, OAuth actually
/// succeeded) must adopt the recovered state — invalidate the cache, no
/// error toast — never fall through to reporting cancelled/timed out.
#[test]
fn on_oauth_click_recovery_rechecks_datasources_before_reporting_failure() {
    let recovery = extract_between(SRC, "move |outcome| {", "popup_monitor.update_value");
    let fetch_pos = recovery
        .find("list_datasources()")
        .expect("recovery must call list_datasources() to re-check this row's status");
    let report_pos = recovery
        .find("popup_monitor_outcome_message(")
        .expect("recovery must build a failure message via popup_monitor_outcome_message");
    assert!(
        fetch_pos < report_pos,
        "the status recheck (list_datasources) must be positioned — and therefore run — \
         before the failure message is ever built, so a recovered connection never gets \
         reported as cancelled/timed out (KYO-440)"
    );

    let success_arm = extract_between(recovery, "if recovered {", "} else {");
    assert!(
        !success_arm.contains("toast_error"),
        "a recovered connection must not show an error toast — the OAuth handshake \
         actually succeeded, only the postMessage notifying this row was lost (KYO-440)"
    );
    assert!(
        success_arm.contains("query_cache.invalidate(\"datasources\")"),
        "a recovered connection must invalidate the datasources query cache, adopting \
         the recovered state exactly as the list-level postMessage success handler does \
         (KYO-440)"
    );
}

/// KYO-524: this diff (KYO-440) introduced a duplicate-toast regression —
/// `on_outcome` fires from the popup monitor regardless of whether an
/// OAuth `postMessage` already resolved this attempt, because
/// `still_connecting` (`connect_attempt_live`) is deliberately private to
/// this row and never observes that message (see
/// `still_connecting_does_not_read_the_shared_oauth_connecting_signal`
/// above). Without a check, a genuine error already toasted accurately by
/// the list-level listener would get a second, false "connection
/// cancelled." toast on top of it once this row's popup closed itself.
/// The fix reads the list-shared `oauth_connecting` signal — cleared to
/// `None` by that listener the instant ANY recognized postMessage arrives
/// — and, when `None`, skips the recovery fetch and both toast branches
/// entirely rather than merely suppressing one of them.
#[test]
fn on_outcome_says_nothing_when_a_postmessage_already_resolved_the_attempt() {
    let recovery = extract_between(SRC, "move |outcome| {", "popup_monitor.update_value");

    let guard_pos = recovery
        .find("oauth_connecting.try_get_untracked().flatten().is_none()")
        .expect(
            "on_outcome must check whether the list-shared oauth_connecting signal is None \
             — None means a postMessage already resolved this attempt and the listener \
             already gave the user an accurate toast, so recovering again would be at best \
             redundant and at worst a false toast over a real one (KYO-524)",
        );
    let return_pos = recovery[guard_pos..]
        .find("return;")
        .map(|i| guard_pos + i)
        .expect(
            "the oauth_connecting-is-None check must actually early-return out of the \
             closure — merely reading the signal without acting on it fixes nothing \
             (KYO-524)",
        );
    let fetch_pos = recovery
        .find("list_datasources()")
        .expect("recovery must still call list_datasources() for the Some(_) case");

    assert!(
        guard_pos < return_pos && return_pos < fetch_pos,
        "the None-check and its early return must both be positioned — and therefore run — \
         strictly before the list_datasources() recovery fetch, so an attempt already \
         resolved by a postMessage never reaches the fetch or either toast branch at all \
         (KYO-524); found guard at {guard_pos}, return at {return_pos}, fetch at {fetch_pos}"
    );
}

/// Companion to the test above: the KYO-524 "was this already resolved?"
/// check must live in the RECOVERY closure (`on_outcome`) only, never
/// migrate into the MONITORING closure (`still_connecting`) passed as
/// `monitor_oauth_popup`'s second argument. The two closures answer
/// deliberately different questions — see the doc comment on the guard in
/// production — and `still_connecting` must stay a pure read of this
/// row's own private `connect_attempt_live` so the monitor keeps running
/// to completion no matter what an external writer does to the shared
/// signal (the cycle-3 property, pinned separately above). Reading
/// `oauth_connecting` from `still_connecting` instead would let an
/// unrelated writer stop this row's OWN monitor early, reintroducing
/// cycle 3's bug in a new shape.
#[test]
fn oauth_connecting_check_lives_in_the_outcome_closure_not_the_still_connecting_closure() {
    let still_connecting = extract_between(
        SRC,
        "let cleanup = monitor_oauth_popup(",
        "move |outcome| {",
    );
    assert!(
        !still_connecting.contains("try_get_untracked().flatten().is_none()"),
        "the KYO-524 already-resolved check must live in the on_outcome closure, not in \
         still_connecting — still_connecting must stay a pure read of connect_attempt_live \
         alone (the cycle-3 property), never touching oauth_connecting — found in \
         still_connecting body: {still_connecting:?}"
    );

    let recovery = extract_between(SRC, "move |outcome| {", "popup_monitor.update_value");
    assert!(
        recovery.contains("oauth_connecting.try_get_untracked().flatten().is_none()"),
        "the KYO-524 check must live in the on_outcome closure — found: {recovery:?}"
    );
}

/// `on_oauth_click`'s click guard is scoped to this row's own state again
/// (KYO-440 cycle 4 reverted the "any row connecting" widening — see
/// `on_oauth_click_guard_and_disabled_prop_are_scoped_to_this_rows_state`
/// above), so a *different* row's click can freely overwrite the shared
/// `oauth_connecting` display signal at any time. That can make THIS
/// row's own guard read "not connecting" — since it only compares against
/// this row's id — and let a re-click through while this row's own
/// earlier popup and monitor are still unresolved: drain-before-arm is
/// what actually stops that superseded attempt's `Interval`/`Timeout`,
/// making it load-bearing rather than merely defensive. It must do so
/// synchronously in the same `on:click` handler that arms the new
/// monitor, since wasm32 is single-threaded and no tick of a superseded
/// monitor's poll can land between the drain and the new monitor
/// replacing it in the slot. This pins that ordering directly.
#[test]
fn on_oauth_click_drains_the_previous_monitor_before_installing_a_new_one() {
    let body = extract_between(
        SRC,
        "let on_oauth_click = move |_: leptos::ev::MouseEvent| {",
        "view! {\n                <Button",
    );
    let drain_pos = body.find("if let Some(previous) = slot.take() {").expect(
        "on_oauth_click must drain popup_monitor's previously-stashed cleanup before \
         installing a new one — load-bearing since a different row's click can freely \
         overwrite the shared oauth_connecting display signal and defeat this row's own \
         click guard (KYO-440 cycle 4)",
    );
    let invoke_pos = body.find("previous.take()()").expect(
        "the drained cleanup must actually be invoked, not merely dropped — dropping a \
         Box<dyn FnOnce()> does nothing, since the interval/timeout closures are kept alive \
         by the running JS timers, not by this Box, so only calling the cleanup reaches into \
         those and cancels them (KYO-440)",
    );
    let install_pos = body
        .find("*slot = Some(send_wrapper::SendWrapper::new(")
        .expect("on_oauth_click must install the new monitor's cleanup into popup_monitor");
    assert!(
        drain_pos < invoke_pos && invoke_pos < install_pos,
        "the previous monitor's cleanup must be found AND invoked strictly before the new \
         monitor's cleanup is installed into the same slot — found the drain at byte \
         {drain_pos}, the invocation at {invoke_pos}, and the install at {install_pos}; out \
         of order here means a stale monitor from a superseded attempt on this row can \
         outlive the one that replaces it, and later misfire a false cancelled/timed-out \
         report over a genuinely in-flight second attempt (KYO-440)"
    );
}

/// `on_cleanup` must actually *call* the stashed popup-monitor cleanup,
/// not merely drop it — dropping a `Box<dyn FnOnce()>` without invoking
/// it does nothing (unlike a raw `gloo_timers` handle, whose `Drop` impl
/// itself clears the timer): the interval/timeout closures are kept
/// alive by the running JS timers, not by this Box, so only *calling*
/// the cleanup reaches into those and cancels them.
#[test]
fn popup_monitor_cleanup_is_invoked_not_merely_dropped_on_teardown() {
    let panel_body = extract_between(
        SRC,
        "fn ModalOAuthStatusPanel(",
        "\n// ─────────────────────────────────────────────────────────────────────────────\n// OAuth Status Re-fetch Hook",
    );
    assert!(
        panel_body.contains("on_cleanup(move || {"),
        "ModalOAuthStatusPanel must register an on_cleanup for the popup monitor"
    );
    // End marker matches the outer `on_cleanup(...)` call's closing
    // "});" specifically (4-space indent) rather than the first "});"
    // in the body, which would be the inner `update_value(...)` call's.
    let cleanup_block = extract_between(panel_body, "on_cleanup(move || {", "\n    });");
    assert!(
        cleanup_block.contains("cleanup.take()()"),
        "on_cleanup must call the stashed cleanup (SendWrapper::take() then invoke it), \
         not just drop it — a dropped-not-called Box<dyn FnOnce()> leaves the popup's \
         gloo_timers Interval/Timeout still armed, since they're kept alive by the \
         running JS timers rather than by this Box"
    );
}

/// `build_oauth_recovery_callback` must re-check OAuth status
/// (`fetch_oauth_status_once`) *before* it can report a failure
/// (`popup_monitor_outcome_message`) — this is the KYO-436 half of the
/// ticket: a lost postMessage after a real successful link must be
/// discovered and adopted, not reported as cancelled/timed out.
/// Asserting the textual order (not just that both calls exist)
/// specifically guards against a regression that re-checks status
/// *after* already deciding to show the error.
#[test]
fn recovery_callback_rechecks_status_before_reporting_failure() {
    let body = extract_between(
        SRC,
        "fn build_oauth_recovery_callback(",
        "\n/// Maps BigQuery's auth mode",
    );
    let fetch_pos = body
        .find("fetch_oauth_status_once(")
        .expect("build_oauth_recovery_callback must call fetch_oauth_status_once");
    let report_pos = body
        .find("popup_monitor_outcome_message(")
        .expect("build_oauth_recovery_callback must call popup_monitor_outcome_message");
    assert!(
        fetch_pos < report_pos,
        "the status recheck (fetch_oauth_status_once) must be positioned — and therefore \
         run — before the failure message is ever built, so a recovered connection never \
         gets reported as cancelled/timed out"
    );
    assert!(
        body.contains("Some(status) if status.connected =>"),
        "a recheck that finds the account connected must be adopted, not just discarded"
    );
}

/// KYO-426 for the recovery path specifically: `build_oauth_recovery_callback`
/// fires after a connect attempt resolves without a postMessage, which is
/// reachable in create mode too (e.g. a user opens BigQuery
/// enterprise_oauth's popup, closes it, all before the datasource is
/// created). Without threading `is_create_mode` into the same
/// `oauth_status_source_to_fetch` guard `use_oauth_status_refetch` uses,
/// this recheck would fire `get_datasource_oauth_status` against a
/// datasource that doesn't exist yet and hit the identical 500 the
/// mode-change path was fixed for.
#[test]
fn build_oauth_recovery_callback_threads_is_create_mode_into_the_shared_guard() {
    let body = extract_between(
        SRC,
        "fn build_oauth_recovery_callback(",
        "\n/// Maps BigQuery's auth mode",
    );
    assert!(
        body.contains("is_create_mode: Signal<bool>"),
        "build_oauth_recovery_callback must accept is_create_mode so its recheck can \
         apply the same create-mode guard use_oauth_status_refetch does (KYO-426)"
    );
    assert!(
        body.contains("oauth_status_source_to_fetch(") && body.contains("create_mode,"),
        "build_oauth_recovery_callback must pass its resolved create_mode value into \
         oauth_status_source_to_fetch — passing a hardcoded false or omitting the value \
         entirely would silently reintroduce KYO-426 for the recovery path alone, since \
         the mode-change path's own guard is a separate Memo and wouldn't catch it"
    );
    assert!(
        body.contains("move || slug.get_untracked()"),
        "build_oauth_recovery_callback must pass slug into oauth_status_source_to_fetch \
         lazily via move || slug.get_untracked() — this callback already runs outside \
         any reactive scope, so this doesn't change tracking behavior, but the accessor \
         shape must match oauth_status_source_to_fetch's lazy-accessor signature \
         (KYO-443)"
    );
}

/// `connection_step_satisfied_from` — extended by KYO-411 — must accept
/// an OAuth connection that was already established before the modal
/// opened (via the status fetch fixed above), not only one just
/// completed through the popup. Without this, letting the status fetch
/// run is actively harmful: the panel would show "Connected" with no
/// popup left to run, `test_result` would never be set, and Next would
/// stay permanently disabled — a narrower recurrence of the KYO-404
/// deadlock, scoped to exactly the already-linked users this ticket is
/// meant to help.
#[test]
fn connection_step_satisfied_from_accepts_an_already_connected_kyomi_oauth_account() {
    assert!(
        connection_step_satisfied_from("bigquery", "kyomi_oauth", true, false),
        "an already-connected Google account must satisfy the Connection step for \
         BigQuery kyomi_oauth even with no test_result yet (KYO-411)"
    );
    assert!(
        connection_step_satisfied_from("bigquery", "kyomi_oauth", false, true),
        "test_result success must still satisfy the step independently of \
         oauth_connected — this is the KYO-404 popup path and must not regress"
    );
    assert!(
        !connection_step_satisfied_from("bigquery", "kyomi_oauth", false, false),
        "with neither a live connection nor a test_result, kyomi_oauth must not be \
         satisfied — only enterprise_oauth gets the unconditional precreate exception"
    );
    assert!(
        !connection_step_satisfied_from("bigquery", "service_account", true, false),
        "the connected-account exception must not leak into service_account, which has \
         no OAuth connection concept at all"
    );
    assert!(
        !connection_step_satisfied_from("snowflake", "kyomi_oauth", true, false),
        "the connected-account exception is BigQuery-specific — a non-bigquery type \
         must not be satisfied even if its auth_mode string happened to be \
         \"kyomi_oauth\""
    );
}

// ── KYO-404: BigQuery create-mode OAuth deadlock ────────────────────
//
// BigQuery datasources could not be created at all. The create-mode
// "Next" button gates on `test_result.success`, but BigQuery's OAuth
// success arm never wrote `test_result` — BigQuery has no Test &
// Discover step; the OAuth handshake itself is the verification — so
// `test_result` stayed `None` forever and "Next" never enabled. The fix
// has three independent parts, each guarded by one test below: the
// OAuth success arm now writes `test_result`/`discovery_status` itself;
// the kyomi_oauth connect panel is no longer hidden in create mode (so
// there is something to click that produces the write above); and the
// create-mode `can_next` gate gained a narrow bigquery-enterprise_oauth
// exception for the one mode that still cannot produce a `test_result`
// before save.

/// The `GoogleSuccess | BigqueryEnterpriseSuccess` arm of the modal-level
/// OAuth `postMessage` listener must itself write `test_result` and
/// `discovery_status` to a success state. Before KYO-404 this arm only
/// updated the OAuth connected/email/expired/connecting signals, so
/// nothing in the BigQuery kyomi_oauth flow (which never calls
/// `do_test_and_discover`) ever produced a `test_result`, permanently
/// disabling the create-mode "Next" button.
///
/// Bounds: the extraction runs from the exact arm pattern
/// `OAuthMessage::GoogleSuccess { email }` up to the next arm's pattern,
/// `OAuthMessage::SnowflakeSuccess { email }`, so it captures only this
/// arm's body. This matters because both signals are also written
/// elsewhere in the file for unrelated reasons: `do_test_and_discover`
/// (called by the very next arm, for Snowflake/Databricks) writes both
/// on success, and the `test_action` Effect earlier in the file writes
/// both directly — a whole-file or whole-listener `contains` check would
/// pass even with this specific arm fully reverted. The assertion that
/// the arm does NOT call `do_test_and_discover` pins that the extracted
/// span is this arm and not a slice that swallowed the next one.
#[test]
fn google_oauth_success_arm_sets_test_result_and_discovery_status() {
    let arm = extract_between(
        SRC,
        "OAuthMessage::GoogleSuccess { email }",
        "OAuthMessage::SnowflakeSuccess { email }",
    );
    assert!(
        !arm.contains("do_test_and_discover"),
        "sanity check on the extraction bounds: this arm must not reach into the \
         next (Snowflake/Databricks) arm's do_test_and_discover() call"
    );
    assert!(
        arm.contains("set_test_result.try_set(Some(TestConnectionResult {"),
        "the GoogleSuccess | BigqueryEnterpriseSuccess arm must set test_result to \
         Some(...) on success — without it, test_result stays None forever for \
         BigQuery kyomi_oauth, which never runs Test & Discover, permanently \
         disabling the create-mode Next button (KYO-404)"
    );
    assert!(
        arm.contains("set_discovery_status.try_set(\"success\""),
        "the GoogleSuccess | BigqueryEnterpriseSuccess arm must also set \
         discovery_status to \"success\" alongside test_result"
    );
}

// ── KYO-408: BigQuery kyomi_oauth access-request notice + gate ─────────
//
// Kyomi's shared Google OAuth app only accepts Google accounts a Kyomi
// admin has manually added as testers in the Cloud Console — Kyomi has
// no programmatic access to that list, so Google is the only
// enforcement layer. The notice/checkbox added here are NOT a security
// control; they exist purely to make the user request access before
// burning a doomed OAuth round-trip. Three things are covered below:
// the notice renders only for kyomi_oauth, the gate itself (a pure
// predicate, `bq_kyomi_oauth_access_gate_satisfied`) is exercised
// directly, and its wiring into the Save/Create buttons (but
// deliberately NOT the Next button) is pinned by source-text checks.
//
// Copy and link target were rewritten in KYO-499 to restore parity with
// the React original (`AuthModeSelector.jsx` at `ee16f48a^`) — the
// version that shipped in KYO-408/435/477 diverged into a heading + two
// explanatory paragraphs + a standalone <Button> opening the in-app
// FeedbackModal, none of which React did. See `utils::beta_access` for
// the shared copy/persistence/link-target module both this notice and
// the login page's equivalent (`pages/auth/login.rs`) now read through.

use super::super::bq_kyomi_oauth_access_gate_satisfied;

/// The KYO-408/KYO-499 notice (Alert + inline "Request beta access" link +
/// confirmation checkbox) must render inside the kyomi_oauth `<Show>` block
/// and must not leak into enterprise_oauth or service_account — neither of
/// those modes has a Kyomi-side Google-account allowlist to request
/// access to (enterprise_oauth uses the customer's own OAuth app;
/// service_account has no OAuth at all).
#[test]
fn kyomi_oauth_notice_renders_only_for_kyomi_oauth_mode() {
    let kyomi_block = extract_between(
        SRC,
        "<Show when=move || bq_auth_mode.get() == \"kyomi_oauth\">",
        "<Show when=move || bq_auth_mode.get() == \"enterprise_oauth\">",
    );
    assert!(
        kyomi_block.contains("This authentication method requires beta access."),
        "the kyomi_oauth block must render the KYO-499 access notice sentence"
    );
    assert!(
        kyomi_block.contains("\"Request beta access\""),
        "the kyomi_oauth notice must include a \"Request beta access\" link (KYO-499 \
         copy, restoring the React original)"
    );
    assert!(
        kyomi_block.contains("beta_access::BETA_ACCESS_REQUEST_HREF"),
        "the \"Request beta access\" link must point at the shared \
         utils::beta_access::BETA_ACCESS_REQUEST_HREF target (KYO-499) — not a \
         hardcoded href or the removed FeedbackAccessRequestHandle context, which \
         login.rs can never reach pre-auth"
    );
    assert!(
        kyomi_block.contains("\"I have beta access\""),
        "the kyomi_oauth block must render the KYO-499 confirmation checkbox with the \
         exact copy \"I have beta access\", restoring the React original's wording \
         (KYO-408's \"I have requested access and had it confirmed\" was rejected as a \
         divergence from React — see KYO-499)"
    );

    let enterprise_block = extract_between(
        SRC,
        "<Show when=move || bq_auth_mode.get() == \"enterprise_oauth\">",
        "<Show when=move || bq_auth_mode.get() == \"service_account\">",
    );
    assert!(
        !enterprise_block.contains("requires beta access"),
        "the KYO-408/499 notice must not leak into enterprise_oauth — that mode uses \
         the customer's own Google Cloud OAuth app, which has no Kyomi allowlist to \
         request access to"
    );
    assert!(
        !enterprise_block.contains("\"I have beta access\""),
        "the KYO-499 confirmation checkbox must not leak into enterprise_oauth"
    );

    let service_account_block = extract_between(
        SRC,
        "<Show when=move || bq_auth_mode.get() == \"service_account\">",
        "fn SnowflakeAuthModeSection(",
    );
    assert!(
        !service_account_block.contains("requires beta access"),
        "the KYO-408/499 notice must not leak into service_account — that mode has no \
         OAuth flow at all, so an OAuth-allowlist notice would be nonsensical there"
    );
    assert!(
        !service_account_block.contains("\"I have beta access\""),
        "the KYO-499 confirmation checkbox must not leak into service_account"
    );
}

// ── KYO-499: "Request beta access" is an inline link, not a <Button> ───
//
// KYO-435 had promoted this control from an inline text link to a
// standalone <Button> on its own line, reasoning the inline link was
// "effectively invisible" against the amber Warning background. KYO-499
// reverts that: React's original rendered it as a plain inline link
// spliced mid-sentence (`text-primary hover:underline font-medium`), and
// restoring exact parity with React was this ticket's explicit goal —
// see the KYO-499 ticket for the full "how we diverged" writeup. This
// test pins the reverted shape so a future edit can't silently
// reintroduce the standalone-<Button> layout without a deliberate
// decision to do so.

/// Scoped to the same kyomi_oauth `<Show>` block as
/// `kyomi_oauth_notice_renders_only_for_kyomi_oauth_mode` above — bounded
/// by the next mode's `<Show>` opening tag — so a match here can only be
/// the production markup for this one panel, not a sibling panel's
/// textually similar markup.
#[test]
fn request_beta_access_control_is_an_inline_link_not_a_button() {
    let kyomi_block = extract_between(
        SRC,
        "<Show when=move || bq_auth_mode.get() == \"kyomi_oauth\">",
        "<Show when=move || bq_auth_mode.get() == \"enterprise_oauth\">",
    );
    assert!(
        kyomi_block.contains("<a\n") || kyomi_block.contains("<a "),
        "KYO-499: \"Request beta access\" must render as a plain inline <a> link \
         (React's own shape — text-primary hover:underline font-medium, spliced \
         mid-sentence), not a <Button>/<ButtonLink> — found no <a> tag in the \
         kyomi_oauth block"
    );
    assert!(
        !kyomi_block.contains("<Button") && !kyomi_block.contains("<ButtonLink"),
        "KYO-499: the standalone <Button>/<ButtonLink> control KYO-435 introduced must \
         be gone — the request-access control reverted to an inline link matching React"
    );
    assert!(
        kyomi_block.contains("beta_access::write_beta_access(v)"),
        "KYO-499: ticking the checkbox must persist to localStorage via \
         beta_access::write_beta_access, not merely update the in-memory signal — found \
         no call in the kyomi_oauth block"
    );
}

/// `bq_kyomi_oauth_access_gate_satisfied` — the pure predicate behind
/// the KYO-408 Save/Create gate — exercised directly rather than via
/// the view tree. Covers: blocked when unchecked and unconnected;
/// released by ticking the checkbox; released by an already-successful
/// OAuth connection even with the checkbox unchecked (so a returning,
/// already-authorized user is never nagged); and a no-op (always
/// satisfied) for every other datasource type / BigQuery auth mode.
#[test]
fn bq_kyomi_oauth_access_gate_blocks_unchecked_and_releases_when_checked() {
    // Blocked: BigQuery kyomi_oauth, not connected, checkbox unchecked.
    assert!(
        !bq_kyomi_oauth_access_gate_satisfied("bigquery", "kyomi_oauth", false, false),
        "the gate must block Save/Create for BigQuery kyomi_oauth when neither \
         connected nor confirmed"
    );

    // Released by the checkbox alone.
    assert!(
        bq_kyomi_oauth_access_gate_satisfied("bigquery", "kyomi_oauth", false, true),
        "ticking the confirmation checkbox must release the gate even without a live \
         OAuth connection"
    );

    // Released by a real OAuth connection alone — the whole point of
    // not nagging a user who already has proven access.
    assert!(
        bq_kyomi_oauth_access_gate_satisfied("bigquery", "kyomi_oauth", true, false),
        "a successful OAuth connection must release the gate on its own — it is itself \
         proof the account was already allowlisted, so an unchecked checkbox must not \
         re-block Save/Create"
    );

    // Unaffected: BigQuery service_account and enterprise_oauth.
    assert!(
        bq_kyomi_oauth_access_gate_satisfied("bigquery", "service_account", false, false),
        "service_account has no Kyomi Google-account allowlist — the gate must never \
         block it, regardless of connected/confirmed"
    );
    assert!(
        bq_kyomi_oauth_access_gate_satisfied("bigquery", "enterprise_oauth", false, false),
        "enterprise_oauth uses the customer's own OAuth app, not Kyomi's — the gate must \
         never block it"
    );

    // Unaffected: a non-BigQuery provider, even if (hypothetically) its
    // auth mode string were literally "kyomi_oauth".
    assert!(
        bq_kyomi_oauth_access_gate_satisfied("snowflake", "kyomi_oauth", false, false),
        "the gate is BigQuery-specific — ds_type must be checked, not auth_mode alone"
    );
}

// ── KYO-477: Connect/Reconnect gets its OWN predicate ───────────────────
//
// KYO-427 pointed the "Connect BigQuery" button at `bq_kyomi_oauth_access_ok`
// — the Save/Create gate above, which OR's in `oauth_connected`. That
// shipped in v2.6.5 with a green review and a mutation-proved test
// (`bigquery_kyomi_oauth_connect_button_reads_the_shared_access_ok_signal`,
// below) — and did not fix the reported behavior, because `oauth_connected`
// for kyomi_oauth is account-level: true forever, for every datasource,
// once a user has linked Google to Kyomi even once. The prior test proved
// the button *read a signal*; it never proved the signal computed the
// right answer for a returning, previously-linked user trying a *new*
// datasource without having confirmed access for it. That is the gap this
// section closes.

use super::super::bq_kyomi_oauth_connect_allowed;

/// `bq_kyomi_oauth_connect_allowed` — the pure predicate behind the
/// KYO-477 Connect/Reconnect gate — exercised directly. Unlike
/// `bq_kyomi_oauth_access_gate_satisfied` above, this predicate does not
/// even accept an `oauth_connected` argument: Connect must always require
/// its own explicit, in-the-moment confirmation.
#[test]
fn bq_kyomi_oauth_connect_allowed_blocks_unchecked_and_releases_when_checked() {
    // Blocked: BigQuery kyomi_oauth, checkbox unchecked.
    assert!(
        !bq_kyomi_oauth_connect_allowed("bigquery", "kyomi_oauth", false),
        "the Connect gate must block BigQuery kyomi_oauth when access_confirmed is false"
    );

    // Released by the checkbox alone.
    assert!(
        bq_kyomi_oauth_connect_allowed("bigquery", "kyomi_oauth", true),
        "ticking the confirmation checkbox must release the Connect gate"
    );

    // Unaffected: BigQuery service_account and enterprise_oauth.
    assert!(
        bq_kyomi_oauth_connect_allowed("bigquery", "service_account", false),
        "service_account has no Kyomi Google-account allowlist — the Connect gate must \
         never block it"
    );
    assert!(
        bq_kyomi_oauth_connect_allowed("bigquery", "enterprise_oauth", false),
        "enterprise_oauth uses the customer's own OAuth app, not Kyomi's — the Connect \
         gate must never block it"
    );

    // Unaffected: a non-BigQuery provider, even if (hypothetically) its
    // auth mode string were literally "kyomi_oauth".
    assert!(
        bq_kyomi_oauth_connect_allowed("snowflake", "kyomi_oauth", false),
        "the Connect gate is BigQuery-specific — ds_type must be checked, not auth_mode \
         alone"
    );
}

/// Wiring companion to the pure-predicate test above: the
/// `bq_kyomi_oauth_connect_ok` `Signal::derive` definition itself in
/// `DatasourceModal` (not merely its `connect_blocked` call site, covered
/// by `bigquery_kyomi_oauth_connect_button_reads_the_connect_only_signal`
/// below) must never read any spelling of `oauth_connected` — a `||
/// modal_oauth_connected.get()` folded in at the definition site would
/// reintroduce KYO-477 without changing anything the connect_blocked
/// wiring test inspects.
#[test]
fn bq_kyomi_oauth_connect_ok_definition_never_reads_oauth_connected() {
    let definition = extract_between(
        SRC,
        "let bq_kyomi_oauth_connect_ok: Signal<bool> = Signal::derive(move || {",
        "});",
    );
    assert!(
        definition.contains("bq_kyomi_oauth_connect_allowed("),
        "bq_kyomi_oauth_connect_ok must be defined by calling bq_kyomi_oauth_connect_allowed"
    );
    assert!(
        !definition.to_lowercase().contains("oauth_connected"),
        "KYO-477: the bq_kyomi_oauth_connect_ok Signal::derive body must not read \
         oauth_connected (in any spelling — modal_oauth_connected, oauth_connected, \
         etc.) — folding an account-level connected flag in here is the exact defect \
         this ticket fixes, found:\n{definition}"
    );
}

/// The exact regression KYO-477 was filed against, and the acceptance
/// criterion the ticket names explicitly: a Kyomi user who has linked
/// Google to their account before (`oauth_connected == true`,
/// account-level and permanently true from here on) must still be
/// blocked from clicking Connect on a datasource they have not
/// confirmed access for. Save/Create's own gate is contrasted directly
/// alongside it to make the point unmissable: the same
/// `oauth_connected = true, access_confirmed = false` inputs must
/// release ONE gate and hold the OTHER — that is the entire fix.
#[test]
fn connect_gate_stays_blocked_when_oauth_connected_but_not_confirmed() {
    // Save/Create: unaffected by this fix, correctly stays satisfied.
    assert!(
        bq_kyomi_oauth_access_gate_satisfied("bigquery", "kyomi_oauth", true, false),
        "Save/Create must remain satisfied once oauth_connected is true, regardless of \
         access_confirmed — this is existing, correct, unchanged Save/Create behavior"
    );

    // Connect: must NOT reuse that same permissive path. This predicate
    // structurally cannot read `oauth_connected` at all — it only takes
    // `access_confirmed` — so an account-level "linked somewhere, some
    // time" can never silently satisfy it.
    assert!(
        !bq_kyomi_oauth_connect_allowed("bigquery", "kyomi_oauth", false),
        "KYO-477: Connect must stay BLOCKED when access_confirmed is false, even though \
         oauth_connected is (permanently, account-wide) true — this is the exact defect \
         that let a previously-linked user bypass the checkbox and reach a doomed OAuth \
         round-trip on every subsequent datasource"
    );
}

/// The gate must be wired into the edit-mode Save button and the
/// create-mode Catalog-tab Create button, but deliberately NOT into
/// the create-mode Connection-tab Next button — matching the React
/// reference (`DatasourceModal.jsx`), whose `requiresBetaAccess` check
/// lived only on the Create/Save actions. By the time a create-mode
/// user can even reach the Catalog tab for BigQuery kyomi_oauth, a
/// successful OAuth connection has already set `test_result` (KYO-404)
/// — so gating Next too would be redundant, and gating Next instead of
/// Create would let the user pass the notice, uncheck the box, and
/// still complete the create.
#[test]
fn bq_kyomi_oauth_access_gate_wired_into_save_and_create_not_next() {
    // Bounded to just the `disabled=Signal::derive(...)` body — NOT the
    // explanatory comment directly above it, which also names
    // `bq_kyomi_oauth_access_ok` and would otherwise make this
    // assertion pass even if the real read were deleted from the
    // expression itself.
    let save_button = extract_between(
        SRC,
        "disabled=Signal::derive(move || {",
        "on:click=move |_| do_save()",
    );
    assert!(
        save_button.contains("bq_kyomi_oauth_access_ok"),
        "the edit-mode Save button's disabled expression must read \
         bq_kyomi_oauth_access_ok"
    );

    let next_button = extract_between(
        SRC,
        "if is_connection_tab {",
        "\"Next\"",
    );
    assert!(
        !next_button.contains("bq_kyomi_oauth_access_ok"),
        "the create-mode Next button must NOT read bq_kyomi_oauth_access_ok — the KYO-408 \
         gate only ever blocks Save/Create, matching the React reference and avoiding a \
         redundant second gate (BigQuery kyomi_oauth's Next is already gated by \
         connection_step_satisfied's test_result requirement, which cannot be true \
         without a successful — therefore already-allowlisted — OAuth connection)"
    );

    // Bounded to the Create button's own opening-tag attributes: starts
    // at this button's own KYO-408 comment (unique — the Save button's
    // parallel comment reads differently) and ends at the button's own
    // children text, just before `>` closes the opening tag — so a
    // later, unrelated bq_kyomi_oauth_access_ok read elsewhere can't
    // make this pass vacuously.
    let create_button = extract_between(
        SRC,
        "// KYO-408: does not gate \"Next\" above, only the",
        "\"Creating...\"",
    );
    assert!(
        create_button.contains("bq_kyomi_oauth_access_ok"),
        "the create-mode Catalog-tab Create button's disabled expression must read \
         bq_kyomi_oauth_access_ok"
    );
}

// ── KYO-408: Google OAuth denial error translation ──────────────────
//
// `translate_google_oauth_error` itself moved to `utils::oauth_popup`
// (KYO-421), once a third call site (onboarding) needed it — its two
// direct unit tests (`translate_google_oauth_error_rewrites_access_denied`,
// `translate_google_oauth_error_passes_other_errors_through_unchanged`)
// moved with it into that module's own `mod tests`, alongside its other
// wasm32-or-test pure-predicate helpers. The wiring guard below stays
// here: it is a source-text assertion over `datasources.rs`, not a test
// of the function itself.

/// Both OAuth `postMessage` listeners (list-level in `DatasourcesContent`
/// and modal-level in `DatasourceModal`) must translate `GoogleError`
/// specifically, and must NOT apply that translation to the other
/// providers' error arms — `translate_google_oauth_error` assumes an
/// OAuth2 `access_denied` code from Google's shared-app allowlist
/// rejection specifically; applying it to a Snowflake/Databricks/
/// Microsoft/BigQuery-enterprise error would misdescribe those. The
/// sibling guard for the onboarding page's own listener lives in
/// `pages/onboarding/datasource_onboarding.rs`'s own test module.
#[test]
fn google_error_translation_is_not_applied_to_other_providers_error_arms() {
    let list_level = extract_between(
        SRC,
        "OAuthMessage::GoogleError { error } => {",
        "OAuthMessage::SnowflakeError { error }",
    );
    assert!(
        list_level.contains("translate_google_oauth_error(error)"),
        "the list-level listener's GoogleError arm must call \
         translate_google_oauth_error"
    );

    let list_level_others = extract_between(
        SRC,
        "OAuthMessage::SnowflakeError { error }\n                | OAuthMessage::DatabricksError { error }\n                \
         | OAuthMessage::MicrosoftError { error }\n                | OAuthMessage::MicrosoftEnterpriseError { error }\n                \
         | OAuthMessage::BigqueryEnterpriseError { error } => {",
        "toast_error(error);\n                }",
    );
    assert!(
        !list_level_others.contains("translate_google_oauth_error"),
        "the list-level listener's non-Google error arm must pass `error` straight to \
         toast_error, not through translate_google_oauth_error"
    );

    let modal_level = extract_between(
        SRC,
        "OAuthMessage::GoogleError { error } => {\n                    set_modal_oauth_connecting",
        "OAuthMessage::SnowflakeError { error }",
    );
    assert!(
        modal_level.contains("translate_google_oauth_error(error)"),
        "the modal-level listener's GoogleError arm must call translate_google_oauth_error"
    );
}

// ── KYO-427/KYO-477: gate "Connect BigQuery" on its OWN attestation gate ─

/// The KYO-408 checkbox previously gated only Save/Create, which in
/// create mode is unreachable until OAuth has already succeeded — so it
/// gated nothing where it mattered. KYO-427 requires the kyomi_oauth
/// `ModalOAuthStatusPanel`'s "Connect BigQuery" button itself to read a
/// gate driven by the same checkbox — but KYO-477 corrects *which*
/// signal that must be: `bq_kyomi_oauth_connect_ok`, NOT
/// `bq_kyomi_oauth_access_ok` (the Save/Create signal, which OR's in the
/// account-level `oauth_connected` and therefore defeats the checkbox
/// for any previously-linked user — see `bq_kyomi_oauth_connect_allowed`'s
/// doc comment). This test also asserts the negative: the connect_blocked
/// derive must NOT read `bq_kyomi_oauth_access_ok` at all, so a
/// regression back to the KYO-427 shape fails loudly here rather than
/// silently passing a "some gate exists" check.
///
/// Scoped to `kyomi_block`, the `<Show when=bq_auth_mode == "kyomi_oauth">`
/// body used by the sibling KYO-404 tests above — this is the same
/// isolation those tests rely on to keep the textually-similar
/// enterprise_oauth block (which must NOT gain this gate, see the next
/// test) from making either assertion pass vacuously.
#[test]
fn bigquery_kyomi_oauth_connect_button_reads_the_connect_only_signal() {
    let kyomi_block = extract_between(
        SRC,
        "<Show when=move || bq_auth_mode.get() == \"kyomi_oauth\">",
        "<Show when=move || bq_auth_mode.get() == \"enterprise_oauth\">",
    );
    assert!(
        kyomi_block.contains("connect_blocked=Signal::derive(move || {"),
        "the kyomi_oauth ModalOAuthStatusPanel must pass a connect_blocked prop \
         derived fresh from a signal, not a plain bool literal"
    );
    assert!(
        appears_shortly_before(
            kyomi_block,
            "!bq_kyomi_oauth_connect_ok.get()",
            "on_disconnect=on_google_disconnect",
            200,
        ),
        "the kyomi_oauth ModalOAuthStatusPanel's connect_blocked derive must read \
         bq_kyomi_oauth_connect_ok — the Connect-only gate (KYO-477) — not \
         bq_kyomi_oauth_access_ok, the Save/Create gate"
    );
    // Narrowly scoped to the connect_blocked derive itself — NOT the whole
    // kyomi_block, which legitimately still mentions bq_kyomi_oauth_access_ok
    // in an explanatory comment a few lines above (contrasting the two
    // gates). Only the derive's own body must never read it.
    let connect_blocked_derive = extract_between(
        kyomi_block,
        "connect_blocked=Signal::derive(move || {",
        "on_disconnect=on_google_disconnect",
    );
    assert!(
        !connect_blocked_derive.contains("bq_kyomi_oauth_access_ok"),
        "KYO-477: the connect_blocked derive must not read bq_kyomi_oauth_access_ok — \
         that would be the exact defect this ticket fixes"
    );

    // Exact-body check, not just "contains": a `.contains()` check alone
    // would still pass if the derive were diluted to e.g.
    // `!bq_kyomi_oauth_connect_ok.get() && !oauth_connected.get()` — which
    // reintroduces exactly the KYO-477 defect (an account-level
    // `oauth_connected` bypassing the Connect gate) without removing the
    // literal substring the checks above look for. Pin the derive's body
    // to precisely one expression.
    // `extract_between` includes the start marker itself in its result
    // (see its doc comment in `tests/mod.rs`), so strip it back off here
    // to leave just the derive's inner expression.
    let derive_body = extract_between(
        kyomi_block,
        "connect_blocked=Signal::derive(move || {",
        "})",
    )
    .trim_start_matches("connect_blocked=Signal::derive(move || {")
    .trim();
    assert_eq!(
        derive_body, "!bq_kyomi_oauth_connect_ok.get()",
        "KYO-477: the connect_blocked derive's body must be exactly \
         `!bq_kyomi_oauth_connect_ok.get()` — no additional `||`/`&&` clause folding in \
         oauth_connected or any other signal, which would silently dilute the gate back \
         toward the KYO-427 defect"
    );
}

/// Negative-space companion to the test above: none of the other four
/// `ModalOAuthStatusPanel` call sites — BigQuery enterprise_oauth,
/// Snowflake, Databricks, and Microsoft — may gain a `connect_blocked`
/// prop as a side effect of this change. The default
/// (`#[prop(default = false.into())]`) already makes an omitted prop
/// behavior-preserving, but this test would still catch a stray
/// find-and-replace that wired the KYO-408 checkbox into an unrelated
/// provider's Connect button.
#[test]
fn connect_blocked_prop_is_bigquery_kyomi_oauth_only() {
    // KYO-455: this test used to scope to `SRC.split(MOD_TESTS_MARKER).next()`
    // because `SRC` was `include_str!`-ed from this same file, and this
    // test's own marker/assertion-message literals repeat
    // "connect_url=enterprise_oauth_url" and "connect_blocked" verbatim,
    // which would have made a whole-`SRC` scan match this test's own text
    // and either miscount the call sites or pass vacuously regardless of
    // production code. Now that the test module lives in
    // `datasources/tests/` rather than inline in `datasources.rs`, `SRC`
    // contains production code only, so no split/expect workaround is
    // needed — `production_src` is just `SRC` under its old name so the
    // rest of this test's body needs no further changes.
    let production_src = SRC;

    let other_call_sites: &[(&str, &str)] = &[
        ("Snowflake", "connect_url=sf_connect_url"),
        ("Databricks", "connect_url=db_connect_url"),
    ];
    for (name, marker) in other_call_sites {
        let call = extract_between(production_src, marker, "/>");
        assert!(
            !call.contains("connect_blocked"),
            "{name}'s ModalOAuthStatusPanel call must not pass connect_blocked — \
             only BigQuery kyomi_oauth has a KYO-408 attestation gate"
        );
    }

    // "connect_url=enterprise_oauth_url" is shared verbatim by BigQuery
    // enterprise_oauth and Microsoft, so both occurrences must be
    // checked explicitly rather than relying on extract_between's
    // first-match behavior, which would silently skip the second call.
    let enterprise_offsets: Vec<usize> = production_src
        .match_indices("connect_url=enterprise_oauth_url")
        .map(|(i, _)| i)
        .collect();
    assert_eq!(
        enterprise_offsets.len(),
        2,
        "expected exactly two connect_url=enterprise_oauth_url call sites in \
         production code (BigQuery enterprise_oauth and Microsoft) — this test's \
         bounds assume that shape and must be revisited if it changes"
    );
    for offset in enterprise_offsets {
        let tail = &production_src[offset..];
        let end = tail
            .find("/>")
            .unwrap_or_else(|| panic!("no closing /> found after offset {offset}"));
        let call = &tail[..end];
        assert!(
            !call.contains("connect_blocked"),
            "an enterprise_oauth ModalOAuthStatusPanel call site (BigQuery or \
             Microsoft) must not pass connect_blocked — only BigQuery kyomi_oauth \
             has a KYO-408 attestation gate"
        );
    }
}

/// Reachability guard, not a truth-table test: this proves the wiring
/// between the KYO-408 predicate and the Connect/Reconnect buttons
/// cannot be silently deleted from ModalOAuthStatusPanel without a test
/// failing — the acceptance-criteria bar KYO-427 sets, over and above
/// the existing truth-table coverage of
/// `bq_kyomi_oauth_access_gate_satisfied` above.
///
/// Both the "Not connected" (Connect) and "Expired" (Reconnect) branches
/// must each carry `disabled=connect_blocked` — matching how
/// `disconnect_pending` is guarded in the "Connected" branch just above
/// them. `start_connect` (the shared click handler both branches wire
/// via `on:click=start_connect`, KYO-437) must additionally early-return
/// on `connect_blocked.get_untracked()` — a `disabled` attribute alone
/// does not stop a synthetic/programmatic click — and that guard must
/// live in `start_connect` itself, not be duplicated per branch: KYO-437
/// already pins that both buttons share one handler so the popup-monitor
/// arming can't drift between them, and a per-branch copy of the guard
/// would reintroduce exactly that drift risk for this new condition.
/// If a future change removes the guard from either half, the
/// exact-count assertions below fail.
#[test]
fn modal_oauth_status_panel_gates_both_connect_and_reconnect_branches() {
    let panel_fn = extract_between(
        SRC,
        "fn ModalOAuthStatusPanel(",
        "// OAuth Status Re-fetch Hook",
    );

    let disabled_count = panel_fn.matches("disabled=connect_blocked").count();
    assert_eq!(
        disabled_count, 2,
        "expected disabled=connect_blocked on exactly two buttons (Connect and \
         Reconnect) inside ModalOAuthStatusPanel — found {disabled_count}. A count \
         of 1 means one branch lost its gate (the KYO-427 half-propagation defect); \
         a count of 0 means the gate was deleted entirely"
    );

    let click_wiring_count = panel_fn.matches("on:click=start_connect").count();
    assert_eq!(
        click_wiring_count, 2,
        "expected on:click=start_connect on exactly two buttons (Connect and \
         Reconnect) — found {click_wiring_count}. Both branches must keep sharing \
         the one handler (KYO-437) rather than each getting its own inline \
         on:click body, which is exactly the regression this change must avoid"
    );

    let start_connect_body = extract_between(
        panel_fn,
        "let start_connect = move |_: leptos::ev::MouseEvent| {",
        "view! {\n        {move || {",
    );
    let guard_count = start_connect_body
        .matches("connect_blocked.get_untracked()")
        .count();
    assert_eq!(
        guard_count, 1,
        "expected exactly one connect_blocked.get_untracked() early-return guard, \
         inside the shared start_connect closure itself — found {guard_count}. Since \
         both Connect and Reconnect wire the same start_connect (KYO-437), one guard \
         there covers both buttons; 0 means the guard was deleted, and >1 suggests it \
         was duplicated instead of shared"
    );
    assert!(
        appears_shortly_before(
            start_connect_body,
            "connect_blocked.get_untracked()",
            "set_oauth_connecting.set(true);",
            80,
        ),
        "the connect_blocked early-return must gate entry to start_connect — i.e. sit \
         before set_oauth_connecting.set(true) — not merely be present somewhere in the \
         closure"
    );

    // The "Connected" branch's Disconnect button must remain ungated by
    // connect_blocked — this predicate answers "can the user start a
    // NEW connection", which is meaningless once already connected.
    let connected_branch = extract_between(
        panel_fn,
        "// Connected state.",
        "// Expired state.",
    );
    assert!(
        !connected_branch.contains("connect_blocked"),
        "the Disconnect button (Connected branch) must not read connect_blocked — \
         that gate governs starting a new connection, not ending an existing one"
    );
}

// ── KYO-519: oauth_config_missing — shared "OAuth not configured" predicate ──
//
// Ports React's `OAuthConnect.jsx:49-58`: `configFields.every(f =>
// !!connectionConfig[f.name])` gates whether the Connect button (vs. an
// explanatory Alert) renders. Negated — "not configured" — "all fields
// present" becomes "any field missing", i.e. OR. Before this extraction,
// three of the four config-bearing call sites got that wrong in three
// different ways: BigQuery enterprise_oauth and Synapse used `&&` (so a
// half-filled config — exactly what an interrupted admin leaves behind —
// still showed a normal Connect button, because BOTH fields had to be
// empty to trip the warning), and Snowflake was hardcoded
// `Signal::stored(false)` (never warned at all — it had no
// `cfg_oauth_client_id`/`cfg_oauth_client_secret` signals in scope to
// read in the first place, having never received them as props).
// Databricks alone already had the correct `||`. This predicate is now
// the single place the check is spelled out; the guard test below pins
// that a fifth surface can't reintroduce a hand-rolled copy.

use super::super::oauth_config_missing;

#[test]
fn oauth_config_missing_true_when_neither_field_set() {
    assert!(
        oauth_config_missing("", ""),
        "neither client_id nor client_secret set must be reported as missing"
    );
}

#[test]
fn oauth_config_missing_true_when_only_client_id_set() {
    // This is the case the pre-KYO-519 `&&` formula (BigQuery
    // enterprise_oauth, Synapse) got wrong: `"id".is_empty() && "".is_empty()`
    // short-circuits to false on the first operand, so `&&` reported this
    // half-filled config as "configured" even though the secret is blank
    // and any connect attempt would fail server-side.
    assert!(
        oauth_config_missing("client-id-abc", ""),
        "client_id set but client_secret empty must still be reported as missing"
    );
}

#[test]
fn oauth_config_missing_true_when_only_client_secret_set() {
    assert!(
        oauth_config_missing("", "shh-secret"),
        "client_secret set but client_id empty must still be reported as missing"
    );
}

#[test]
fn oauth_config_missing_false_when_both_fields_set() {
    assert!(
        !oauth_config_missing("client-id-abc", "shh-secret"),
        "both fields set means OAuth is fully configured — must not be reported as missing"
    );
}

/// Extracts the four `*AuthModeSection` component bodies, in the same
/// boundaries `auth_mode_sections.rs` already establishes (Synapse's
/// section has no fifth sibling function to anchor on, so its end marker
/// is the next item's `struct` keyword rather than a `fn`).
fn config_bearing_auth_mode_sections(src: &str) -> [(&'static str, &str); 4] {
    [
        (
            "BigQueryAuthModeSection",
            extract_between(src, "fn BigQueryAuthModeSection(", "fn SnowflakeAuthModeSection("),
        ),
        (
            "SnowflakeAuthModeSection",
            extract_between(src, "fn SnowflakeAuthModeSection(", "fn DatabricksAuthModeSection("),
        ),
        (
            "DatabricksAuthModeSection",
            extract_between(src, "fn DatabricksAuthModeSection(", "fn SynapseAuthModeSection("),
        ),
        (
            "SynapseAuthModeSection",
            extract_between(src, "fn SynapseAuthModeSection(", "struct ConnectionFieldsSignals"),
        ),
    ]
}

#[test]
fn all_four_config_bearing_surfaces_route_through_oauth_config_missing() {
    const CALL: &str =
        "oauth_config_missing(&cfg_oauth_client_id.get(), &cfg_oauth_client_secret.get())";

    for (name, section) in config_bearing_auth_mode_sections(SRC) {
        let call_count = section.matches(CALL).count();
        assert_eq!(
            call_count, 1,
            "{name} must derive its cfg_missing signal from exactly one call to \
             oauth_config_missing(...) — found {call_count}. Zero means this surface \
             lost (or never had) the predicate call — the exact KYO-519 Snowflake \
             defect, where the section had no cfg_oauth_client_id/secret signals in \
             scope at all; more than one suggests a second, possibly-drifted copy"
        );

        // The regression this guards against: a call site re-deriving the
        // emptiness check inline instead of calling the shared predicate —
        // whether spelled with `&&` (BigQuery enterprise_oauth, Synapse,
        // pre-KYO-519) or `||` (what Databricks already had, which was
        // correct but still an independent copy).
        assert!(
            !section.contains("cfg_oauth_client_id.get().is_empty()"),
            "{name} must not re-derive the OAuth-config emptiness check inline via \
             a raw `.get().is_empty()` read — that inline re-derivation is exactly \
             the failure mode oauth_config_missing exists to prevent from \
             recurring on a fifth surface (docs/standards/code-organization/\
             propagate-predicate-changes-to-every-copy.md)"
        );
    }
}

#[test]
fn snowflake_cfg_missing_is_derived_not_hardcoded_false() {
    let snowflake = extract_between(
        SRC,
        "fn SnowflakeAuthModeSection(",
        "fn DatabricksAuthModeSection(",
    );
    assert!(
        !snowflake.contains("cfg_missing=Signal::stored(false)"),
        "SnowflakeAuthModeSection's OAuth status panel must not hardcode \
         cfg_missing=Signal::stored(false) — that was the KYO-519 defect: this \
         component never received cfg_oauth_client_id/secret as props, so a member \
         always saw a normal-looking Connect button regardless of whether the admin \
         had configured OAuth, and clicking it started a flow that could not succeed"
    );
    assert!(
        snowflake.contains("cfg_missing=sf_cfg_missing"),
        "SnowflakeAuthModeSection's OAuth status panel must read cfg_missing from \
         the derived sf_cfg_missing signal"
    );
}

#[test]
fn bigquery_kyomi_oauth_is_the_only_surviving_stored_false_exception() {
    // BigQuery's kyomi_oauth mode is the one deliberate exception: it's
    // account-level, globally-hosted Kyomi OAuth with no configFields at
    // all, so there is nothing for oauth_config_missing to check.
    let bigquery = extract_between(SRC, "fn BigQueryAuthModeSection(", "fn SnowflakeAuthModeSection(");
    let stored_false_count = bigquery.matches("cfg_missing=Signal::stored(false)").count();
    assert_eq!(
        stored_false_count, 1,
        "expected exactly one cfg_missing=Signal::stored(false) inside \
         BigQueryAuthModeSection — the documented kyomi_oauth exception. Zero means \
         the exception was removed (and kyomi_oauth is now probably reading the \
         wrong, enterprise_oauth-scoped signals); more than one means \
         enterprise_oauth also regressed to a hardcoded false"
    );

    // None of the other three config-bearing surfaces may hardcode false —
    // each has real configFields to check.
    for (name, section) in [
        (
            "SnowflakeAuthModeSection",
            extract_between(SRC, "fn SnowflakeAuthModeSection(", "fn DatabricksAuthModeSection("),
        ),
        (
            "DatabricksAuthModeSection",
            extract_between(SRC, "fn DatabricksAuthModeSection(", "fn SynapseAuthModeSection("),
        ),
        (
            "SynapseAuthModeSection",
            extract_between(SRC, "fn SynapseAuthModeSection(", "struct ConnectionFieldsSignals"),
        ),
    ] {
        assert!(
            !section.contains("cfg_missing=Signal::stored(false)"),
            "{name} must not hardcode cfg_missing=Signal::stored(false) — only \
             BigQuery's kyomi_oauth mode has no configFields to check"
        );
    }
}
