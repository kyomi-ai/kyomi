//! Modal-level OAuth popup lifecycle: `ModalOAuthStatusPanel`'s own
//! `start_connect` handler arming `monitor_oauth_popup` and stashing its
//! cleanup, the on_cleanup teardown actually invoking that cleanup (not
//! merely dropping it), and `build_oauth_recovery_callback`'s recovery
//! path re-checking OAuth status before it can ever report a
//! cancelled/timed-out failure (KYO-436/KYO-437) — including threading
//! `is_create_mode` into the same guard `use_oauth_status_refetch` uses,
//! so the recheck can't 500 against a datasource that doesn't exist yet
//! (KYO-426).
//!
//! The list-level (`DatasourceRow`) counterpart of this same lifecycle
//! lives in the sibling `oauth_popup_recovery_list.rs` — split out
//! separately because the two entry points are owned by different
//! components, and a test's home should be obvious without reading
//! either ticket (KYO-437 vs KYO-440/KYO-524). See
//! `docs/standards/code-organization/one-test-topic-per-file-not-one-big-mod-tests.md`.

use super::{extract_between, SRC};

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
