# A pattern copied from another call site must bring its precondition, not just its shape

"Same pattern as `X`" is the most common justification an implementer writes for a design
decision in this codebase, and it is usually right. It is worth writing down *why* it is
sometimes catastrophically wrong.

A pattern in working code is a shape plus an unwritten precondition that makes the shape
correct — a page that has no error variant, a flag that is private to one component, a signal
whose only writer is the closure reading it. The shape is what you can see in the source; the
precondition is not in the source at all. Copying reproduces the shape perfectly and drops the
precondition silently, and because the shape came from code that demonstrably works, the copy
inherits its credibility. The reviewer's attention lands on the diff, which looks like an
existing convention being followed.

The two ways this shows up are both nasty:

- **The precondition is false at the new site.** The copy is a live defect from the first
  commit; the justification comment attached to it is a false statement, so a reader who checks
  the reasoning is actively misled rather than merely uninformed.
- **The precondition was never articulated anywhere,** so the fix is not "copy something else"
  — it is to *establish* the property at the new site. Both incidents below were ultimately
  resolved that way, and one of them took four review cycles to get there because each
  intermediate fix patched the manifestation the last cycle had named rather than the missing
  property.

**Rule:** When you justify code by pointing at another call site, name the *property* of that
site that makes the shape correct, and show that the property holds where you are putting it —
in the code comment, not just in your head. If the property does not hold, do not adapt the
shape around it: either establish the property here (make the flag private, give the value its
own scope) or pick a different construction. A justification of the form "same as `X`" with no
named property is not a justification; it is a hope that `X`'s reviewer thought about your case.

```rust
// WRONG — the shape transfers, the precondition doesn't. `oauth_complete.rs` can render
// unconditionally positive copy because it is a success-only terminal page with no error
// variant. This page has both outcomes, so the same shape tells a user whose OAuth just
// failed that their account is linked.
// "Same pattern as OAuthCompletePage."
// (illustrative shape, not verbatim source)
let title = if popup_close_blocked.get() { "You're All Set" } else { title_for(status.get()) };

// RIGHT — title and icon stay driven by the thing that knows the outcome, and only an
// outcome-neutral sentence is appended when the close was refused.
let title = title_for(status.get());
let subtitle = {
    let base = subtitle_for(status.get());
    if popup_close_blocked.get() { format!("{base} You can close this window.") } else { base }
};
```

```rust
// WRONG — `ModalOAuthStatusPanel`'s guard really is airtight, because its `oauth_connecting`
// is a bool private to one component instance. Reusing the guard's *reasoning* against a
// signal shared by every row in the list makes the comment false and leaks a live timer.
// "The click guard refuses to start a second connect while the first is still in flight,
//  so the slot can only ever hold a resolved monitor's now-inert cleanup."
if is_connecting.get_untracked() { return; }   // reads the list-wide `oauth_connecting`
*popup_monitor.write_value() = Some(monitor_oauth_popup(/* ... */));

// RIGHT — establish the property the borrowed reasoning depended on: give this monitor its
// own per-attempt state that no other row, and no other flow on the page, can write.
let (connect_attempt_live, set_connect_attempt_live) = signal(false);
let still_connecting = move || connect_attempt_live.get_untracked();
```

Real precedent — two 🟡, two tickets, both traced to a borrowed precondition:

- **KYO-436 (`15:05` cycle 2, 2026-08-22)** — 🟡, *incorrect user-facing message on error path*.
  The new `popup_close_blocked` branch rendered "You're All Set" / "Your account is linked"
  unconditionally, including after `send_oauth_error_to_opener`. The review names the borrowing
  explicitly: *"The implementer's own justification ('same pattern as OAuthCompletePage') does
  not hold: `oauth_complete.rs` is a success-only terminal page with no error variant, so its
  unconditionally-positive copy doesn't transfer to a page that can represent either outcome."*
  Cycle 3 (`15:45`) resolved it by making title and icon purely `status`-driven and appending
  only an outcome-neutral sentence.
- **KYO-440 (`13:19` cycle 1, 2026-08-24)** — 🟡, *State Management*, at
  `crates/kyomi-ui/src/pages/settings/datasources.rs`. A comment justified overwriting the
  per-row `popup_monitor` slot on every click. *"That assumption is copied from
  `ModalOAuthStatusPanel`, where `oauth_connecting` is a private `bool` and the guard is
  genuinely airtight. It does not hold here: `oauth_connecting` is a single
  `ReadSignal<Option<String>>` shared across every row."* The consequence was a real leaked
  `Interval`/`Timeout` that could fire a false "cancelled" over a live attempt. It then took
  cycles 2 (`13:49`), 3 (`14:22`) and 4 (`06:15`) to close, because cycles 2 and 3 kept guarding
  the manifestation the previous cycle had traced — first another row's click, then the shared
  signal's second writer — while the modal's own independent connect flow remained a third,
  unenumerated writer. Cycle 4 shipped the property instead of another guard:
  `connect_attempt_live`, a row-private signal that is the sole input to `still_connecting`.

The same window shows what the discipline looks like when it is done right, and in every case
it is one grep plus one named property:

- **KYO-467 (`19:15`, 2026-08-23)** — the new `toast_success` call is `#[cfg]`-gated while the
  neighbouring `toast_error` is not. Justified by grepping every `toast_success`/`toast_info`
  call inside an `Effect::new` in the same file and confirming all five are gated identically,
  *and* by reading `toast.rs` to confirm the functions are unconditionally defined and no-op
  safely — the property, not just the tally.
- **KYO-434 (`12:45`, 2026-08-22)** — `overlay_class.clone()` justified by the property rather
  than the neighbour: the `String` is captured by a `<Show>` children closure, which Leptos
  requires to be `Fn` and re-invokes on every retoggle, so a moved-in `String` would make it
  `FnOnce`. `content_class` a few lines above does the same thing for the same reason.
- **KYO-406 (`15:40`, 2026-08-24)** — a raw `<button>` styled as an inline text link, justified
  by six pre-existing occurrences of the identical construction in the same file, which makes it
  established local precedent rather than a new design-system deviation.

Related to
[no-guarantee-stronger-than-code-enforces.md](../comments-documentation/no-guarantee-stronger-than-code-enforces.md):
that rule is about a comment claiming more than the code below it does, which is what a borrowed
precondition *becomes* once written down. This rule is about the step before — the decision that
produced the claim. Distinct from
[propagate-predicate-changes-to-every-copy.md](propagate-predicate-changes-to-every-copy.md),
which is about the same expression duplicated so that both copies must move together; here
there is only one copy, and its problem is the context it landed in.
