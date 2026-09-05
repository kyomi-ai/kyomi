# A second gate on the same destructive action must reuse the first gate's condition, not a narrower copy of it

Some destructive actions in this codebase — archiving a datasource's cached tables,
garbage-collecting a container that discovery no longer sees — already have a hardened
gate protecting them, built and re-hardened after a real incident. When a second
mechanism is added beside that gate to do a related job (a new best-effort cleanup, a
new side channel that also decides "is it safe to delete this"), the natural thing to
write is a fresh boolean derived from the new mechanism's own local signals: did *my*
loop see any errors? That question is not the question the existing gate answers, and
the two are close enough in shape that the difference is easy to miss in review.

The existing gate earned its extra condition the hard way. `archive_missing_tables`
does not archive on `errors.is_empty()` alone — it also requires `outcome.archive`,
because a run can legitimately produce zero containers and zero errors while still
being untrustworthy evidence for deletion (a permission change, a quota error absorbed
upstream, a genuinely-empty-but-reachable catalog). That second condition is not
incidental; it is the fix for a previously-shipped catastrophic-archive bug. A new
mechanism that recomputes "was this run trustworthy?" from scratch, using only the
subset of signals it happens to have in scope, silently drops whatever the original
fix added — and reintroduces the exact class of bug the original gate exists to
prevent, through a path that never touches the original gate's code at all.

**Rule:** Before writing a new condition that authorizes a destructive action already
guarded elsewhere for the same underlying risk, find the existing gate and read its
*full* condition — not just its name. If your new mechanism's local signals are a
subset of what the existing gate already requires, call the existing computation (or
thread its result through) rather than re-deriving your own boolean from your own
narrower view. If the two mechanisms genuinely need different conditions, say so
explicitly and justify the difference in a comment near both sites — a silent
divergence between two things that sound like they mean the same thing is the defect,
whether or not either one is wrong in isolation.

```rust
// WRONG — reconstructs "was this run trustworthy for GC" from only the
// signals this new function happens to see. Zero containers + zero errors
// passes, even though the same run's own `outcome.archive` is false and
// the existing archive_missing_tables gate explicitly skipped this run.
let run_complete = errors.is_empty();
if run_complete {
    reconcile_container_liveness(&db, &enumerated, ...).await?;
}

// RIGHT — reuses the condition the hardened gate already established for
// "is this run's discovery result trustworthy enough to delete on."
let run_complete = errors.is_empty() && outcome.archive;
if run_complete {
    reconcile_container_liveness(&db, &enumerated, ...).await?;
}
```

Real precedent: **KYO-622** (review log `2026-09-06`, `01:06`, 🔴 Correctness / Asymmetry
Violation). A new container-liveness GC mechanism (`reconcile_container_liveness`) was
added beside the existing, KYO-385/KYO-614-hardened `archive_missing_tables` gate. All
three call sites derived `run_complete = errors.is_empty()` alone. The reviewer proved
with a temporary integration test that a datasource refreshed three times with
`containers: Vec::new(), errors: Vec::new()` — precisely the shape the codebase already
has a name and a test for, `nothing_found_reports_idle_not_failed`, "a genuinely empty
(but reachable) catalog must not be reported as failed" — silently archived every live
row, even though `outcome.archive` was `false` for that exact run and
`archive_missing_tables` was explicitly skipped for it. The fix was
`run_complete = errors.is_empty() && outcome.archive` at all three sites, reusing the
condition `archive_missing_tables` already required. The reviewer's own note names this
as a recurring shape, not a one-off: "a new best-effort side mechanism bolted in next to
an existing, hardened gate needs to inherit that gate's trust boundary explicitly, not
just avoid conflicting with its own local error list." The same subsystem had already
produced a structurally different instance of "a new archive-adjacent mechanism doesn't
fully inherit an existing safety invariant" in **KYO-614** (`2026-09-03`, `10:58`): a
new `ArchiveScope::Containers` gate was keyed by bare `dataset_id`, dropping the
`project_id` component the multi-project BigQuery path actually varies over — a
different mechanical shape (an identity-key collision, not a missing boolean), but the
same category of defect: a new mechanism next to an established one, safe only for as
long as nobody looks at the sibling mechanism's full precondition.

Distinct from
[audit-write-sites-when-tightening-constraint.md](audit-write-sites-when-tightening-constraint.md),
which is about sweeping *existing* call sites when a schema constraint tightens — there
is one canonical condition and every writer must catch up to it. This rule is the
opposite direction: a *new* mechanism is added, and the risk is that it never adopts the
existing condition in the first place. Distinct from
[a-scope-key-must-carry-every-field-the-run-varies-over.md](a-scope-key-must-carry-every-field-the-run-varies-over.md),
which is about a cache/scope key missing a field the run varies over — the KYO-614
half of this rule's evidence is a specific instance of that shape, but this rule is
broader: it also covers a plain boolean condition that was never a "key" at all. See
also
[../error-handling/a-guard-in-one-branch-does-not-cover-the-others.md](../error-handling/a-guard-in-one-branch-does-not-cover-the-others.md),
which is about one dispatch's branches disagreeing with each other; this rule is about
two independent mechanisms, on two independent code paths, that are supposed to agree on
one trust boundary and were never made to.
