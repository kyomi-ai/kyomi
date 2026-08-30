# A diff comparison is not evidence of mergeability

"I compared the two diffs and the changed lines don't overlap" feels like proof that two
branches will merge cleanly. It is not. It answers a question git does not ask. Git's three-way
merge does not require the two sides' changed-line ranges to be identical to conflict — it
conflicts whenever those ranges **overlap or are directly adjacent**, with no unchanged line
separating them. Two edits with zero literally-shared lines can still collide this way: "the
changed regions are disjoint," read as "no line number appears in both diffs," was never
actually checking for adjacency, only for exact overlap — and adjacency is enough.

A second, sharper case a line-diff comparison misses entirely: two **insertions** anchored at
the same point. An insertion changes no existing line — there is nothing for a diff comparison
to flag as overlapping — but git still has to decide which insertion goes first, and it cannot
infer that from either diff alone. Two PRs that each add a new function "after `should_handle`"
in the same file conflict on merge even though neither one *changed* a line the other one
touched.

Both failure modes share the same root cause: eyeballing two diffs substitutes a mental model of
"do the changed lines overlap" for the actual algorithm git runs. The only way to know if a merge
is clean is to ask git to attempt it.

**Rule:** when asserting that two branches are mergeable — in a PR description, a handoff, or a
decision to skip a rebase — run a trial merge, don't reason from the diffs. `git merge-tree
--write-tree --name-only origin/main HEAD` computes the merge and reports conflicts with no
side effects: it does not touch the working tree, the index, or any ref. If it reports a clean
tree, the branches merge cleanly. If it doesn't, that's the conflict list, produced before either
branch had to build one from scratch.

```
WRONG — reasoning from the diffs:

  PR #439 touches lines 570-613 and 1086-1201 of chat_engine.rs.
  PR #429 touches lines 570-613 and 1086-1201 of chat_engine.rs.
  ...but PR #439's diff and PR #429's diff, side by side, change different
  individual lines within those ranges — no line overlap, so they should
  merge fine.

RIGHT — asking git:

  $ git fetch origin main
  $ git merge-tree --write-tree --name-only origin/main HEAD
  <tree-oid>
  crates/kyomi-ui/src/components/chat/chat_engine.rs
  (a conflicted path is reported by name; a clean merge prints only the tree oid)
```

Real precedent: KYO-550 (PR #439) and KYO-501 (PR #429) were both correct, independently
reviewed and approved, and both added a new helper function plus new tests to the same `"error"`
subscription handler in `crates/kyomi-ui/src/components/chat/chat_engine.rs` — #501 added
`error_event_context_type` after `should_handle`, #550 added `error_event_message` at the same
anchor. A handoff for KYO-550 asserted the two PRs "could not conflict" on the strength of a
line-by-line diff comparison finding the changed regions disjoint. #429 merged first; #439 went
DIRTY the moment it landed, because the two insertions shared an anchor point a diff comparison
never considered. The rework cost a full extra cycle: re-fetching, replaying the approved diff by
hand, and re-verifying — all of which `git merge-tree` would have surfaced before either PR was
declared safe.
