//! KYO-468: BigQuery's create-mode Catalog tab renders only the bare
//! manual-entry input, regardless of whether the account's project list
//! was already discovered.
//!
//! The ticket's own premise correction: the Catalog tab *is* reachable in
//! create mode for BigQuery (`connection_step_satisfied_from` — see
//! `create_mode.rs` — already unlocks it for all three auth modes). The
//! actual defect is inside `CreateModeCatalogPicker`: its item list came
//! solely from `available_items`/`catalog_items_for_type`, which reads the
//! three generic discovery buckets (`discovered_databases` /
//! `discovered_schemas` / `discovered_catalogs`) and maps `"bigquery"` onto
//! the `_ => databases` fallthrough arm — a bucket BigQuery never
//! populates. BigQuery's discovery results land in a separate signal,
//! `bq_projects`, that `CreateModeCatalogPicker` never received as a prop
//! at all. So a BigQuery user could connect, the app would successfully
//! fetch and hold their project list in memory, and the Catalog tab would
//! still show nothing but the bare text input.
//!
//! The fix threads `bq_projects` / `bq_projects_loading` / `bq_projects_error`
//! / `bq_projects_attempted` into `CreateModeCatalogPicker`, which routes to
//! the new `BqCreateModeProjectPicker` — sourced from those signals instead
//! of `available_items` — once a listing attempt has actually been made.
//! `bq_projects_attempted` is a new signal (not inferred from
//! `bq_projects.is_empty()`, which is true for "never attempted" too) that
//! distinguishes `enterprise_oauth` (which deliberately never lists
//! projects) from `kyomi_oauth`/`service_account` (which do).
//!
//! Follow-up (code review): the first version of this fix leaked stale
//! state across an auth-mode switch. `bq_projects_attempted` alone can't
//! tell "genuinely attempted under the *current* mode" apart from "still
//! true from a populating mode the user switched away from" — e.g.
//! `service_account` validates (`bq_projects_attempted = true`,
//! `bq_projects` = that service account's real projects), the user then
//! switches Authentication Mode to `enterprise_oauth`, and nothing cleared
//! the three signals, so the Catalog tab kept rendering the previous
//! mode's project checkboxes as if they belonged to the new one. The fix
//! is two parts, both required: (1) `create_mode_catalog_uses_generic_picker`
//! now also takes `bq_auth_mode` and only routes to
//! `BqCreateModeProjectPicker` for the two modes that actually populate
//! `bq_projects` (`kyomi_oauth`/`service_account`), making the leaked state
//! unrenderable regardless of any missed reset site; (2) the auth-mode and
//! `ds_type` `<Select>` `on_change` handlers now reset `bq_projects` /
//! `bq_projects_error` / `bq_projects_attempted`, so the stale state stops
//! existing at all rather than merely being unrenderable today.

use super::super::{
    create_mode_catalog_uses_generic_picker, reset_bq_projects_signals, try_reset_bq_projects_signals,
};
use super::{extract_between, SRC};
use leptos::prelude::{signal, GetUntracked, Owner};

// ── The routing decision itself: a real unit test, not source-text
// inspection ──────────────────────────────────────────────────────────
//
// `create_mode_catalog_uses_generic_picker` is the crux of the fix — the
// boolean that decides whether `CreateModeCatalogPicker` renders through
// the pre-KYO-468 `available_items` branch (which never had BigQuery data)
// or through the new `BqCreateModeProjectPicker` (sourced from
// `bq_projects`). Unlike the rest of this file's Leptos view-tree code,
// this is a plain function extracted specifically so the decision can be
// asserted by value.

/// The regression this ticket exists to fix, stated directly: BigQuery on
/// a populating mode (`kyomi_oauth` or `service_account`), once an attempt
/// has been made, must NOT take the generic `available_items` branch —
/// that is exactly the bug (discovered projects held in `bq_projects`,
/// never rendered).
#[test]
fn bigquery_after_an_attempt_on_a_populating_mode_does_not_use_the_generic_picker() {
    for mode in ["kyomi_oauth", "service_account"] {
        assert!(
            !create_mode_catalog_uses_generic_picker("bigquery", mode, true),
            "once bq_projects_attempted is true under {mode}, BigQuery must route to \
             BqCreateModeProjectPicker (sourced from bq_projects) — routing it \
             through the generic available_items branch is the exact KYO-468 bug, \
             since available_items never carries BigQuery data"
        );
    }
}

/// The regression guard named explicitly by the ticket: `enterprise_oauth`
/// never attempts a project listing (organizational tokens can't list a
/// user's personal GCP projects) and has no discovery button of its own,
/// so `bq_projects_attempted` stays `false` for its entire lifetime in a
/// given modal session under that mode. That case must keep using the
/// generic (pre-KYO-468) branch, unchanged — never the new BigQuery-aware
/// picker, which would otherwise tell a user "no projects found" for a
/// mode that never looked.
#[test]
fn bigquery_before_any_attempt_still_uses_the_generic_picker() {
    for mode in ["kyomi_oauth", "enterprise_oauth", "service_account"] {
        assert!(
            create_mode_catalog_uses_generic_picker("bigquery", mode, false),
            "bigquery under {mode} with bq_projects_attempted == false must still \
             render through the original available_items-driven branch, unchanged \
             from before KYO-468 — it must never be told a listing came back empty \
             when none was ever attempted"
        );
    }
}

/// The KYO-468 leak this fix closes: `enterprise_oauth` must use the
/// generic branch even when `bq_projects_attempted` is stale-`true` from a
/// populating mode the user switched away from earlier in the same modal
/// session (e.g. `service_account` validated, then the user switched
/// Authentication Mode to `enterprise_oauth`). The predicate must check
/// `bq_auth_mode` itself, not just trust `bq_projects_attempted` — the
/// resets on the auth-mode/`ds_type` `on_change` handlers are a second,
/// independent line of defense, not the only one.
#[test]
fn enterprise_oauth_uses_the_generic_picker_even_with_a_stale_attempted_flag() {
    assert!(
        create_mode_catalog_uses_generic_picker("bigquery", "enterprise_oauth", true),
        "enterprise_oauth must route through the generic available_items branch \
         regardless of bq_projects_attempted — its organizational token never lists \
         personal GCP projects, so bq_projects_attempted == true here can only mean \
         a stale value inherited from a different, populating mode. Routing on \
         bq_projects_attempted alone (ignoring bq_auth_mode) is exactly the KYO-468 \
         leak: it would render another mode's stale bq_projects as if they belonged \
         to enterprise_oauth"
    );
}

/// Every non-BigQuery type must render through the generic branch
/// regardless of `bq_auth_mode`/`bq_projects_attempted` — those signals are
/// meaningless for them (only ever written by BigQuery-specific effects).
/// A mutation that dropped the `ds_type != "bigquery"` short-circuit would
/// flip these to `false` and break every other provider's Catalog tab.
#[test]
fn non_bigquery_types_always_use_the_generic_picker_either_way() {
    for ds_type in [
        "postgres", "redshift", "sqlserver", "synapse", "flaredb", "databricks", "clickhouse",
        "mysql", "snowflake",
    ] {
        for mode in ["kyomi_oauth", "enterprise_oauth", "service_account", ""] {
            for attempted in [true, false] {
                assert!(
                    create_mode_catalog_uses_generic_picker(ds_type, mode, attempted),
                    "{ds_type} (bq_auth_mode = {mode:?}, bq_projects_attempted = \
                     {attempted}) must always use the generic available_items branch \
                     — bq_auth_mode/bq_projects_attempted are BigQuery-only state and \
                     must not affect any other provider's Catalog tab"
                );
            }
        }
    }
}

// ── Wiring: the caller must actually pass the four new signals, and the
// view tree must actually branch on the function above ─────────────────

/// `CreateModeCatalogPicker`'s own body must gate its checkbox-picker
/// section on `create_mode_catalog_uses_generic_picker`, not a re-inlined
/// copy of the same boolean expression (which could silently drift from
/// the tested function) and not the pre-KYO-468 unconditional
/// `available_items` branch.
#[test]
fn create_mode_catalog_picker_gates_on_the_shared_routing_function() {
    let f = extract_between(SRC, "fn CreateModeCatalogPicker(", "fn view_service_account_form(");
    assert!(
        f.contains(
            "create_mode_catalog_uses_generic_picker(\n                        &datasource_type.get(),\n                        &bq_auth_mode.get(),\n                        bq_projects_attempted.get(),\n                    )"
        ),
        "CreateModeCatalogPicker must call create_mode_catalog_uses_generic_picker with \
         datasource_type, bq_auth_mode, and bq_projects_attempted as its Show `when=` \
         condition — not a re-inlined boolean expression that could disagree with the \
         tested function, and not the two-argument pre-KYO-468-leak-fix signature that \
         ignores bq_auth_mode entirely: {f}"
    );
    assert!(
        f.contains("<BqCreateModeProjectPicker"),
        "CreateModeCatalogPicker must render BqCreateModeProjectPicker as the fallback of \
         that Show — the branch reached once a BigQuery listing attempt has been made: {f}"
    );
    assert!(
        f.contains("bq_projects=bq_projects")
            && f.contains("bq_projects_loading=bq_projects_loading")
            && f.contains("bq_projects_error=bq_projects_error"),
        "CreateModeCatalogPicker must forward its own bq_projects / bq_projects_loading / \
         bq_projects_error props straight through to BqCreateModeProjectPicker: {f}"
    );
}

/// `CreateModeCatalogPicker` must actually receive the five new signals as
/// props — without this, the caller has nothing to forward and the gate
/// above cannot be wired regardless of what the function does.
#[test]
fn create_mode_catalog_picker_declares_the_five_new_props() {
    let f = extract_between(SRC, "fn CreateModeCatalogPicker(", "fn view_service_account_form(");
    for prop in [
        "bq_projects: ReadSignal<Vec<(String, String)>>",
        "bq_projects_loading: ReadSignal<bool>",
        "bq_projects_error: ReadSignal<Option<String>>",
        "bq_projects_attempted: ReadSignal<bool>",
        "bq_auth_mode: ReadSignal<String>",
    ] {
        assert!(
            f.contains(prop),
            "CreateModeCatalogPicker must declare `{prop}` as a prop — KYO-468 threads all \
             five of the modal's BigQuery project-list/auth-mode signals through explicitly, \
             the same discipline catalog_discovery_denied already established: {f}"
        );
    }
}

/// The modal's call site (the `<Show>` gating the create-mode Catalog tab)
/// must actually pass its own `bq_projects` / `bq_projects_loading` /
/// `bq_projects_error` / `bq_projects_attempted` / `bq_auth_mode` signals
/// into `CreateModeCatalogPicker` — declaring the props (tested above) is
/// not enough if nothing is ever wired to them at the one call site.
#[test]
fn create_mode_catalog_picker_call_site_passes_all_five_signals() {
    let call_site = extract_between(
        SRC,
        "<CreateModeCatalogPicker",
        "// ── CATALOG TAB (edit mode only) ──",
    );
    for wiring in [
        "bq_projects=bq_projects",
        "bq_projects_loading=bq_projects_loading",
        "bq_projects_error=bq_projects_error",
        "bq_projects_attempted=bq_projects_attempted",
        "bq_auth_mode=bq_auth_mode",
    ] {
        assert!(
            call_site.contains(wiring),
            "the create-mode Catalog tab's <CreateModeCatalogPicker> call site must pass \
             `{wiring}` — without it CreateModeCatalogPicker's new props exist but are never \
             fed the modal's actual BigQuery project-list/auth-mode state: {call_site}"
        );
    }
}

// ── bq_projects_attempted: set only where an attempt actually happens,
// never inferred, never set for enterprise_oauth ────────────────────────

/// The kyomi_oauth post-connect fetch (the only Effect that fires for
/// kyomi_oauth specifically — see its own comment) must mark the attempt
/// as made as soon as it starts, not only conditionally on success —
/// otherwise a slow/failed fetch would leave `bq_projects_attempted` false
/// while the Catalog tab is already visible (kyomi_oauth's create-mode
/// gate is `oauth_connected || test_succeeded`, satisfied the instant the
/// popup succeeds, independent of this fetch's own completion).
#[test]
fn kyomi_oauth_projects_fetch_marks_the_attempt_as_made() {
    let f = extract_between(
        SRC,
        "// ── Fetch BigQuery project list when OAuth connects",
        "// ── Discovery section: show post-test fields",
    );
    assert!(
        f.contains("if connected && mode == \"kyomi_oauth\" {"),
        "the project-list fetch must stay gated on mode == \"kyomi_oauth\" specifically — \
         this is the exact gate that keeps enterprise_oauth from ever attempting a listing: \
         {f}"
    );
    assert!(
        appears_shortly_after(
            f,
            "set_bq_projects_attempted.set(true);",
            "if connected && mode == \"kyomi_oauth\" {",
            500,
        ),
        "set_bq_projects_attempted.set(true) must appear inside the \
         `if connected && mode == \"kyomi_oauth\"` block, at (or near) the start of the \
         fetch — not only inside the async success arm, which would leave \
         bq_projects_attempted false for the entire duration of a slow or failing fetch \
         while the Catalog tab may already be visible: {f}"
    );
}

/// The `GoogleSuccess | BigqueryEnterpriseSuccess` popup-message arm sets
/// `modal_oauth_connected = true` for BOTH kyomi_oauth and enterprise_oauth
/// — it is shared between them (see the match arm itself). It must NOT
/// itself set `bq_projects_attempted`; only the mode-gated fetch Effect
/// above may do that. If this arm set it directly, enterprise_oauth would
/// wrongly get `bq_projects_attempted = true` on every successful OAuth
/// popup, and BqCreateModeProjectPicker would render "No projects found"
/// for an account that was never queried — the exact regression the
/// ticket calls out as the required guard.
#[test]
fn the_shared_oauth_success_arm_never_sets_bq_projects_attempted() {
    let arm = extract_between(
        SRC,
        "OAuthMessage::GoogleSuccess { email }\n                | OAuthMessage::BigqueryEnterpriseSuccess { email } => {",
        "OAuthMessage::SnowflakeSuccess { email }",
    );
    assert!(
        !arm.contains("bq_projects_attempted"),
        "the shared GoogleSuccess/BigqueryEnterpriseSuccess arm must never reference \
         bq_projects_attempted — it fires for enterprise_oauth too, which must never be \
         marked as having attempted a project listing: {arm}"
    );
}

/// `test_action`'s Effect (service_account's "Validate & Discover
/// Projects", KYO-405) must mark the attempt as made in BOTH outcomes —
/// a successful listing and a per-key `resource_errors["projects"]`
/// denial — mirroring the discipline `bq_projects_error` already follows
/// in the same two branches (KYO-466 review follow-up, covered in
/// catalog.rs). Missing either would leave a real, completed attempt
/// indistinguishable from "never attempted".
#[test]
fn test_action_effect_marks_the_attempt_as_made_in_both_outcomes() {
    let f = extract_between(
        SRC,
        "Effect::new(move |_| {\n        if let Some(result) = test_action.value().get() {",
        "let do_test_and_discover = move || {",
    );
    assert!(
        f.contains(
            "set_bq_projects.set(opts);\n                            set_bq_projects_error.set(None);\n                            set_bq_projects_attempted.set(true);"
        ),
        "the r.resources.get(\"projects\") success branch must set bq_projects_attempted \
         immediately after populating bq_projects and clearing bq_projects_error: {f}"
    );
    assert!(
        f.contains(
            "set_bq_projects_error.set(Some(format!(\"Couldn't list projects: {reason}\")));\n                            set_bq_projects_attempted.set(true);"
        ),
        "the r.resource_errors.get(\"projects\") denial branch must also set \
         bq_projects_attempted — a denied listing is still a completed attempt, not \
         \"never attempted\": {f}"
    );
}

/// `do_test_and_discover` (dispatched by every provider's Test & Discover
/// button, including service_account's "Validate & Discover Projects")
/// must reset all three `bq_projects*` signals before dispatching a fresh
/// attempt — otherwise stale state from a previous validate would outlive
/// the very dispatch meant to replace it, and a re-validate that hasn't
/// resolved yet would still read as "attempted" (or still show a stale
/// error) while actually back in flight with no fresh result.
///
/// Third-review-cycle finding (KYO-468): this site reset `bq_projects` and
/// `bq_projects_attempted` but never `bq_projects_error`, so a re-validate
/// that starts before a prior failure's error message is cleared kept
/// showing that stale message across the run. The fix routes this site
/// through the shared `reset_bq_projects_signals` helper instead of
/// writing the three signals inline — asserting the single call rather
/// than three separate lines is deliberate: the helper's signature makes
/// it structurally impossible to reset two of the three and forget the
/// third, so there's nothing left for a per-signal assertion to catch that
/// the call-site assertion doesn't already cover.
#[test]
fn do_test_and_discover_resets_all_three_bq_projects_signals_via_the_shared_helper() {
    let f = extract_between(SRC, "let do_test_and_discover = move || {", "test_action.dispatch(");
    assert!(
        f.contains("set_discovered_databases.set(vec![]);"),
        "sanity check on the extraction bounds: expected do_test_and_discover's own reset \
         block: {f}"
    );
    assert!(
        f.contains(
            "reset_bq_projects_signals(set_bq_projects, set_bq_projects_error, set_bq_projects_attempted);"
        ),
        "do_test_and_discover must reset bq_projects/bq_projects_error/bq_projects_attempted \
         together via reset_bq_projects_signals before dispatching a fresh test_action — \
         writing any of the three inline again would reopen the KYO-468 missing-error-reset \
         finding: {f}"
    );
}

// ── BqCreateModeProjectPicker: the four required rendering states ───────

/// The in-flight state takes priority over every other signal — it's the
/// outermost `<Show>`, so a project list arriving stale/empty from a prior
/// attempt can never flash "no projects found" while a fresh fetch is
/// still running.
#[test]
fn bq_picker_shows_a_loading_indicator_while_the_fetch_is_in_flight() {
    let f = extract_between(
        SRC,
        "fn BqCreateModeProjectPicker(",
        "/// Create-mode catalog tab body.",
    );
    assert!(
        f.contains("when=move || bq_projects_loading.get()"),
        "BqCreateModeProjectPicker's outermost Show must gate on bq_projects_loading — the \
         in-flight state must take priority over the discovered/empty/error states nested \
         inside its fallback: {f}"
    );
    assert!(
        f.contains("\"Discovering projects…\""),
        "the loading branch must render an explicit \"Discovering projects…\" indicator: {f}"
    );
}

/// Discovered projects render through the shared `CatalogItemCheckboxList`
/// — the same Select All / Clear affordances the non-BigQuery checkbox
/// list uses — fed directly from `bq_projects`, whose tuples are already
/// `(project_id, label)` (see the kyomi_oauth fetch and the
/// service_account KYO-405 comment: "id and label are the same string").
/// `CatalogItemCheckboxList` writes its `<For>` item's first tuple element
/// — the value, not the label — into `set_selected`, so this wiring is
/// what makes the persisted catalog scope a project id rather than a
/// display name.
#[test]
fn bq_picker_discovered_branch_uses_the_shared_checkbox_list_keyed_by_project_id() {
    let f = extract_between(
        SRC,
        "fn BqCreateModeProjectPicker(",
        "/// Create-mode catalog tab body.",
    );
    assert!(
        f.contains("when=move || !bq_projects.get().is_empty()"),
        "the discovered-projects branch must gate on bq_projects, not available_items: {f}"
    );
    assert!(
        f.contains(
            "<CatalogItemCheckboxList\n                        items=Signal::derive(move || bq_projects.get())\n                        selected=catalog_selected\n                        set_selected=set_catalog_selected\n                    />"
        ),
        "the discovered-projects branch must render CatalogItemCheckboxList fed directly \
         from bq_projects — reusing the shared checkbox-list component (Select All / Clear \
         included) rather than a second, independently maintained copy of that markup: {f}"
    );

    // CatalogItemCheckboxList itself must write the tuple's first element
    // (value = project id), never the second (label), into set_selected —
    // pinned once here rather than per-caller, since every caller (this
    // one included) depends on it.
    let list = extract_between(SRC, "fn CatalogItemCheckboxList(", "/// BigQuery's create-mode catalog-scope picker");
    assert!(
        list.contains("let (value, label) = item;")
            && list.contains("let value_for_change = value.clone();")
            && list.contains("let val = value_for_change.clone();"),
        "CatalogItemCheckboxList must write the (value, _) tuple element — not label — into \
         set_selected via its checkbox on:change handler: {list}"
    );
    assert!(
        !list.contains("let val = label"),
        "CatalogItemCheckboxList must never write `label` into set_selected — for \
         BigQuery, label is the human-readable \"name (project_id)\" string, and catalog \
         scope must persist project ids, not display names: {list}"
    );
}

/// The failed/denied and genuinely-empty states share one `<div>` (manual
/// entry always renders as the fallback either way) but must render
/// visibly different content above the input: a warning `Alert` carrying
/// `bq_projects_error`'s text when set, or an explicit "No projects
/// found." message when the listing succeeded but returned nothing.
#[test]
fn bq_picker_distinguishes_failed_from_genuinely_empty() {
    let f = extract_between(
        SRC,
        "fn BqCreateModeProjectPicker(",
        "/// Create-mode catalog tab body.",
    );
    assert!(
        f.contains("when=move || bq_projects_error.get().is_some()"),
        "the empty-or-failed branch must distinguish an error from a genuine empty result \
         by checking bq_projects_error.get().is_some() — not by re-deriving it from \
         bq_projects.is_empty(), which both states share: {f}"
    );
    assert!(
        f.contains("<Alert variant=AlertVariant::Warning>")
            && f.contains("{move || bq_projects_error.get().unwrap_or_default()}"),
        "a failed/denied listing must render bq_projects_error's actual text inside a \
         warning Alert — not a generic \"couldn't list projects\" string: {f}"
    );
    assert!(
        f.contains("\"No projects found.\""),
        "a genuinely empty (but successful) listing must render an explicit \"No projects \
         found.\" message — distinguishable from the failed-listing Alert above: {f}"
    );
    assert!(
        f.contains(
            "placeholder=\"Enter project IDs, comma-separated\"\n                                prop:value=move || catalog_text.get()"
        ),
        "both the failed and genuinely-empty states must still offer the manual-entry \
         input as a fallback, per the ticket's required rendering states: {f}"
    );
}

/// The regression guard the ticket calls out explicitly: `bq_projects_attempted`
/// must never be read inside `BqCreateModeProjectPicker` itself — the
/// "never attempted" state is guaranteed correct entirely by the caller's
/// routing (tested above), not by any logic in this component. If this
/// component ever gained its own `bq_projects_attempted` check, that would
/// be a second, independent place the "never attempted" fact could be
/// gotten wrong.
#[test]
fn bq_picker_never_reads_bq_projects_attempted_itself() {
    let f = extract_between(
        SRC,
        "fn BqCreateModeProjectPicker(",
        "/// Create-mode catalog tab body.",
    );
    assert!(
        !f.contains("bq_projects_attempted"),
        "BqCreateModeProjectPicker must not reference bq_projects_attempted at all — the \
         caller (CreateModeCatalogPicker) is the only place that decides whether this \
         component renders in the first place; a local reference here would be a second, \
         possibly-disagreeing derivation of the same fact: {f}"
    );
}

/// Belt and braces on the "never attempted" guard: the literal "No
/// projects found." string must appear nowhere in `datasources.rs` other
/// than inside `BqCreateModeProjectPicker` — which the routing tests above
/// already prove is unreachable until `bq_projects_attempted` is true. A
/// second site emitting this copy (e.g. a fallback re-added directly to
/// CreateModeCatalogPicker) could bypass that guard entirely.
#[test]
fn no_projects_found_copy_exists_only_inside_the_bq_picker() {
    let occurrences = SRC.matches("\"No projects found.\"").count();
    assert_eq!(
        occurrences, 1,
        "expected exactly one occurrence of \"No projects found.\" in datasources.rs (inside \
         BqCreateModeProjectPicker) — found {occurrences}. A second site would be reachable \
         outside the bq_projects_attempted-gated branch this ticket requires"
    );
}

// ── The leak fix's second line of defense: the two `on_change` handlers
// that must reset `bq_projects`/`bq_projects_error`/`bq_projects_attempted`
// so the stale state stops existing at all, not merely unrenderable ──────

/// `BigQueryAuthModeSection` must declare setters for `bq_projects_error`
/// and `bq_projects_attempted` — without these two new props, its
/// Authentication Mode `on_change` has no way to reset either signal
/// (`set_bq_projects` already existed for the Remove chip, per KYO-413).
#[test]
fn bigquery_auth_mode_section_declares_the_new_setter_props() {
    let component = extract_between(
        SRC,
        "#[component]\nfn BigQueryAuthModeSection(",
        "#[component]\nfn SnowflakeAuthModeSection(",
    );
    for prop in [
        "set_bq_projects_error: WriteSignal<Option<String>>",
        "set_bq_projects_attempted: WriteSignal<bool>",
    ] {
        assert!(
            component.contains(prop),
            "BigQueryAuthModeSection must declare `{prop}` as a prop so its Authentication \
             Mode selector can reset it on switch: {component}"
        );
    }
}

/// The exact repro from the code review finding: a BigQuery user validates
/// `service_account` credentials (`bq_projects_attempted = true`,
/// `bq_projects` = that service account's real projects), then switches
/// Authentication Mode to `enterprise_oauth`. The Authentication Mode
/// `<Select>`'s `on_change` must reset all three signals — leaving any one
/// of them unreset would let `BqCreateModeProjectPicker` keep rendering
/// (or `create_mode_catalog_uses_generic_picker` keep considering) data
/// that belongs to the service account, not the newly selected
/// `enterprise_oauth` mode.
///
/// Routed through the shared `try_reset_bq_projects_signals` helper
/// (KYO-468 third review cycle) rather than three separate `try_set`
/// calls: asserting the single call is strictly stronger than asserting
/// the three lines individually, since the helper's signature makes it
/// impossible to pass only one or two of the three signals.
///
/// KYO-234 extracted the Authentication Mode `<Select>` itself into the
/// shared `AuthModeSelector` component, used by all four providers — but
/// this reset is BigQuery-specific (the other three providers have no
/// discovered project list to invalidate), so it could not simply move
/// into AuthModeSelector's own on_change alongside the test_result/
/// discovery_status resets that are shared (see
/// `auth_mode_selectors_reset_test_result_on_mode_change` in
/// `credential_state_reset.rs`). Instead BigQueryAuthModeSection builds an
/// `on_bq_auth_mode_changed` callback and passes it to AuthModeSelector's
/// `on_mode_changed` prop, which AuthModeSelector runs after its own two
/// resets. This test checks both halves: the callback's body still calls
/// try_reset_bq_projects_signals, and the call site still wires it in —
/// either one alone silently reintroduces the KYO-468 leak.
#[test]
fn bigquery_auth_mode_switch_resets_bq_projects_state() {
    let component = extract_between(
        SRC,
        "#[component]\nfn BigQueryAuthModeSection(",
        "#[component]\nfn SnowflakeAuthModeSection(",
    );
    let callback_block = extract_between(
        component,
        "let on_bq_auth_mode_changed = Callback::new(move |()| {",
        "});",
    );
    assert!(
        callback_block.contains(
            "try_reset_bq_projects_signals(set_bq_projects, set_bq_projects_error, set_bq_projects_attempted);"
        ),
        "on_bq_auth_mode_changed must reset bq_projects, bq_projects_error, and \
         bq_projects_attempted together via try_reset_bq_projects_signals — otherwise \
         state discovered/failed under a previous mode's credentials survives a switch \
         to a mode that never listed anything, or that never listed at all (the \
         KYO-468 leak): {callback_block}"
    );
    assert!(
        component.contains("on_mode_changed=on_bq_auth_mode_changed"),
        "BigQueryAuthModeSection must wire on_bq_auth_mode_changed into \
         AuthModeSelector's on_mode_changed prop — declaring the callback (checked \
         above) is not enough if AuthModeSelector never runs it: {component}"
    );
}

/// The modal's call site must actually pass `set_bq_projects_error` /
/// `set_bq_projects_attempted` into `BigQueryAuthModeSection` — declaring
/// the props (tested above) is not enough if nothing is wired to them.
#[test]
fn bigquery_auth_mode_section_call_site_passes_the_new_setters() {
    let call_site = extract_between(SRC, "<BigQueryAuthModeSection", "// Snowflake auth mode selector");
    for wiring in ["set_bq_projects_error=set_bq_projects_error", "set_bq_projects_attempted=set_bq_projects_attempted"] {
        assert!(
            call_site.contains(wiring),
            "the <BigQueryAuthModeSection> call site must pass `{wiring}` — without it \
             the component's new setter props exist but are never fed the modal's \
             actual bq_projects_error/bq_projects_attempted signals: {call_site}"
        );
    }
}

/// The other reset path named in the fix: switching the `ds_type` selector
/// away from (or into) `"bigquery"` must also reset `bq_projects` /
/// `bq_projects_error` / `bq_projects_attempted` — a stale BigQuery project
/// list must not survive a round trip through another provider type and
/// reappear when the user selects `"bigquery"` again.
///
/// Routed through `reset_bq_projects_signals` (KYO-468 third review
/// cycle) — see `bigquery_auth_mode_switch_resets_bq_projects_state` for
/// why a single call-site assertion supersedes three per-signal ones.
#[test]
fn ds_type_switch_resets_bq_projects_state() {
    let block = extract_between(
        SRC,
        "on_change=move |val: String| {\n                                                    set_ds_type.set(val);",
        "// Name & Slug (admin fields)",
    );
    assert!(
        block.contains(
            "reset_bq_projects_signals(set_bq_projects, set_bq_projects_error, set_bq_projects_attempted);"
        ),
        "the ds_type <Select>'s on_change must reset bq_projects, bq_projects_error, and \
         bq_projects_attempted together via reset_bq_projects_signals — a project list \
         (or error/attempted state) recorded while bigquery was selected must not survive \
         switching to another type and back (KYO-468 leak): {block}"
    );
}

/// Code review finding on this ticket: the service_account "Remove" chip is
/// a pre-existing reset site (KYO-413) that this ticket's diff never
/// reached — it only reset `bq_projects`, not the two signals that travel
/// with it. Repro: validate service_account credentials, have the listing
/// fail (`bq_projects_attempted = true`, `bq_projects_error = Some(...)`),
/// then click Remove — `bq_projects` clears but the error and attempted
/// flags survive, so the create-mode Catalog tab renders the stale failure
/// message for credentials the user just removed instead of falling back
/// to "not yet validated".
///
/// Routed through `try_reset_bq_projects_signals` (KYO-468 third review
/// cycle) — see `bigquery_auth_mode_switch_resets_bq_projects_state` for
/// why a single call-site assertion supersedes three per-signal ones. The
/// sanity check below still guards the anchor-uniqueness lesson this test
/// itself taught: an earlier version of `set_service_account_email.set(...)`
/// as a start marker was not unique within `BigQueryAuthModeSection`
/// (it also matched an earlier closure), so the extraction silently
/// grabbed the wrong block and this test asserted nothing.
#[test]
fn bigquery_remove_chip_resets_bq_projects_state() {
    let component = extract_between(
        SRC,
        "#[component]\nfn BigQueryAuthModeSection(",
        "#[component]\nfn SnowflakeAuthModeSection(",
    );
    let remove_block = extract_between(
        component,
        "set_service_account_email.set(String::new());\n                                            set_cfg_service_account_json.set(String::new());",
        "\"Remove\"",
    );
    assert!(
        remove_block.contains("set_test_result.try_set(None);"),
        "sanity check on the extraction bounds: expected the Remove chip's on:click \
         block: {remove_block}"
    );
    assert!(
        remove_block.contains(
            "try_reset_bq_projects_signals(set_bq_projects, set_bq_projects_error, set_bq_projects_attempted);"
        ),
        "the Remove chip's on:click must reset bq_projects, bq_projects_error, and \
         bq_projects_attempted together via try_reset_bq_projects_signals — otherwise \
         the create-mode Catalog tab still believes a listing was attempted (or still \
         shows a stale failure message) for credentials that no longer exist: \
         {remove_block}"
    );
}

/// Third review cycle's finding on `google_disconnect_action`'s success
/// Effect (KYO-468): it already reset `bq_projects`/`bq_projects_attempted`
/// (so `BqProjectField`'s dropdowns revert to text inputs after a
/// disconnect) but never `bq_projects_error`, so a listing failure
/// recorded before the disconnect kept showing its stale message
/// afterward. Bounds mirror
/// `google_disconnect_success_resets_test_result_and_discovery_status` in
/// `credential_state_reset.rs`, which pins the neighboring `test_result`/
/// `discovery_status` reset in this same Effect.
#[test]
fn google_disconnect_resets_all_three_bq_projects_signals_via_the_shared_helper() {
    let arm = extract_between(
        SRC,
        "if let Some(result) = google_disconnect_action.value().get() {",
        "// Input: (provider, datasource_slug).",
    );
    assert!(
        arm.contains("Google account disconnected"),
        "sanity check on the extraction bounds: this must be the \
         google_disconnect_action Effect, not a neighboring one: {arm}"
    );
    assert!(
        arm.contains(
            "reset_bq_projects_signals(set_bq_projects, set_bq_projects_error, set_bq_projects_attempted);"
        ),
        "google_disconnect_action's success arm must reset bq_projects, \
         bq_projects_error, and bq_projects_attempted together via \
         reset_bq_projects_signals — leaving bq_projects_error unreset was the \
         KYO-468 third-cycle finding: a listing failure recorded before the \
         disconnect kept showing its stale message afterward: {arm}"
    );
}

/// Same finding, same fix, on the other disconnect Effect
/// (`datasource_disconnect_action` — shared by BigQuery enterprise_oauth,
/// Snowflake, Databricks, and Synapse, though only BigQuery ever populates
/// these three signals). Bounds mirror
/// `datasource_disconnect_success_resets_test_result_and_discovery_status`
/// in `credential_state_reset.rs`.
#[test]
fn datasource_disconnect_resets_all_three_bq_projects_signals_via_the_shared_helper() {
    let arm = extract_between(
        SRC,
        "if let Some(result) = datasource_disconnect_action.value().get() {",
        "// ── SSH tunnel keypair generation",
    );
    assert!(
        arm.contains("Account disconnected"),
        "sanity check on the extraction bounds: this must be the \
         datasource_disconnect_action Effect: {arm}"
    );
    assert!(
        arm.contains(
            "reset_bq_projects_signals(set_bq_projects, set_bq_projects_error, set_bq_projects_attempted);"
        ),
        "datasource_disconnect_action's success arm must reset bq_projects, \
         bq_projects_error, and bq_projects_attempted together via \
         reset_bq_projects_signals, for every provider that shares this Action — \
         leaving bq_projects_error unreset was the KYO-468 third-cycle finding: {arm}"
    );
}

// ── The reset helper itself: a real signal test, not source-text ────────
//
// Unlike the rest of this file's Leptos-view-tree code, `reset_bq_projects_signals`
// and `try_reset_bq_projects_signals` are plain functions taking `WriteSignal`s
// directly (mirrors `create_mode_catalog_uses_generic_picker` above, which is
// tested the same way for the same reason) — so their behavior can be
// asserted by value instead of by inspecting source text.

/// `reset_bq_projects_signals` must actually clear all three signals when
/// called with real, non-empty/non-default state — the source-text tests
/// above only prove call sites *invoke* it, not that the function itself
/// does what its name claims.
#[test]
fn reset_bq_projects_signals_clears_all_three() {
    let owner = Owner::new();
    owner.set();

    let (bq_projects, set_bq_projects) =
        signal::<Vec<(String, String)>>(vec![("p1".to_string(), "Project One".to_string())]);
    let (bq_projects_error, set_bq_projects_error) =
        signal(Some("Couldn't list projects: denied".to_string()));
    let (bq_projects_attempted, set_bq_projects_attempted) = signal(true);

    reset_bq_projects_signals(set_bq_projects, set_bq_projects_error, set_bq_projects_attempted);

    assert_eq!(bq_projects.get_untracked(), Vec::<(String, String)>::new());
    assert_eq!(bq_projects_error.get_untracked(), None);
    assert!(!bq_projects_attempted.get_untracked());
}

/// Same behavior, `try_set` variant — must not silently no-op (e.g. from a
/// signature typo swapping `try_set` for a no-op) when the owning scope is
/// still alive, which is the only case a unit test can exercise; the
/// disposed-scope no-op path is inherent to `try_set` itself, not this
/// helper's own logic.
#[test]
fn try_reset_bq_projects_signals_clears_all_three() {
    let owner = Owner::new();
    owner.set();

    let (bq_projects, set_bq_projects) =
        signal::<Vec<(String, String)>>(vec![("p1".to_string(), "Project One".to_string())]);
    let (bq_projects_error, set_bq_projects_error) =
        signal(Some("Couldn't list projects: denied".to_string()));
    let (bq_projects_attempted, set_bq_projects_attempted) = signal(true);

    try_reset_bq_projects_signals(set_bq_projects, set_bq_projects_error, set_bq_projects_attempted);

    assert_eq!(bq_projects.get_untracked(), Vec::<(String, String)>::new());
    assert_eq!(bq_projects_error.get_untracked(), None);
    assert!(!bq_projects_attempted.get_untracked());
}

/// Small helper mirroring the one in `create_mode.rs` / `catalog.rs`:
/// true when `needle` appears within `window` characters after the first
/// occurrence of `anchor` — the inverse of `super::appears_shortly_before`,
/// needed here because the assertion under test (`set_bq_projects_attempted
/// .set(true)` inside the `if connected && mode == "kyomi_oauth"` block)
/// follows its anchor rather than preceding it.
fn appears_shortly_after(haystack: &str, needle: &str, anchor: &str, window: usize) -> bool {
    let Some(anchor_pos) = haystack.find(anchor) else {
        return false;
    };
    let start = anchor_pos + anchor.len();
    let end = (start + window).min(haystack.len());
    haystack[start..end].contains(needle)
}
