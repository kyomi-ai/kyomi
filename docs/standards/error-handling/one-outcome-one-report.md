# One outcome, one report — enumerate the channels that already speak before adding another

A single user-visible outcome — "the connection test failed", "the OAuth attempt ended" —
is usually reachable through more than one piece of UI machinery: a badge, an `Alert`
driven by its own error signal, a toast raised by a `postMessage` listener, a toast raised
by a polling monitor, a message field computed server-side. Each of those was added by a
different ticket, and each one looks correct in isolation. What no single call site shows
you is how many of them fire for the same event.

Both failure directions are real and both are user-visible:

- **Two channels speak.** The user sees the same failure twice, or — worse — sees an
  accurate message from one channel overwritten by a generic or stale one from the other.
- **Neither channel speaks.** Each was written assuming the other owned the case, and the
  outcome the feature exists to surface produces silence.

There is a third variant that is not a bug today and becomes one later: a channel that is
computed, unit-tested, and never read by any consumer. It reads as working coverage while
the UI derives its own text somewhere else entirely.

The tell in a diff is a new `set_*_error`, `toast_*`, or message field added next to an
existing one without a sentence saying what the existing one does on that same path.

**Rule:** Before adding, removing, or re-plumbing anything that reports an outcome to the
user, enumerate every channel already reachable for that outcome and state, in the code or
the PR, which one owns it. Give each outcome exactly one owner. When two producers can
both resolve the same attempt, make the later one check whether the earlier already did
and return without speaking, rather than reporting unconditionally. When you consolidate
onto one channel, delete the other channel's signal and *all* of its sites — a retired
reporter that still has live setters is a second channel waiting to be re-rendered. And if
a message is computed but no consumer reads it, either wire it or say in a doc comment
why it exists, so the next reader doesn't delete it as dead or trust it as displayed.

```rust
// WRONG — the monitor reports every resolved attempt, including ones the
// postMessage listener already reported accurately. The user gets a false
// "connection cancelled." on top of a real success toast.
let on_outcome = move |_outcome| {
    spawn_local(async move { /* recovery fetch */ });
    toast_error("Connection cancelled.");
};

// RIGHT — the listener clears the shared signal when it handles an attempt,
// so the monitor can tell it was already spoken for and stays silent.
let on_outcome = move |_outcome| {
    if oauth_connecting.try_get_untracked().flatten().is_none() {
        return; // a postMessage already resolved and reported this attempt
    }
    spawn_local(async move { /* recovery fetch */ });
    toast_error("Connection cancelled.");
};
```

Three tickets, one shape:

- **KYO-469** (`13:05`, 2026-08-23) — the double-speak case. The generic Test & Discover
  site rendered the same failure twice: once as the connection-badge heading and once
  through a separate `discovery_error`-driven `Alert`. The fix routed both sites through
  one shared `ConnectionTestResultBadge` and removed `discovery_error` entirely — the
  review counted the removal as *five* sites for one signal (two setters, two resets, one
  reader), which is the real size of retiring a reporting channel.
- **KYO-440 cycle 4 / KYO-524** (`06:15`, 2026-08-24) — the same-event-two-producers case,
  introduced by an otherwise-correct fix. Making the popup monitor's liveness gate
  row-private (see
  [split-a-value-that-answers-two-questions.md](../data-state-management/split-a-value-that-answers-two-questions.md))
  meant a private token could no longer observe a `postMessage`, so `on_outcome` began
  firing on attempts `install_oauth_listener` had already reported — a duplicate, and
  factually wrong, "connection cancelled." toast over a real one. Fixed in the same diff
  with the early return shown above, plus two tests pinning that the check lives in
  `on_outcome` and *not* in `still_connecting`, where it would resurrect the shared-signal
  bug the cycle had just closed.
- **KYO-466** (`17:22`, 2026-08-23) — both remaining variants in one review. 🟡: the
  `test_action` Effect read `r.resources` but never `r.resource_errors`, so a
  `list_projects()` denial rendered an empty, unexplained dropdown — nobody spoke, on the
  exact code path the ticket existed to fix. 🟢: `discovery_outcome_message` computed three
  conditional success messages, well unit-tested server-side, that neither UI consumer ever
  read — both derived their own text. Resolved by wiring the silent path into the
  already-rendered `bq_projects_error` signal, and by doc-commenting the unread message
  field with why it stays.

Distinct from
[no-user-facing-claim-the-branch-doesnt-establish.md](no-user-facing-claim-the-branch-doesnt-establish.md):
that rule is about a single message asserting something the branch never established. This
one is about *how many* messages fire, and which of them the user is entitled to believe
when they disagree.
