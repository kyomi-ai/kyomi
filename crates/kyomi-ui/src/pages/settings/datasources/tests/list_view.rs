//! The datasource list view: analytics-access prop threading (KYO-260),
//! the memoized view-state branch on `DatasourcesContent` (KYO-429), and
//! the per-row delete-in-progress state (KYO-467).

use super::super::row_is_deleting;
use super::{extract_between, SRC};

// ── KYO-260: analytics-link permission gating ──────────────────────
//
// The "Analytics Settings" link on an analytics-typed datasource row
// used to be gated only on `ds.is_analytics` (a datasource *type*),
// with no check on whether the viewer could actually use the page it
// routes to. A non-admin member could click through to
// `/settings/analytics` and land on an empty shell, because that
// page's own guard only checked `is_self_hosted`. The fix threads a
// single `analytics_access` Signal — computed once at the list level
// via `use_analytics_access()`, the same predicate the Settings tab
// bar and `analytics.rs` now consume — down into `DatasourceRow`,
// which gates the link with `<Show>`.

#[test]
fn datasource_row_accepts_analytics_access_prop() {
    let f = extract_between(SRC, "fn DatasourceRow(", "fn DatasourceModal(");
    assert!(
        f.contains("analytics_access: Signal<AnalyticsAccess>"),
        "DatasourceRow must accept an analytics_access: Signal<AnalyticsAccess> prop \
         (KYO-260) — without it, the row has no way to gate the Analytics Settings link \
         on anything but the static ds.is_analytics check"
    );
}

#[test]
fn analytics_settings_link_is_gated_by_a_reactive_show() {
    let f = extract_between(SRC, "fn DatasourceRow(", "fn DatasourceModal(");
    let branch = extract_between(f, "if ds.is_analytics {", "} else {");
    assert!(
        branch.contains("<Show") && branch.contains("analytics_access.get()"),
        "the analytics-row \"Analytics Settings\" link must be wrapped in a <Show> \
         gated on the reactive analytics_access signal (KYO-260) — a plain `.then()` \
         would bake in whatever access level was true when the row first rendered, \
         not react to a later-resolving UserContext"
    );
    assert!(
        branch.contains("AnalyticsAccess::Allowed"),
        "the gate must specifically require AnalyticsAccess::Allowed, not just any \
         resolved access value"
    );
}

/// `use_analytics_access` must be called exactly once, at the list
/// level in `DatasourcesContent` — never per row. Calling it inside
/// `DatasourceRow` would create a separate resource subscription per
/// rendered row instead of sharing the one computed by the parent.
#[test]
fn use_analytics_access_is_called_once_at_the_list_level() {
    // KYO-455: this test used to scope to `SRC.split(MOD_TESTS_MARKER).next()`
    // because `SRC` was `include_str!`-ed from this same file, so an
    // unscoped count would also match this test's own source (the
    // "use_analytics_access(" literal above). Now that the test module
    // lives in `datasources/tests/` rather than inline in `datasources.rs`,
    // `SRC` contains production code only, so counting against it directly
    // is correct without the split/expect workaround.
    let count = SRC.matches("use_analytics_access(").count();
    assert_eq!(
        count, 1,
        "use_analytics_access() must be called exactly once (in DatasourcesContent), \
         found {count} call sites in production code — DatasourceRow must receive it as \
         a prop, not call the hook itself"
    );

    let row = extract_between(SRC, "fn DatasourceRow(", "fn DatasourceModal(");
    assert!(
        !row.contains("use_analytics_access("),
        "DatasourceRow must not call use_analytics_access() itself — it must receive \
         the already-computed Signal as the analytics_access prop"
    );
}

// ── KYO-429: memoized view-state branch, not a raw tracked get() ─────

/// `DatasourcesPage`'s view closure must branch on a memoized
/// discriminant, never on a raw tracked `datasources_signal.get()` read.
/// A raw tracked branch re-runs (and rebuilds `DatasourcesContent` — a
/// fresh mount, fresh signals) on every `datasources_signal` write,
/// including the `Some(Ok(_))` -> `Some(Ok(_))` refetch completion that
/// `QueryCache::invalidate` (`query_cache/mod.rs`) produces on every
/// cache invalidation — destroying any open create/edit modal's
/// in-progress, unsaved form state. Extraction is bounded to
/// `DatasourcesPage`'s own body so these assertions can't accidentally
/// match wiring belonging to a different component.
#[test]
fn datasources_page_branches_on_a_memoized_view_state_not_a_raw_tracked_get() {
    let page_fn = extract_between(
        SRC,
        "pub fn DatasourcesPage(",
        "fn DatasourcesLoadingSkeleton(",
    );

    assert!(
        page_fn.contains("Memo::new(move |_| match datasources_signal.get()"),
        "DatasourcesPage must derive its render-branch discriminant via \
         Memo::new reading datasources_signal.get() — a plain closure or \
         Signal::derive re-runs its whole body on every write to \
         datasources_signal, not just when the branch actually changes; \
         a Memo only notifies on a PartialEq-unequal output, which is \
         what collapses a Some(Ok(_)) -> Some(Ok(_)) refetch into a no-op"
    );

    // `page_fn` contains several `view! {` occurrences: the outer one
    // opening the component's returned view, plus one per match arm
    // nested inside it. The LEFTMOST occurrence is exactly the outer
    // one — no arm's `view! {` can appear before `match view_state.get()
    // {`, which itself can only appear after the outer `view! {` opens —
    // so slicing `page_fn` from the first match is the render closure's
    // opening brace, regardless of how many further `view! {}` macros
    // the branches themselves contain.
    let view_start = page_fn
        .find("view! {")
        .expect("DatasourcesPage must still contain its outer view! {} block");
    let view_block = &page_fn[view_start..];

    assert!(
        view_block.contains("match view_state.get()"),
        "the outer view closure must branch on the memoized view_state, \
         not on datasources_signal directly"
    );
    assert!(
        !view_block.contains("datasources_signal.get()"),
        "the outer view closure must not perform its own raw tracked \
         datasources_signal.get() read — that would re-subscribe the \
         closure (and therefore DatasourcesContent's mount) to every \
         refetch completion, including a same-value \
         Some(Ok(_)) -> Some(Ok(_)) transition, reintroducing the \
         KYO-429 remount bug the Memo above exists to prevent"
    );
    assert!(
        view_block.contains("datasources_signal.get_untracked()"),
        "the Ready arm must seed DatasourcesContent's initial_datasources \
         via an untracked read — a tracked .get() there would create a \
         second, redundant subscription to datasources_signal and defeat \
         the point of routing through view_state in the first place"
    );
}

// ── KYO-467: per-row delete-in-progress state ─────────────────────────
//
// Deleting a datasource takes 5-10s server-side; the row previously gave
// no feedback at all for the whole round trip (no spinner, no dimming,
// no toast on failure). `delete_ds_action` is one Action shared by every
// row via `<For>`, so `delete_ds_action.pending()` alone is action-wide —
// gating the visible in-progress state on it directly would spin *every*
// row during any single delete. `row_is_deleting` is the pure boolean
// `DatasourceRow`'s `is_deleting` Signal::derive wraps (also reused by
// `on_toggle`/`on_settings_click`/`on_oauth_click`'s guards), kept as a
// free function specifically so it can be asserted on directly — per
// KYO-477, a test that only proves a signal is *read* by the view
// doesn't prove it *computes the right answer*, and that's exactly what
// shipped green without changing behaviour earlier today.

#[test]
fn matching_id_and_pending_is_deleting() {
    assert!(row_is_deleting(Some("ds-1"), "ds-1", true));
}

#[test]
fn matching_id_but_action_not_pending_is_not_deleting() {
    // A stale `datasource_to_delete` id (left over from a delete that
    // already resolved, or before dispatch) must not keep the row
    // spinning once the action itself isn't pending.
    assert!(!row_is_deleting(Some("ds-1"), "ds-1", false));
}

#[test]
fn pending_action_but_different_row_is_not_deleting() {
    // This is the exact simplification the ticket calls out: dropping the
    // id comparison and gating on `delete_pending` alone would make this
    // row report is_deleting == true while a *different* row's delete is
    // in flight — every row would spin, not just the one being deleted.
    assert!(!row_is_deleting(Some("ds-1"), "ds-2", true));
}

#[test]
fn no_delete_target_is_never_deleting_even_while_pending() {
    // `datasource_to_delete` is `None` before any delete has ever been
    // initiated (or after `on_delete_cancel`) — `delete_pending` being
    // true with no target shouldn't be reachable in practice, but the
    // comparison must still fail safe rather than matching every row.
    assert!(!row_is_deleting(None, "ds-1", true));
}

/// Simulates the actual `<For>` call site: every row in the list computes
/// `row_is_deleting` independently against the same `datasource_to_delete`
/// and `delete_pending` values. Only the one row whose id matches the
/// shared target may come back `true` — this is the "two concurrent
/// deletes can't cross-contaminate" property from the ticket, expressed
/// as a single assertion over all three rows sharing one target/pending
/// pair rather than three isolated calls.
#[test]
fn only_the_targeted_row_reports_deleting_the_rest_do_not() {
    let target = Some("ds-2");
    let pending = true;

    assert!(
        !row_is_deleting(target, "ds-1", pending),
        "ds-1 is not the delete target — must not show in-progress state"
    );
    assert!(
        row_is_deleting(target, "ds-2", pending),
        "ds-2 is the delete target with the action pending — must show in-progress state"
    );
    assert!(
        !row_is_deleting(target, "ds-3", pending),
        "ds-3 is not the delete target — must not show in-progress state"
    );
}

/// `DatasourceRow`'s `is_deleting` Signal::derive must read *both*
/// `datasource_to_delete` and `delete_pending` through `row_is_deleting` —
/// not just the pending flag. Locks in the wiring `row_is_deleting`'s own
/// unit tests above prove the *values* of.
#[test]
fn is_deleting_signal_reads_both_target_id_and_action_pending() {
    let block = extract_between(
        SRC,
        "let is_deleting = Signal::derive(move || {",
        "let ds_for_toggle = ds.clone();",
    );
    assert!(
        block.contains("row_is_deleting("),
        "DatasourceRow's is_deleting signal must be computed via the pure, \
         testable row_is_deleting function, not inlined ad hoc"
    );
    assert!(
        block.contains("datasource_to_delete.get()"),
        "is_deleting must read datasource_to_delete — without it the row \
         can't tell whether it, specifically, is the delete target"
    );
    assert!(
        block.contains("delete_pending.get()"),
        "is_deleting must read delete_pending — without it a stale delete \
         target would keep showing in-progress state after the action resolved"
    );
}

/// A failed delete must surface a visible toast, not just a console log —
/// the confirm dialog has already closed by the time the Err arm runs, so
/// `leptos::logging::error!` alone is invisible to the user (KYO-467).
#[test]
fn failed_delete_surfaces_a_toast_not_just_a_console_log() {
    let effect = extract_between(
        SRC,
        "Effect::new(move |_| {\n        if let Some(result) = delete_ds_action.value().get() {",
        "let on_delete_confirm",
    );
    let err_arm = extract_between(effect, "Err(e) => {", "            }\n        }\n    });");

    assert!(
        err_arm.contains("leptos::logging::error!"),
        "the console log must be kept, not replaced"
    );
    assert!(
        err_arm.contains("toast_error("),
        "a failed delete must also raise a visible toast_error — otherwise \
         it is indistinguishable from a successful delete: the dialog closed, \
         the row didn't move, and nothing else told the user it failed"
    );
}

/// A successful delete must still remove the row from the list — the new
/// in-progress/error handling must not have disturbed the existing
/// optimistic-list-update-on-success path.
#[test]
fn successful_delete_still_removes_the_row_from_the_list() {
    let effect = extract_between(
        SRC,
        "Effect::new(move |_| {\n        if let Some(result) = delete_ds_action.value().get() {",
        "let on_delete_confirm",
    );
    let ok_arm = extract_between(effect, "Ok(()) => {", "Err(e) => {");

    assert!(
        ok_arm.contains("list.retain(|d| d.id != ds.id)"),
        "the Ok arm must still filter the deleted datasource out of the \
         local list by id — this is the actual row-removal behavior, not \
         just a call to some retain() with an unrelated predicate"
    );
}

/// The delete button's own icon must swap to a spinner while `is_deleting`
/// is true — a dimmed row alone (with no per-control feedback) is exactly
/// the ambiguity the ticket says a bare spinner-less fix would still leave.
#[test]
fn delete_button_shows_a_spinner_while_deleting() {
    let row = extract_between(SRC, "fn DatasourceRow(", "fn DatasourceModal(");
    let button = extract_between(
        row,
        "variant=ButtonVariant::GhostDestructive",
        "</Button>",
    );
    assert!(
        button.contains("is_deleting.get()") && button.contains("<Spinner"),
        "the delete button must branch on is_deleting.get() and render a \
         <Spinner> while true, replacing the static trash icon"
    );
    assert!(
        button.contains("disabled=is_deleting"),
        "the delete button must be disabled while its own row is deleting, \
         so a second click can't re-open the confirm dialog mid-delete"
    );
}
