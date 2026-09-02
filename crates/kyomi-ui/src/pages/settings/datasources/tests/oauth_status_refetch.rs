//! OAuth status re-fetch wiring: the shared `use_oauth_status_refetch`
//! hook and its per-provider source-mapping functions (KYO-197, fixing
//! Synapse's missing re-fetch), the create-mode fetch-guard predicate
//! `oauth_status_source_to_fetch` (KYO-411/KYO-426/KYO-443), and the two
//! places that predicate's effect has to be visible end-to-end: the
//! Connection-step gate accepting an already-established connection
//! (KYO-411) and the BigQuery OAuth success arm writing `test_result` so
//! create mode's Next button can ever enable at all (KYO-404).
//!
//! Split out of `oauth.rs` by KYO-475 — see
//! `docs/standards/code-organization/one-test-topic-per-file-not-one-big-mod-tests.md`.

use super::{extract_between, SRC};

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
