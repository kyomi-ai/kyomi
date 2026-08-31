# Prove a behaviour-preserving refactor by diffing the bodies — and name every difference it unifies

"Pure refactor, no behaviour change" is a claim, and it is one of the few claims in a diff
that nothing in the toolchain checks. `cargo check` proves the new shape type-checks.
`cargo test` proves the surviving tests still pass — but those tests were written against the
*old* shape, so they only cover the behaviour someone already thought to pin, and a green
suite is exactly what you get whether the refactor preserved everything or quietly unified two
things that differed. The diff itself is the worst possible evidence: a consolidation moves
code, so every line shows as `-` on one side and `+` on another, and reading the two halves
side by side is precisely the comparison a human is bad at.

The dangerous shape is not the extraction that changes something loudly. It is the
consolidation of N near-duplicates into one shared implementation, because near-duplicates
differ — that is why there were N of them — and each real difference has to be either
preserved in the shared path or dropped on purpose. A difference nobody enumerated gets
resolved by whichever copy the author happened to type out, and it is then invisible forever:
there is no second copy left to compare against.

Both halves of the failure are cheap to close, and both are mechanical rather than a matter of
care. Extract each pre-refactor body from `origin/main` and each post-refactor body from the
working tree, and `diff` them — do not read them. And before that, enumerate the differences
the merge is going to unify, list them in the PR body, and give each one a decision.

**Rule:** When a change claims to preserve behaviour — an extraction, a delegation, a
consolidation of duplicated implementations, a rebase or conflict rework replaying an
already-approved fix — prove it by diffing the bodies mechanically: pull each original out of
`git show origin/main:<path>` and each replacement out of the tree, and `diff` them
identifier-for-identifier, including the ordering of side effects, the response/JSON shapes,
and the error-message strings. Where the sources being merged genuinely differ, enumerate
those differences *before* writing the shared implementation, state each one in the PR body
with its resolution, and leave a `NOTE:` in code naming the ticket for any difference you
deliberately dropped. If your only evidence is "I read both and they look the same," you have
not checked the thing that actually goes wrong.

```bash
# WRONG — the refactor is asserted, and the suite is green because the suite
# only ever covered one of the two behaviours being merged.
$ cargo test -p kyomi-agent --lib
test result: ok. 1053 passed; 0 failed
#   "delegation is behaviour-preserving" — the tests inherited from the old
#   shape cannot distinguish "identical" from "silently unified".

# RIGHT — extract both sides and let diff answer it.
$ git show origin/main:crates/kyomi-agent/src/tools/knowledge.rs \
    | sed -n '/impl AgentTool for EditDocumentTool/,/^}/p' > /tmp/before.rs
$ sed -n '/pub(crate) async fn execute_for/,/^}/p' \
    crates/kyomi-agent/src/tools/document/edit.rs > /tmp/after.rs
$ diff /tmp/before.rs /tmp/after.rs   # logic, strings, and call ordering, not vibes

# ...and the enumeration that diff cannot do for you, stated in the PR body:
#   Differences unified by document::edit:
#     1. CAS hash source            -> kept knowledge.rs's internal re-read
#     2. embedding-refresh timing   -> kept embed-then-update-then-rechunk order
#     3. read locator vs view track -> record_view stays Dashboard-only
#     4. cross-doc-type reach       -> doc_type_filter: None preserved, KYO-NNN
```

Real precedent — four reviews in four days where the mechanical diff *was* the evidence, and
two where the thing it would have caught was found only because someone went looking:

- **KYO-537** (review log `2026-08-29`, `13:28`) — a tests-only stage whose central constraint
  was that production code not change at all. The reviewer did not read the diff for that:
  > Extracted everything above the first `#[cfg(test)]` from `origin/main` and `HEAD` for all
  > five touched production files and diffed — byte-identical in all five.

  The same review's Notes are the counter-half, and they are about a *fixture* consolidation,
  not production code: the shared `test_support::test_pool()` enables `PRAGMA
  foreign_keys=ON`, which `catalog.rs`'s pre-existing local `test_pool` already did and
  `knowledge.rs`'s did not, so the migration
  > silently tightens `knowledge.rs`'s test harness to match `catalog.rs`'s.

  Harmless here — the review records "all 1041 pass" — and correctly in the spirit of the
  ticket, but it is
  a difference between two merged copies that nobody enumerated, found by a reviewer reading
  for it rather than by anything the build ran.

- **KYO-538** (review log `2026-08-29`, `19:49`) — the collapse of four per-family tool impls
  into `crates/kyomi-agent/src/tools/document/{read,edit,delete}.rs`. Verified by extraction
  and diff, down to the ordering:
  > Byte-diffed the four pre-refactor `AgentTool::execute`/`name`/`description`/
  > `parameters_schema`/`annotations` impls ... against the new `document::{read,edit,delete}::
  > execute_for`/`name_for`/etc. bodies they now delegate to — identical logic, identical
  > strings, identical ordering (embed-then-update-then-rechunk-then-broadcast for edit;
  > get-then-record_view for dashboard read; delete-then-broadcast-then-broadcast for delete).

  And the enumeration half was treated as its own, separate obligation:
  > Looked for a fifth silently-unified difference beyond the four the implementer listed
  > (CAS, embedding-refresh timing, read-locator-vs-view-tracking, cross-doc-type reach) —
  > none found. Annotations (`read_only_hint`/`destructive_hint`), response JSON shapes, and
  > error-message text all diffed identical to pre-refactor per-family originals.

  The four known differences each carried a `NOTE:`/doc comment naming the deferring ticket.
  That is the shape: a listed difference with a decision, not a difference that dissolved.

- **KYO-550 rework** (review log `2026-08-30`, `21:15`) — a conflict rework of an
  already-approved fix, where the claim is "same change, new base":
  > diffed `edaa0300..f3dceaa6` (original PR #439 commit, pre-conflict) against
  > `origin/main..HEAD` (this rework) with line numbers stripped — the `+`/`-` content is
  > byte-identical between the two.

- **KYO-568** (review log `2026-08-31`, `16:37`) — tracked copies of files that live outside
  the repo, where "identical apart from a provenance header" is the entire contract:
  > Byte-identity of tracked copies vs live originals, re-derived myself by locating the
  > provenance-header line offset in each file ... and diffing the stripped tracked copy
  > against the live file directly — all three IDENTICAL.

- **KYO-474** (review log `2026-08-29`, `15:05`) — the unification that did change behaviour,
  correctly and unavoidably, but was described in the diff only along the axis the ticket
  named. Routing both the create-mode picker and `EditModeCatalogTab` through the single
  `catalog_denial_key_for_type` (`crates/kyomi-ui/src/pages/settings/datasources.rs`) rather
  than `discovery_resource_key_for_type`'s `_ => "databases"` fallthrough fixed the denial
  case *and* changed the success case for BigQuery edit mode, which now populates a real
  checkbox list instead of always falling through to manual entry. The reviewer's finding was
  not that the change was wrong — it was that it was unstated:
  > Disclosure is present but narrowly framed around the denial case throughout every
  > comment; recommended (not blocking — 🟢) that the eventual commit message/PR body add one
  > explicit sentence calling out the success-path population change for BigQuery edit mode
  > specifically, since a future `git blame` reader tracing "why do BigQuery projects suddenly
  > show as checkboxes in edit mode" would otherwise have to reconstruct it from the diff.

- **KYO-468 cycle 1** (review log `2026-08-29`, `21:20`) — the `CatalogItemCheckboxList`
  extraction was accepted on a *read*, not a diff: "verified behavior-preserving for every
  non-BigQuery type ... a pure wrapping transform ... unchanged in substance." That judgement
  held. It is cited here because it is the common case, and because "unchanged in substance"
  is the sentence this rule exists to replace with a command a reviewer can re-run.

Nearest sibling is [preserve-side-effect-and-error-ordering.md](preserve-side-effect-and-error-ordering.md):
that rule names *what* a "no behaviour change" refactor must hold constant — which side
effects run, and which error surfaces first. This rule is about how you establish that it did,
and about the enumeration step that has no analogue there, because merging N copies raises a
question a single relocation never does: which copy's behaviour wins.

Distinct from [prove-a-conflict-resolution-conserved-content.md](../version-control-working-tree/prove-a-conflict-resolution-conserved-content.md)
and its line-level sibling: those cover a *hand-resolved conflict*, where the hazard is
accidental destruction in a region nobody meant to change, and the unit conserved is the item.
Here the restructuring is entirely deliberate and nothing is expected to survive verbatim in
place — what has to be conserved is behaviour, and the comparison is old body against new
body, not old file against new file.

See also [third-copy-of-test-helper-is-extraction-trigger.md](third-copy-of-test-helper-is-extraction-trigger.md)
for when to consolidate at all, and
[enumerate-consumers-from-the-type-not-from-the-diff.md](enumerate-consumers-from-the-type-not-from-the-diff.md)
for the mirror-image enumeration failure — that rule is about finding every site a change must
reach, this one about every behaviour a merge must decide.
