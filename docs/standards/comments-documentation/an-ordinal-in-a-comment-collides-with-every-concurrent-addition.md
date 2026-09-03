# An ordinal in a comment collides with every concurrent addition

When you append an item to a shared enumerated collection — an array in a workflow
file, a list of gate scripts, a table of routes — the collection itself merges
cleanly. Git resolves two distinct appends to the same array without help; that is
exactly what a version-controlled list is for. What does not merge cleanly is the
*prose describing the count*: a header comment that calls your addition "the sixth
suite," or restates the collection's size nearby ("all six suites are hermetic").
Two concurrently-open PRs each append one item, each correctly calls its own
addition "the sixth," and each is right — until the other merges, at which point
both comments are simultaneously true-when-written and wrong-in-the-merged-tree,
and git has no way to reconcile English prose the way it reconciles array elements.

This is a different failure from a stale count. A stale count was wrong the moment
it was written — someone miscounted, or copied a number without checking it. An
ordinal-in-a-comment is not wrong when written: it is an accurate description of
the branch's own diff, invalidated entirely by someone else's merge landing first.
No amount of care at authoring time prevents it, because the defect isn't a
mistake — it's a race.

**Rule:** When you add to a shared enumerated collection, do not describe your
addition by its ordinal, and do not restate the collection's size in nearby prose.
Prefer a form that survives a concurrent append: derive the count mechanically from
the collection at read time rather than asserting it in words ("N suites" via
`${#suites[@]}`, not "the sixth suite" in a comment above it), or describe the
property every member shares ("every suite here is hermetic") rather than how many
members there are. If the file's existing convention still wants an explicit tally
nearby (see `name-the-invariant-not-a-count.md` on when that's the right call),
recompute and rewrite the whole sentence at merge time — never patch just the new
member in — and accept that the sentence is disposable by construction, not a
liability to avoid.

```yaml
# WRONG — two PRs each append one suite to the same array in ci.yml and each
# writes a header comment naming their own addition by ordinal. The array
# merges cleanly; these two sentences do not, because they each assert a
# total that the other PR's merge invalidates.
#
# PR #481 (KYO-617), written first:
#   # ... the newest of the six, KYO-617 ...
#   # All six suites are hermetic ...
#
# PR #483 (KYO-629), written independently, same anchor:
#   # ... KYO-629 (2026-09-03) added a sixth suite ...
#   # All six suites are hermetic ...
#
# Both were correct when written. #481 merged first; #483 went CONFLICTING
# on this exact prose although the array itself — the actual payload of both
# changes — had no conflict at all.

# RIGHT — the count is derived from the array at the point of use, so no
# comment anywhere asserts a number that a later append could invalidate.
suites=(
  scripts/check-ticket-in-flight-test.sh
  scripts/mark-worktree-stranded-test.sh
  scripts/mark-branch-stranded-test.sh
  scripts/append-review-log-test.sh
  scripts/audit-agent-run-deaths-test.sh
  scripts/reconcile-merged-tickets-test.sh
  scripts/preflight-clippy-test.sh
)
echo "All ${#suites[@]} suites passed."   # true no matter how many appends land
```

Real precedent:

- **This ticket (KYO-629).** PR #483 and PR #481 (KYO-617) both appended to
  `.github/workflows/ci.yml`'s `suites` array in the `worktree-lifecycle-selftests`
  job. The array merged without a conflict; the two header comments, each naming
  its own addition by ordinal — PR #481 called KYO-617 "the newest of the six,"
  PR #483 called its own addition "a sixth suite" — and each restating "all six
  suites are hermetic," collided on the same lines. PR #483 had been signed
  clean with zero reviewer findings and went `CONFLICTING` anyway when #481
  merged four minutes earlier — `/merge-sweeper` routed it back, costing a full
  extra agent cycle for a change nothing was actually wrong with.
- **2026-09-01 review log, line 251.** PR #448 (KYO-573) was signed clean in its
  third review cycle, then went `CONFLICTING` because PR #445 (KYO-572, `749eaa62`)
  merged first — both appended tests at the same anchor, the end of `mod tests` in
  `crates/kyomi-agent/src/adapter.rs`, and git interleaved the two independent
  appends into bogus conflict hunks. No enumeration-comment was involved here; this
  is the same shape one level down — a shared *anchor point* colliding even though
  neither PR's actual content conflicted with the other's.
- **`docs/CODING_STANDARDS.md`'s own "How to add a standard" section** already
  states this principle for the standards corpus itself: two PRs each adding a rule
  to one shared file collided on the same tail region, costing two rework cycles
  before the fix (KYO-375; PRs #337/#339 on 2026-08-13, #336 on 2026-08-12). That
  fix was structural — one file per rule, so two new rules are two new files and
  git has nothing to merge. This rule generalises the same reasoning one layer in:
  from *the file layout housing a collection* to *the prose describing a
  collection's contents*, which no file-per-item split can fix because the prose
  lives alongside the collection itself.

Sibling of [name-the-invariant-not-a-count.md](name-the-invariant-not-a-count.md):
that rule is about a count that is **inaccurate the moment it's written** — a
miscount, or a number copied without checking — and its fix is to name the
property the safety argument actually rests on instead of a tally. This rule is
about a count that is **accurate when written and invalidated by someone else's
merge**, a defect no amount of care at authoring time can prevent, because it
isn't a mistake — it's a race between two branches that can't see each other.

Sibling of
[re-derive-enumeration-comment-from-source.md](re-derive-enumeration-comment-from-source.md):
that rule is about *repairing* an enumeration comment once a reviewer has flagged
one claim in it wrong — re-read every named function fresh, don't patch around the
one finding. This rule is about *not writing the fragile sentence in the first
place*, so there is nothing left to repair when the next concurrent PR lands.

See also
[../version-control-working-tree/a-diff-comparison-is-not-evidence-of-mergeability.md](../version-control-working-tree/a-diff-comparison-is-not-evidence-of-mergeability.md),
which documents the same "two insertions at the same anchor" collision mechanism
from the verification side — how to *predict* whether two branches will conflict
before assuming they won't. This rule is upstream of that one: it is about writing
comments that never create the anchor collision to predict in the first place.
