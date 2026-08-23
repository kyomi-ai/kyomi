//! `test_result` / `discovery_status` / `bq_projects` must reset when
//! credentials or auth mode change out from under them, so a stale
//! "connected" state can't survive a teardown (KYO-413).

use super::super::connection_step_satisfied_from;
use super::{extract_between, SRC};

// ── KYO-413: the "Next" gate must re-close on credential teardown ──
//
// `test_result` gates "Next" (`connection_step_satisfied_from`,
// `google_oauth_success_arm_sets_test_result_and_discovery_status`
// above), but nothing cleared it when the credentials that produced it
// were removed: disconnecting Google OAuth, disconnecting a
// per-datasource OAuth account (BigQuery enterprise_oauth, Snowflake,
// Databricks, Synapse), or removing the BigQuery service-account JSON
// all left a prior `Some(success: true)` in place, so a user could
// advance and save a datasource validated against credentials that no
// longer existed. The fix adds a reset to `test_result`/
// `discovery_status` at every teardown site; the four tests below pin
// each one.

/// `google_disconnect_action` is dispatched only by BigQuery
/// kyomi_oauth's disconnect button
/// (`BigQueryAuthModeSection::on_google_disconnect`). Its success arm
/// already reset the OAuth connected/email/expired signals and the
/// project list before this fix — but left `test_result` alone, so a
/// `test_result` written by the `GoogleSuccess` postMessage arm
/// (`google_oauth_success_arm_sets_test_result_and_discovery_status`)
/// survived a disconnect.
///
/// Bounds: from the Effect's own `if let Some(result) = ...` guard —
/// a single-occurrence string — to the comment introducing the next
/// Action's declaration, also single-occurrence. This captures exactly
/// this Effect's Ok/Err arms and nothing from the near-identically
/// shaped `datasource_disconnect_action` Effect that follows it.
#[test]
fn google_disconnect_success_resets_test_result_and_discovery_status() {
    let arm = extract_between(
        SRC,
        "if let Some(result) = google_disconnect_action.value().get() {",
        "// Input: (provider, datasource_slug).",
    );
    assert!(
        arm.contains("Google account disconnected"),
        "sanity check on the extraction bounds: this must be the \
         google_disconnect_action Effect, not a neighboring one"
    );
    assert!(
        arm.contains("set_test_result.set(None);"),
        "google_disconnect_action's success arm must reset test_result to None — \
         otherwise a prior successful OAuth validation keeps the Next gate open \
         after the account it was validated against is disconnected (KYO-413)"
    );
    assert!(
        arm.contains("set_discovery_status.set(\"idle\".to_string());"),
        "google_disconnect_action's success arm must also reset discovery_status \
         to idle, alongside test_result — the two track together everywhere else \
         in this file"
    );
}

/// `datasource_disconnect_action` is the single shared
/// `Action<(String, String), ...>` instantiated once in `DatasourceModal`
/// and passed as a prop into all four `*AuthModeSection` components —
/// BigQuery's `on_enterprise_disconnect`, Snowflake's
/// `on_sf_disconnect`, Databricks' `on_db_disconnect`, and Synapse's
/// `on_enterprise_disconnect` each dispatch it with a different
/// `provider` string. Fixing its one success-arm Effect therefore
/// closes the gate for all four providers at once — this test pins
/// that the fix is present in that shared Effect.
#[test]
fn datasource_disconnect_success_resets_test_result_and_discovery_status() {
    let arm = extract_between(
        SRC,
        "if let Some(result) = datasource_disconnect_action.value().get() {",
        "// ── SSH tunnel keypair generation",
    );
    assert!(
        arm.contains("Account disconnected"),
        "sanity check on the extraction bounds: this must be the \
         datasource_disconnect_action Effect"
    );
    assert!(
        arm.contains("set_test_result.set(None);"),
        "datasource_disconnect_action's success arm must reset test_result to None \
         for every provider that shares this Action (BigQuery enterprise_oauth, \
         Snowflake, Databricks, Synapse) — otherwise a prior successful validation \
         keeps Next open after the disconnected credentials are gone (KYO-413)"
    );
    assert!(
        arm.contains("set_discovery_status.set(\"idle\".to_string());"),
        "datasource_disconnect_action's success arm must also reset \
         discovery_status to idle, alongside test_result"
    );
}

/// The BigQuery service-account "Remove" chip, the JSON textarea's
/// clear/invalidate paths, and the Authentication Mode selector's
/// `on_change` are the teardown routes in `BigQueryAuthModeSection` that
/// aren't an `Action` — there's no disconnect endpoint to call, so this
/// can't be fixed in a shared Effect like the two tests above. Four call
/// sites clear a stale `test_result`: the "Remove" button's `on:click`,
/// the two branches of `handle_service_account_json` that empty
/// `service_account_email` (valid JSON with no `client_email`, and an
/// emptied textarea), and the Authentication Mode `<Select>`'s
/// `on_change` (switching mode invalidates whatever `test_result` the
/// previous mode's credentials produced). Each one must also reset
/// `test_result`/`discovery_status` via `try_set` — `try_set` because
/// these are plain event handlers writing a signal owned by the parent
/// `DatasourceModal`, crossing the parent/child signal boundary
/// (KYO-408 hit a same-shaped boundary bug that still compiled and
/// silently did nothing).
///
/// Bounds: the whole `BigQueryAuthModeSection` function body, using the
/// `#[component]\nfn ...Section(` prefix on both markers to get
/// single-occurrence strings — the bare function names are also
/// referenced in doc comments elsewhere in this file.
#[test]
fn bigquery_service_account_teardown_resets_test_result_and_discovery_status() {
    let component = extract_between(
        SRC,
        "#[component]\nfn BigQueryAuthModeSection(",
        "#[component]\nfn SnowflakeAuthModeSection(",
    );
    let reset_count = component.matches("set_test_result.try_set(None);").count();
    assert_eq!(
        reset_count, 4,
        "BigQueryAuthModeSection must reset test_result at all four teardown \
         sites (the Remove button, both email-clearing branches of \
         handle_service_account_json, and the Authentication Mode selector's \
         on_change) — found {reset_count}. Without every one of them, some path \
         that invalidates the validated credentials leaves a stale successful \
         test_result behind and Next stays enabled (KYO-413). If this count \
         legitimately changes, update it deliberately — don't let it silently \
         pass or fail for the wrong reason."
    );
    let discovery_reset_count = component
        .matches("set_discovery_status.try_set(\"idle\".to_string());")
        .count();
    assert_eq!(
        discovery_reset_count, 4,
        "discovery_status must be reset alongside test_result at all four \
         teardown sites"
    );
}

/// KYO-413 finding: switching Authentication Mode must re-close the
/// "Next" gate for all four providers, not just the disconnect/remove
/// teardown routes. Repro: BigQuery kyomi_oauth OAuth connect succeeds
/// (`test_result = Some(success: true)`), admin switches Authentication
/// Mode to `service_account` with zero credentials entered —
/// `connection_step_satisfied_from`'s `service_account` arm is
/// `test_succeeded` alone with no mode-scoping, so without this reset
/// "Next" stays enabled for a mode that was never validated. Same shape
/// applies to Snowflake, Databricks, and Synapse: each has more than one
/// non-OAuth auth mode sharing the same unscoped `test_result` gate.
///
/// Bounds each `<Select>`'s block between the "Authentication Mode"
/// label and the description paragraph immediately after `</Select>` —
/// unique within each already-bounded provider section — and confirms
/// the mode setter is actually inside that block before asserting on
/// the reset, so a bad bound fails loudly instead of silently passing.
#[test]
fn auth_mode_selectors_reset_test_result_on_mode_change() {
    let sections: &[(&str, &str, &str, &str)] = &[
        (
            "BigQuery",
            "fn BigQueryAuthModeSection(",
            "fn SnowflakeAuthModeSection(",
            "set_bq_auth_mode.set(val);",
        ),
        (
            "Snowflake",
            "fn SnowflakeAuthModeSection(",
            "fn DatabricksAuthModeSection(",
            "set_sf_auth_mode.set(val);",
        ),
        (
            "Databricks",
            "fn DatabricksAuthModeSection(",
            "fn SynapseAuthModeSection(",
            "set_db_auth_mode.set(val);",
        ),
        (
            "Synapse",
            "fn SynapseAuthModeSection(",
            "struct ConnectionFieldsSignals",
            "set_synapse_auth_mode.set(val);",
        ),
    ];
    for (name, start, end, mode_setter) in sections {
        let f = extract_between(SRC, start, end);
        let select_block = extract_between(
            f,
            "<label class=\"block text-sm font-medium\">\"Authentication Mode\"</label>",
            "<p class=\"text-xs text-muted-foreground\">",
        );
        assert!(
            select_block.contains(*mode_setter),
            "sanity check on the extraction bounds for {name}: expected the \
             Authentication Mode <Select> block to contain the mode setter"
        );
        assert!(
            select_block.contains("set_test_result.try_set(None);"),
            "{name}'s Authentication Mode selector on_change must reset \
             test_result — otherwise a stale success validated against a \
             previous mode's credentials keeps Next enabled after switching \
             to a mode that was never validated (KYO-413)"
        );
        assert!(
            select_block.contains("set_discovery_status.try_set(\"idle\".to_string());"),
            "{name}'s Authentication Mode selector on_change must also reset \
             discovery_status alongside test_result"
        );
    }
}

/// KYO-413 finding: the service-account "Remove" chip must also clear
/// `bq_projects`, mirroring its sibling teardown sites —
/// `google_disconnect_action`'s and `datasource_disconnect_action`'s
/// Effects both call `set_bq_projects.set(vec![])`, and
/// `do_test_and_discover` clears it before every fresh validate.
/// `bq_projects` is populated for `service_account` mode
/// (`r.resources.get("projects")`) and `BqProjectField` renders it as a
/// live `<Select>` whenever non-empty — without this reset, removing the
/// service account leaves the billing/default-project dropdowns still
/// offering the orphaned project list.
#[test]
fn service_account_remove_clears_bq_projects() {
    let component = extract_between(
        SRC,
        "#[component]\nfn BigQueryAuthModeSection(",
        "#[component]\nfn SnowflakeAuthModeSection(",
    );
    assert!(
        component.contains("set_bq_projects: WriteSignal<Vec<(String, String)>>"),
        "BigQueryAuthModeSection must accept a set_bq_projects prop so the \
         Remove chip can clear the discovered project list"
    );
    let remove_chip = extract_between(
        component,
        "set_service_account_email.set(String::new());\n                                            set_cfg_service_account_json.set(String::new());",
        "\"Remove\"",
    );
    assert!(
        remove_chip.contains("set_test_result.try_set(None);"),
        "sanity check on the extraction bounds: expected the Remove chip's \
         on:click block, which must still reset test_result"
    );
    assert!(
        remove_chip.contains("set_bq_projects.try_set(vec![]);"),
        "the service-account Remove chip's on:click must clear bq_projects — \
         otherwise BqProjectField's billing/default-project dropdowns keep \
         offering the removed service account's discovered projects (KYO-413)"
    );
}

/// End-to-end acceptance check for the bug report's own repro:
/// BigQuery with Service Account auth, a successful "Validate &
/// Discover Projects" (Next enabled), then the JSON removed (Next must
/// disable again). service_account mode never sets `modal_oauth_connected`
/// (`connection_step_satisfied_from`'s doc comment — that arm is
/// BigQuery-kyomi_oauth-only), so its Next gate reduces to
/// `test_result.success` alone: this fix's write of `test_result` back
/// to `None` at teardown (pinned by the test above) is exactly the
/// `true` → `false` transition on the last argument here.
#[test]
fn bigquery_service_account_next_disables_after_validate_then_remove() {
    assert!(
        connection_step_satisfied_from("bigquery", "service_account", false, true),
        "a successful Validate & Discover Projects must enable Next"
    );
    assert!(
        !connection_step_satisfied_from("bigquery", "service_account", false, false),
        "removing the service account credentials must re-close Next — a stale \
         test_result from before removal must not keep it open (KYO-413)"
    );
}
