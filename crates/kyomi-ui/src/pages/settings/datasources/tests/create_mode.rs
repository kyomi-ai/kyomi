//! Create-mode "Next" button gating: which (type, mode) pairs may
//! bypass a live connection test, and the registry-driven route
//! matrix that enumerates every pair against its production evidence
//! (KYO-404, KYO-407).

use super::super::connection_step_satisfied_from;
use super::{appears_shortly_before, extract_between, SRC};

/// The kyomi_oauth `ModalOAuthStatusPanel` in `BigQueryAuthModeSection`
/// must not be create-mode gated, and must pass `provider_name="BigQuery"`
/// so its connect button reads "Connect BigQuery" (matching the React
/// original) rather than "Connect Google". Before KYO-404 this panel was
/// hidden entirely in create mode behind
/// `<Show when=move || !is_create_mode.get()>`, with a dead fallback
/// paragraph rendered instead — leaving kyomi_oauth users with no
/// legitimate way to connect (and thus no way to satisfy Test A's
/// `test_result` write) until after save. Unlike the enterprise_oauth
/// panel in the same component, which legitimately keeps its create-mode
/// gate (its `connect_url` is slug-scoped and the slug doesn't exist
/// pre-save), `kyomi_oauth_url` is a fixed, non-slug-scoped endpoint, so
/// there is no reason to hide it before save.
///
/// Scoped to the `kyomi_oauth` `<Show>` block specifically (bounded by
/// the next mode's `<Show>` opening tag), so this cannot pass or fail on
/// account of the enterprise_oauth block's own, textually similar
/// create-mode copy ("After saving, connect your BigQuery account from
/// this settings panel.") — that block is covered by the sibling test
/// below instead.
///
/// The dead-fallback-paragraph check below is scoped to `kyomi_block`
/// rather than whole-file `SRC` for a second, independent reason: this
/// assertion writes out the fallback string literal verbatim, and (since
/// KYO-455 moved the tests out of `datasources.rs`, `SRC`'s
/// `include_str!` no longer pulls in this test module's own source, but
/// a future refactor could still reintroduce a legitimate reference to
/// this same copy elsewhere in production code). Bounding the search to
/// `kyomi_block`, which ends at the next `<Show>` tag, keeps the check
/// scoped to exactly the panel under test regardless.
#[test]
fn bigquery_kyomi_oauth_panel_is_not_create_mode_gated() {
    let kyomi_block = extract_between(
        SRC,
        "<Show when=move || bq_auth_mode.get() == \"kyomi_oauth\">",
        "<Show when=move || bq_auth_mode.get() == \"enterprise_oauth\">",
    );
    assert!(
        appears_shortly_before(
            kyomi_block,
            "provider_name=\"BigQuery\"",
            "connect_url=kyomi_oauth_url",
            100,
        ),
        "the kyomi_oauth ModalOAuthStatusPanel (connect_url=kyomi_oauth_url) must pass \
         provider_name=\"BigQuery\" so the connect button reads \"Connect BigQuery\", \
         matching the React original"
    );
    assert!(
        !kyomi_block.contains("is_create_mode"),
        "the kyomi_oauth ModalOAuthStatusPanel must not be gated on is_create_mode at \
         all, in either direction — KYO-404's bug was hiding it behind \
         `!is_create_mode.get()` with a dead fallback paragraph shown in create mode"
    );
    assert!(
        !kyomi_block.contains("After saving, connect your Google account from this settings panel."),
        "the dead create-mode fallback paragraph for the kyomi_oauth panel must not \
         reappear inside its <Show> block"
    );
}

/// Pins the deliberate asymmetry the sibling test above depends on: the
/// enterprise_oauth `ModalOAuthStatusPanel` (`connect_url=enterprise_oauth_url`)
/// legitimately keeps its `!is_create_mode` gate and its own create-mode
/// fallback copy, because `enterprise_oauth_url` is slug-scoped and the
/// slug doesn't exist before the datasource is saved. Without this test,
/// a future change that correctly removed the kyomi_oauth gate could also
/// remove the enterprise gate — reintroducing a pre-save 404 on the
/// slug-scoped connect endpoint — and nothing here would notice.
#[test]
fn bigquery_enterprise_oauth_panel_keeps_create_mode_gate() {
    let enterprise_block = extract_between(
        SRC,
        "<Show when=move || bq_auth_mode.get() == \"enterprise_oauth\">",
        "<Show when=move || bq_auth_mode.get() == \"service_account\">",
    );
    assert!(
        enterprise_block.contains("Show when=move || !is_create_mode.get()"),
        "the enterprise_oauth ModalOAuthStatusPanel must stay gated on \
         !is_create_mode — its connect_url is slug-scoped and has no slug to \
         target before the datasource is saved"
    );
    assert!(
        enterprise_block
            .contains("After saving, connect your BigQuery account from this settings panel."),
        "the enterprise_oauth create-mode fallback copy must still be present"
    );
}

/// The create-mode footer's `can_next` must keep requiring
/// `!name.get().is_empty()` unconditionally, and the shared
/// `connection_step_satisfied` predicate it reads must relax the
/// `test_result.success` requirement only when BOTH the datasource type
/// is "bigquery" AND the BigQuery auth mode is "enterprise_oauth" — not
/// on datasource type alone. A bigquery-only relaxation would re-enable
/// "Next" for kyomi_oauth with no connection at all, reintroducing a
/// create path that produces a datasource with no verified credentials.
///
/// The exception now lives in `connection_step_satisfied_from`'s own
/// definition rather than inline in the footer (KYO-404 follow-up: the
/// footer, and the Catalog tab pill's class/disabled/on:click closures,
/// all read the same `connection_step_satisfied` signal instead of each
/// carrying their own copy of the predicate). KYO-411 moved the
/// predicate's body out of the `Signal::derive` closure and into a pure,
/// directly-testable function (`connection_step_satisfied_from`, next to
/// `bq_kyomi_oauth_access_gate_satisfied`) so it could add the
/// already-connected-account exception without a fourth inline special
/// case at the `can_next` call site. This test checks three things: the
/// derive delegates to the pure function with the right arguments, the
/// AND'd exception behavior itself (exercised directly, not via string
/// matching), and that the footer still ANDs the name check onto the
/// shared signal rather than folding name emptiness into the shared
/// predicate itself (which would wrongly make the Catalog tab pill
/// require a name too).
#[test]
fn create_mode_can_next_bigquery_exception_requires_enterprise_oauth_mode() {
    let derive_body = extract_between(
        SRC,
        "let connection_step_satisfied: Signal<bool> = Signal::derive(move || {",
        "});",
    );
    assert!(
        derive_body.contains("connection_step_satisfied_from("),
        "connection_step_satisfied must delegate to the pure \
         connection_step_satisfied_from predicate rather than reimplementing the \
         exceptions inline (KYO-411 follow-up to the KYO-404 shared-signal fix)"
    );
    assert!(
        derive_body.contains("modal_oauth_connected.get()"),
        "the derive must pass modal_oauth_connected as the oauth_connected argument \
         (KYO-411) — without it, connection_step_satisfied_from's kyomi_oauth exception \
         can never see a live connection"
    );
    assert!(
        derive_body.contains("test_result.get().map(|r| r.success).unwrap_or(false)"),
        "the derive must still pass test_result's success as the test_succeeded \
         argument — the original requirement for every type/mode other than the two \
         BigQuery exceptions"
    );

    // The exception logic itself, exercised directly: it must require
    // BOTH ds_type == "bigquery" AND bq_auth_mode == "enterprise_oauth"
    // — a bigquery-only relaxation would re-enable Next for kyomi_oauth
    // with zero connection (KYO-404 regression risk).
    assert!(
        connection_step_satisfied_from("bigquery", "enterprise_oauth", false, false),
        "the enterprise_oauth precreate exception must be satisfied on its own"
    );
    assert!(
        !connection_step_satisfied_from("bigquery", "kyomi_oauth", false, false),
        "kyomi_oauth with no connection and no test_result must NOT be satisfied — \
         only enterprise_oauth gets the unconditional precreate exception"
    );
    assert!(
        !connection_step_satisfied_from("snowflake", "enterprise_oauth", false, false),
        "the enterprise_oauth exception is bigquery-specific — ds_type must be \
         checked, not auth_mode alone"
    );

    let footer = extract_between(
        SRC,
        "// Direct create footer (original behavior).",
        "let is_saving = save_action.pending().get();",
    );
    assert!(
        footer.contains("connection_step_satisfied.get() && !name.get().is_empty()"),
        "can_next must still require a non-empty name, ANDed onto the shared \
         connection_step_satisfied signal — this must never become an OR'd-in \
         exception, and the name check must not migrate into the shared predicate \
         itself (the Catalog tab pill must not start requiring a name)"
    );
}

/// The create-mode Catalog tab pill (class/disabled/on:click) and the
/// footer's `can_next` must read the exact same `connection_step_satisfied`
/// signal, not independent copies of `test_result.get().map(|r|
/// r.success).unwrap_or(false)`. Before this fix the tab bar re-checked
/// the raw predicate in all three places, so it never got the BigQuery
/// enterprise_oauth precreate exception applied to `can_next`: "Next"
/// worked, but the Catalog tab rendered permanently `TAB_DISABLED` with a
/// no-op click handler for that one type/mode, since it can never
/// produce a `test_result` before save. A future edit that changes the
/// condition in one of these four sites without updating the others is
/// exactly what this test catches.
///
/// Scoped to the create-mode tab bar's own `<Show>` block (bounded by
/// the comment introducing it and the next section's comment), so it
/// cannot be satisfied by the edit-mode tab bar above it, which has no
/// Catalog-gating logic at all, or by `connection_step_satisfied`'s own
/// definition, which legitimately contains the raw `test_result` read.
#[test]
fn catalog_tab_pill_shares_connection_step_satisfied_with_can_next() {
    let tab_bar = extract_between(
        SRC,
        "// Tab bar — hidden in Connect create-mode (the simplified",
        "// ── Connect create-mode simplified form ──",
    );
    assert!(
        !tab_bar.contains("test_result.get()"),
        "the create-mode Catalog tab pill must read the shared \
         connection_step_satisfied signal, not re-check test_result.get() directly \
         — a raw copy here silently diverges from can_next's bigquery \
         enterprise_oauth exception (KYO-404 follow-up)"
    );
    let shared_reads = tab_bar.matches("connection_step_satisfied").count();
    assert_eq!(
        shared_reads, 3,
        "expected all three Catalog tab pill reads (class, disabled, on:click) to use \
         connection_step_satisfied — found {shared_reads}"
    );

    let footer = extract_between(
        SRC,
        "// Direct create footer (original behavior).",
        "let is_saving = save_action.pending().get();",
    );
    assert!(
        footer.contains("connection_step_satisfied.get()"),
        "can_next must also read the shared connection_step_satisfied signal"
    );
}

// ── KYO-407: registry-driven create-mode Next-button route matrix ──
//
// The create wizard's footer gates "Next" on `connection_step_satisfied`
// (see `create_mode_can_next_bigquery_exception_requires_enterprise_oauth_mode`
// above), which requires `test_result.get().map(|r|
// r.success).unwrap_or(false)` for every (type, auth_mode) pair except
// the one documented BigQuery enterprise_oauth exception. But *how* a
// given pair ever produces that `test_result.success = true` — or
// legitimately bypasses the requirement — is wired independently, once
// per provider, by whoever implemented that provider. BigQuery was
// uncreatable in all three of its auth modes for months (KYO-404/405)
// because nothing enumerated the (type, mode) matrix and checked that
// every cell had a route; every *other* provider's route happened to
// work, so the test suite stayed green the whole time. A paying
// customer found the gap, not CI.
//
// This test derives its pair set from
// `kyomi_core::datasource_registry::all_metadata()` — the same registry
// BigQuery's own auth modes live in — rather than a list hand-typed into
// the test. That is the property that makes it load-bearing: a provider
// or auth mode added to the registry with no wired route fails this
// test immediately, instead of shipping silently the way BigQuery did.
//
// Each named `CreateNextRoute` below is backed by its own assertion
// against `SRC` (`assert_route_has_production_evidence`) — a route that
// is merely declared in `expected_route_for` but whose production
// evidence has been deleted or renamed fails just as loudly as an
// unmapped pair.

/// The independent paths within `DatasourceModal` by which a
/// `(datasource_type, auth_mode)` pair reaches (or legitimately
/// bypasses) `test_result.success` in create mode, unlocking the "Next"
/// button. See the module-level KYO-407 comment above for the failure
/// this enumeration exists to catch.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum CreateNextRoute {
    /// The shared "Test & Discover" button rendered inside
    /// `DatasourceModal`: visible for every type/mode except the ones
    /// the other four routes below cover, and its `on:click` runs
    /// `do_test_and_discover()`, whose `test_action` Effect writes
    /// `test_result.success = true` on a successful probe.
    GenericTestAndDiscover,
    /// The modal-level OAuth listener's `GoogleSuccess |
    /// BigqueryEnterpriseSuccess` arm writes `test_result` directly —
    /// BigQuery kyomi_oauth has no Test & Discover step of its own; the
    /// OAuth handshake itself is the verification (KYO-404).
    OAuthArmWritesTestResult,
    /// An OAuth success arm that calls `do_test_and_discover()` itself
    /// once connected: `SnowflakeSuccess | DatabricksSuccess`, and the
    /// Synapse `MicrosoftEnterpriseSuccess` arm. The generic button
    /// (above) is hidden for these pairs until connected, but the arm
    /// runs discovery the instant the connection succeeds, so the pair
    /// still has a route.
    OAuthArmRunsTestAndDiscover,
    /// BigQuery `service_account`'s own "Validate & Discover Projects"
    /// button, rendered inside `BigQueryAuthModeSection` and wired
    /// through `on_validate` to the same `do_test_and_discover` every
    /// other provider's generic button calls.
    BigQueryServiceAccountValidateButton,
    /// BigQuery `enterprise_oauth` in create mode cannot produce a
    /// `test_result` at all pre-save (its connect endpoint needs a
    /// `datasource_slug` that does not exist yet) — the documented
    /// exception inside `connection_step_satisfied_from` (KYO-411 moved
    /// it out of the `connection_step_satisfied` `Signal::derive`,
    /// which now only delegates to it) bypasses the requirement for
    /// exactly this one pair instead.
    ConnectionStepSatisfiedException,
}

/// The answer key. Every `(type_id, mode_id)` pair the registry is
/// expected to expose today maps to exactly one `CreateNextRoute`
/// above; anything else returns `None`. Deliberately no wildcard arm —
/// see the module comment above for why a catch-all here would defeat
/// the entire point of this test: a genuinely new pair must fall
/// through to `None`, not silently inherit `GenericTestAndDiscover`.
fn expected_route_for(type_id: &str, mode_id: &str) -> Option<CreateNextRoute> {
    use CreateNextRoute::*;
    match (type_id, mode_id) {
        ("bigquery", "kyomi_oauth") => Some(OAuthArmWritesTestResult),
        ("bigquery", "enterprise_oauth") => Some(ConnectionStepSatisfiedException),
        ("bigquery", "service_account") => Some(BigQueryServiceAccountValidateButton),
        ("clickhouse", "password") => Some(GenericTestAndDiscover),
        ("snowflake", "password") => Some(GenericTestAndDiscover),
        ("snowflake", "oauth") => Some(OAuthArmRunsTestAndDiscover),
        ("snowflake", "keypair") => Some(GenericTestAndDiscover),
        ("databricks", "token") => Some(GenericTestAndDiscover),
        ("databricks", "oauth") => Some(OAuthArmRunsTestAndDiscover),
        ("redshift", "password") => Some(GenericTestAndDiscover),
        ("postgres", "password") => Some(GenericTestAndDiscover),
        ("mysql", "password") => Some(GenericTestAndDiscover),
        ("sqlserver", "password") => Some(GenericTestAndDiscover),
        ("synapse", "sql") => Some(GenericTestAndDiscover),
        ("synapse", "service_principal") => Some(GenericTestAndDiscover),
        ("synapse", "enterprise_oauth") => Some(OAuthArmRunsTestAndDiscover),
        ("flaredb", "none") => Some(GenericTestAndDiscover),
        _ => None,
    }
}

/// Verifies the production-source evidence for one `CreateNextRoute`.
/// Called once per route that actually appears among the registry's
/// pairs, from `every_registry_auth_mode_pair_has_a_create_next_route`
/// below — this is the half of the test that catches a route whose
/// backing code has been deleted or renamed out from under a pair that
/// is still mapped to it.
fn assert_route_has_production_evidence(route: CreateNextRoute) {
    match route {
        CreateNextRoute::GenericTestAndDiscover => {
            let block = extract_between(
                SRC,
                "// Test & Discover button — workspace-admin-only (KYO-184): it",
                "{move || if test_action.pending().get() { \"Discovering...\" } else { \"Test & Discover\" }}",
            );
            assert!(
                block.contains("t == \"bigquery\""),
                "the generic Test & Discover button must still exclude BigQuery — \
                 BigQuery has no non-OAuth generic-button route"
            );
            assert!(
                block.contains(
                    "t == \"snowflake\" && sf == \"oauth\" && !modal_oauth_connected.get()"
                ),
                "the generic button must still exclude Snowflake oauth mode while \
                 unconnected — that pair's route is the OAuth success arm instead"
            );
            assert!(
                block.contains(
                    "t == \"databricks\" && db == \"oauth\" && !modal_oauth_connected.get()"
                ),
                "the generic button must still exclude Databricks oauth mode while \
                 unconnected — that pair's route is the OAuth success arm instead"
            );
            assert!(
                block.contains("t == \"synapse\"")
                    && block.contains("syn == \"enterprise_oauth\"")
                    && block.contains("!modal_oauth_connected.get()"),
                "the generic button must still exclude Synapse enterprise_oauth while \
                 unconnected — that pair's route is the OAuth success arm instead"
            );
            assert!(
                block.contains("on:click=move |_| do_test_and_discover()"),
                "the generic button must still dispatch do_test_and_discover(), whose \
                 test_action Effect is what ultimately writes test_result.success"
            );
        }
        CreateNextRoute::OAuthArmWritesTestResult => {
            let arm = extract_between(
                SRC,
                "OAuthMessage::GoogleSuccess { email }",
                "OAuthMessage::SnowflakeSuccess { email }",
            );
            assert!(
                arm.contains("set_test_result.try_set(Some(TestConnectionResult {"),
                "the GoogleSuccess | BigqueryEnterpriseSuccess arm must still write \
                 test_result directly — BigQuery kyomi_oauth has no other route to \
                 test_result.success (KYO-404)"
            );
            assert!(
                arm.contains("success: true,"),
                "the arm must write test_result with success: true on a successful \
                 OAuth handshake"
            );
        }
        CreateNextRoute::OAuthArmRunsTestAndDiscover => {
            let sf_db_arm = extract_between(
                SRC,
                "OAuthMessage::SnowflakeSuccess { email }",
                "OAuthMessage::GoogleError { error } => {",
            );
            assert!(
                sf_db_arm.contains("do_test_and_discover();"),
                "the SnowflakeSuccess | DatabricksSuccess arm must still call \
                 do_test_and_discover() on connect — that is the only route to \
                 test_result.success for Snowflake/Databricks oauth mode"
            );

            let synapse_arm = extract_between(
                SRC,
                "OAuthMessage::MicrosoftEnterpriseSuccess { email } => {",
                "let cleanup_cell =",
            );
            assert!(
                synapse_arm.contains("do_test_and_discover();"),
                "the Synapse MicrosoftEnterpriseSuccess arm must still call \
                 do_test_and_discover() on connect — that is the only route to \
                 test_result.success for Synapse enterprise_oauth"
            );
        }
        CreateNextRoute::BigQueryServiceAccountValidateButton => {
            let button = extract_between(
                SRC,
                "<Show when=move || !service_account_email.get().is_empty()>",
                "\"Validate & Discover Projects\"",
            );
            assert!(
                button.contains("on:click=move |_| on_validate.run(())"),
                "the BigQuery service_account \"Validate & Discover Projects\" button \
                 must still dispatch on_validate — its only route to test_result.success"
            );
            assert!(
                SRC.contains("on_validate: Callback<()>,"),
                "BigQueryAuthModeSection must still accept on_validate as a Callback prop"
            );
            assert!(
                SRC.contains("on_validate=on_bq_validate"),
                "the parent must still wire on_validate=on_bq_validate at the \
                 BigQueryAuthModeSection call site"
            );
            let wiring = extract_between(
                SRC,
                "let on_bq_validate = Callback::new(move |()| {",
                "// ── OAuth disconnect actions",
            );
            assert!(
                wiring.contains("do_test_and_discover();"),
                "on_bq_validate must still call do_test_and_discover() — without this, \
                 the button dispatches nothing and can never produce a test_result"
            );
        }
        CreateNextRoute::ConnectionStepSatisfiedException => {
            // KYO-411 (#370) moved this rule out of the
            // `Signal::derive` closure — which is now only a
            // delegation, `connection_step_satisfied_from(&ds_type.get(),
            // ...)` — and into `connection_step_satisfied_from`'s own
            // body. Retarget the evidence there rather than at the
            // derive: the pure function is what actually encodes the
            // rule and, being a plain function rather than a reactive
            // closure, is far less likely to be reshuffled by an
            // unrelated future refactor.
            //
            // Neither marker below is unique in `SRC` (`datasources.rs`),
            // and no count is quoted here on purpose: since `SRC` is
            // production code, any comment that spelled out a marker
            // count would change the number it was describing as the
            // file evolves. What makes the extraction correct is not
            // uniqueness but position — `extract_between` takes the
            // *leftmost* match of each marker, and both functions are
            // defined near the top of the file, ahead of every other
            // occurrence of either name in production code.
            let predicate = extract_between(
                SRC,
                "fn connection_step_satisfied_from(",
                "fn auth_mode_select_options(",
            );
            assert!(
                predicate.contains(
                    "ds_type == \"bigquery\" && bq_auth_mode == \"enterprise_oauth\""
                ),
                "the pre-save exception must still require BOTH ds_type == \"bigquery\" \
                 AND bq_auth_mode == \"enterprise_oauth\" — this is the ONLY pair with no \
                 other route to satisfying create-mode Next"
            );
            assert!(
                predicate.contains("|| test_succeeded"),
                "the exception must still be OR'd onto the ordinary test_succeeded \
                 requirement, not a replacement of it, for every other pair"
            );
        }
    }
}

/// **The point of KYO-407.** Enumerates every `(type_id, mode_id)` pair
/// straight from `kyomi_core::datasource_registry::all_metadata()` — not
/// from a list hand-typed into this test — and confirms each one maps
/// to a `CreateNextRoute` whose production evidence still exists. A
/// provider or auth mode added to the registry with no wired path to
/// `test_result.success` (or the one documented bypass) fails here
/// immediately, instead of shipping silently the way BigQuery did for
/// months before a paying customer found it (KYO-404/405).
#[test]
fn every_registry_auth_mode_pair_has_a_create_next_route() {
    let pairs: Vec<(String, String)> = kyomi_core::datasource_registry::all_metadata()
        .into_iter()
        .flat_map(|(type_id, meta)| {
            meta.auth_modes
                .iter()
                .map(move |mode| (type_id.to_string(), mode.mode_id.clone()))
        })
        .collect();

    // Guards against a vacuous pass: if all_metadata() ever returned
    // empty (a broken registry build, a bad refactor of this test's own
    // enumeration), the "no unmapped pairs" assertion below would hold
    // trivially over an empty list and this test would report green
    // while checking nothing at all.
    assert!(
        pairs.len() >= 17,
        "sanity check: expected at least the 17 (type, mode) pairs known at the time \
         this test was written, found {} — did all_metadata() change shape, or is the \
         registry failing to enumerate?",
        pairs.len()
    );

    let mut unmapped: Vec<String> = Vec::new();
    let mut routes_used: std::collections::HashSet<CreateNextRoute> =
        std::collections::HashSet::new();
    for (type_id, mode_id) in &pairs {
        match expected_route_for(type_id, mode_id) {
            Some(route) => {
                routes_used.insert(route);
            }
            None => unmapped.push(format!("{type_id}/{mode_id}")),
        }
    }

    assert!(
        unmapped.is_empty(),
        "the following (type_id, mode_id) pairs from \
         kyomi_core::datasource_registry::all_metadata() have no route recorded in \
         expected_route_for: {unmapped:?}. This means a provider or auth mode was added \
         to the registry with no verified path to unlock the create-mode \"Next\" button \
         — see the CreateNextRoute enum and the route list in KYO-407 for the shape a fix \
         needs to take: either wire a new route and record it here, or confirm one of the \
         existing five already covers it and add the match arm."
    );

    for route in routes_used {
        assert_route_has_production_evidence(route);
    }
}
