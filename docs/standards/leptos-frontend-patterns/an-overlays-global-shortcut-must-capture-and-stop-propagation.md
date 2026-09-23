# An overlay's global shortcut must intercept in the capture phase

A `document`/`window` `keydown` listener is ambient: nothing about registering it inside a
component scopes it to that component's subtree. So when a modal that owns a chord is
mounted *inside* a page that already owns the same chord, both handlers run for one
keypress. The user sees the modal respond correctly and has no way to see the page quietly
respond too, underneath — the wrong state is only visible after the overlay closes.

Registration order does not save you. Two bubble-phase listeners on the same target both
fire, and z-index, portals and mount order are all irrelevant to the DOM event model. The
overlay is also the component least likely to notice: it is written and reviewed in its own
file, while the colliding listener lives in whatever page happens to mount it — in this
codebase, six files register `keydown` on `document` or `window` (`right_panel.rs`,
`dashboard_editor.rs`, `popover.rs`, `sql_editor/mod.rs`, `sql_editor/code_editor.rs`,
`chart_builder.rs`), and none of them can see the others.

Capture phase is the fix that works by construction rather than by luck: capture-phase
listeners on a target run before bubble-phase listeners on that same target regardless of
registration order, and `stop_propagation()` during capture halts the event before the
target and bubble phases happen at all.

Two traps come with it:

- **`stop_propagation()` must be inside the branch that actually handles the key**, after
  whatever guard decides the overlay is live. Stopping unconditionally makes a
  mounted-but-closed overlay swallow every shortcut on the page behind it.
- **The removal call must pass the same capture flag.** `remove_event_listener_with_callback`
  (bubble) does not remove a listener added with `add_event_listener_with_callback_and_bool(..., true)`.
  There is no compile error and no runtime error — the listener just stays attached, and
  leaks one more copy per open/close cycle.

**Rule:** An overlay that registers a `document`- or `window`-level shortcut registers it in
the **capture** phase (`add_event_listener_with_callback_and_bool(…, true)`), calls
`ev.stop_propagation()` only inside the branch for the keys it actually handles, checks
whether it is open with `get_untracked()` at keydown time rather than reactively at
registration time, and removes with the matching
`remove_event_listener_with_callback_and_bool(…, true)` in `on_cleanup`. Before adding the
listener, grep the page(s) that mount the overlay for the same chord and say in the doc
comment which listener you are intercepting — the capture flag is a bare `true` in an
argument list and reads as arbitrary to the next person who touches it.

```rust
// WRONG — a reconstruction of the overlay's pre-fix registration; the fix
// landed squashed in #514, so no pre-fix revision of `use_catalog_shortcut`
// exists to quote. Bubble phase, no `stop_propagation()`, so the page's own
// listener also runs for the same keypress.
let _ = document.add_event_listener_with_callback(
    "keydown",
    closure.as_ref().unchecked_ref(),
);

// ...which collides with this, quoted from `SqlEditorPage`
// (`crates/kyomi-ui/src/pages/sql_editor/mod.rs`) on `main` @ 6a68ff55,
// de-indented out of its enclosing `#[cfg(target_arch = "wasm32")]` block and
// otherwise verbatim — an unconditional bubble-phase ⌘K listener with no
// guard for an overlay being open. It is not itself the defect and is
// unchanged by the fix; it is the ambient listener the overlay must intercept.
if let Some(doc) = web_sys::window().and_then(|w| w.document()) {
    let _ = doc.add_event_listener_with_callback(
        "keydown",
        keydown_handler.as_ref().unchecked_ref(),
    );
}

// RIGHT — quoted from `use_catalog_shortcut`
// (`crates/kyomi-ui/src/components/dashboard/chart_builder.rs`) on `main` @
// 6a68ff55, de-indented out of its enclosing `Effect::new` body. Elided: the
// inline comments, and the `on_cleanup(move || {` line wrapping the removal
// call (replaced by the comment below it). Every other character is verbatim.
let closure = Closure::wrap(Box::new(move |ev: web_sys::KeyboardEvent| {
    if !open.get_untracked() {
        return;
    }
    let is_meta = ev.meta_key() || ev.ctrl_key();
    if is_meta && ev.key().eq_ignore_ascii_case("k") {
        ev.prevent_default();
        ev.stop_propagation();
        let is_open = catalog_open.try_get_untracked().unwrap_or(false);
        set_catalog_open.try_set(!is_open);
    }
}) as Box<dyn FnMut(web_sys::KeyboardEvent)>);

let _ = document.add_event_listener_with_callback_and_bool(
    "keydown",
    closure.as_ref().unchecked_ref(),
    true,
);
// ...and in on_cleanup, with the same `true`:
let _ = document_clone.remove_event_listener_with_callback_and_bool(
    "keydown",
    &closure_ref,
    true,
);
```

**Enforcement: review-only.** There is no lint for it, and the six-row table in
[enforcement-status.md](enforcement-status.md) predates this rule, so it is not listed
there.

Flagged in **KYO-725** (reopened), `2026-09-11` review log, the *cycle 2* entry under
"chart builder modal remount / typing reversal" — the sole 🟡, and the only finding in that
cycle. `ChartBuilderModal` is mounted as an overlay directly inside `SqlEditorPage`
(`crates/kyomi-ui/src/pages/sql_editor/results_container.rs`), which already installed the
unconditional ⌘K listener quoted above; the modal's newly-added `use_catalog_shortcut`
registered a second one, so opening the modal's catalog also toggled the page's right
sidebar underneath it. Cycle 3 signed the capture-phase fix and specifically checked the
removal flag, noting that *"a mismatched capture flag on removal silently fails to detach
the listener (leak, not a compile/runtime error you'd notice)"*, and grepped every other
`document`-level `keydown` listener in the workspace to confirm none of them keys on
meta+`k` — so the interception cannot silently break a third handler. That grep is part of
the rule, not a one-off: capture-phase interception is only safe once you know who else is
listening for the same chord.

Sibling of [resolve-signal-values-at-click-time.md](resolve-signal-values-at-click-time.md):
that rule is why the handler reads `open`/`catalog_open` with `get_untracked()` at keydown
time. Here the consequence is sharper than staleness — reading `open` reactively at
registration time would make a mounted-but-closed modal steal ⌘K from the page it is
rendered alongside.

Sibling of [self-cancelling-timer-drops-last.md](self-cancelling-timer-drops-last.md): both
are about the teardown half of a raw browser-API resource held across a component's life,
where the failure is silent (a leak, a use-after-free) rather than a compile error, and
both ask for an inline comment because the ordering or the flag looks arbitrary. That one
is about *when* you drop; this one is about the argument you must repeat to drop at all.

Distinct from [no-eager-signal-reads-in-childrenfn.md](no-eager-signal-reads-in-childrenfn.md),
the other rule mined from this component: that one is a reactive-scope defect inside the
Leptos ownership tree. This one is outside it entirely — the DOM does not know the
ownership tree exists, which is exactly why two components can claim one chord without
either file showing a problem.
