//! The datasource list view: analytics-access prop threading (KYO-260)
//! and the memoized view-state branch on `DatasourcesContent` (KYO-429).

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
