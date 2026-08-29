# Prove a conflict resolution conserved content — a line count cannot

Every branch that adds a test, a match arm, or a migration appends to the same tail region
another branch is also appending to, so conflicts there are routine rather than exceptional.
Resolving one goes wrong in two ways, and only one of them is a deletion.

Content gets **eaten**: a hand-resolved hunk absorbs or drops a pre-existing item's closing
brace, and the file may still parse because a neighbour's brace stands in for it. Content
gets **spliced**: the incoming block lands *inside* a surviving item rather than after it,
leaving an unterminated macro invocation — a pure insertion, with nothing deleted anywhere.

Neither shows up in the checks that get run. `--numstat` reporting zero deletions rules out
the first shape and is silent on the second. A green `cargo test` proves the working tree
parses, which is a different object than the commit. And whole-file brace counting — the
obvious mechanical check — is defeated by this repo's own source-text guard tests, whose
assertion strings contain literal `{`/`}` (e.g. `"connect_blocked=Signal::derive(move || {"`),
permanently desyncing a naive depth counter for everything below the first one.

The unit that has to be conserved is the **item**, not the line.

**Rule:** After resolving a conflict in an append-only region, prove conservation *before*
running any build or test:

1. `grep -n '^<<<<<<<\|^=======\|^>>>>>>>'` on the file returns nothing.
2. `git diff origin/main --numstat` shows zero deletions — necessary, never sufficient.
3. Compare the *item sets*, not counts: sorted `#[test] fn` names, `mod` names, migration
   filenames, extracted from both sides independently. Sets equal, no duplicates on either.
4. `diff` each item that sat adjacent to the conflict, extracted from `origin/main` and from
   your resolution. Slice by indentation boundary (`    fn NAME(` to the next `    }` at the
   same indent), not by brace depth.
5. Read the seams themselves — the last line of the incoming block and the first line of
   whatever follows it — rather than inferring them from the diff.

```bash
# WRONG — zero deletions is the whole check, so an insertion-only splice into
# the middle of a preceding test's assert!(...) sails through, and every later
# green run is measuring the working tree rather than the commit.
git diff origin/main --numstat        # 321  0   → "nothing was lost"
cargo test -p kyomi-ui --features ssr # green

# RIGHT — markers, then deletions, then the item set, then the seam.
grep -n '^<<<<<<<\|^=======\|^>>>>>>>' "$f" && exit 1
git diff origin/main --numstat -- "$f"
git show origin/main:"$f" | grep -oP '(?<=^    fn )\w+' | sort > /tmp/before.txt
grep -oP '(?<=^    fn )\w+' "$f" | sort > /tmp/after.txt
diff /tmp/before.txt /tmp/after.txt    # must be a pure addition, no removals
sed -n '/^    fn the_adjacent_test(/,/^    }/p' "$f"   # read the seam
```

Real precedent, six reviews in two days, all on the `mod tests` tail of one file family:

- **KYO-446 cycle 2** (review log `2026-08-22`, `11:40`) — 🔴, refused signing. The committed
  blob was missing the `);` and `}` closing a preceding test's final `assert!(...)`, with the
  incoming comment block "spliced directly into the middle of that unclosed macro call." An
  insertion, not a deletion. It took two further cycles to land.
- **KYO-407 rework** (`2026-08-22`, `20:47`) — the same hazard, one PR earlier: a cherry-pick
  had cost KYO-413's `bigquery_service_account_next_disables_after_validate_then_remove` its
  closing brace. The repair was verified by diffing 10 576 lines of `origin/main`'s copy
  against the new file's, plus `git diff --staged | grep '^-'` returning only the diff header.
- **KYO-407 rebase** (`2026-08-23`, `00:49`) — the review is explicit that "a prior rebase of
  this same branch left a pre-existing test unclosed via this exact hazard," and was scoped
  deliberately to *mechanical proof no line was destroyed* rather than to re-litigating
  content: zero-deletion numstat, zero conflict markers, both competing test names present
  and passing.
- **KYO-452 addendum** (`2026-08-23`, `13:52`) — the source of item 4. Naive brace counting
  was tried and abandoned because the guard tests' own assertion strings carry literal braces;
  all eight tests on both sides of the conflict were extracted by indentation boundary and
  diffed individually, and the two conflict-boundary transitions were read directly. One
  branch's test had landed interleaved between another's tests 2 and 3 — correct, but only
  provably so by reading it.
- **KYO-455** (`2026-08-23`, `14:58`) — a 2 978-line test-module move, verified by exactly the
  set comparison in item 3: sorted `#[test] fn` names extracted independently from both sides,
  byte-identical at 74/74 with no duplicates, plus an `md5sum` on the untouched production
  half.
- **KYO-491** (`2026-08-23`, `23:52`) — `store.rs`'s brace-hoist conflict resolved with both
  `mod reconciliation_tests` (4 tests) and `mod tests` (3) confirmed present and passing;
  the review names this as the highest-risk part of the diff.

The commit-boundary sibling is
[verify against the object that will actually ship](verify-the-object-that-ships-not-the-working-tree.md),
which lists the zero-deletion numstat as one cheap check among several — this rule is why
that check alone is not enough, and what to run after it. The structural cure is
[one test topic per file](../code-organization/one-test-topic-per-file-not-one-big-mod-tests.md):
two branches appending to two different topic files have nothing to resolve.
