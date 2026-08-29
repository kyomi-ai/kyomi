# Prove a hand-resolved conflict conserved every line — with a name set, not a brace count

Several agents work the same files at once and every one of them appends its new tests to the
end of a `mod tests` block, so hand-resolved conflicts, rebases, and cherry-picks in that tail
are routine. The KYO-455 topic-file split narrowed the collision surface; it did not remove it
— `datasources/tests/oauth.rs` alone now carries 49 tests and is the next tail everyone
appends to. Two things have gone wrong at that seam repeatedly: a closing `}` or `);` gets
eaten at the join, and a whole test body gets absorbed into its neighbour.

The second one is the dangerous one, because **the compiler cannot see it**. A test that a
resolution swallowed into the preceding test's body still compiles, still passes, and simply
stops existing — the suite reports a smaller number that nobody was comparing against
anything. A green `cargo test` after a rebase is not evidence that the rebase kept your
tests; it is evidence that whatever survived compiles.

Two mechanical checks are commonly reached for and neither is sufficient on its own:

- **`git diff origin/main --numstat` showing zero deletions** proves conservation only when
  the branch is a pure append. A genuine two-sided resolution has legitimate deletions, and
  the eaten line hides among them.
- **Brace-depth counting** to extract a test body is not valid on this codebase's Rust source.
  The source-text guard tests search for string literals that themselves contain `{` and `}` —
  for example `"connect_blocked=Signal::derive(move || {"`, live in
  `datasources/tests/oauth.rs` — so a naive depth counter desyncs at the first such literal
  and stays wrong for the rest of the file.

**Rule:** after resolving a conflict, rebasing, or cherry-picking into a shared append region,
prove conservation by comparing *named things* on both sides, not by counting lines or braces.
Extract the sorted set of `#[test] fn` names from `origin/main`'s copy and from yours and diff
the two sets; `md5sum` the untouched production region above the seam; and read both sides of
each join directly to confirm every body closes before the next `#[test]` opens. Then
`grep -c '^<<<<<<<\|^=======\|^>>>>>>>'` for leftover markers, and state the resulting test
count in the PR so a reviewer can compare it against the pre-rebase count. If you must extract
a body programmatically, anchor on indentation (`    fn NAME(` to the next `    }` at the same
four-space indent), not on brace depth.

```sh
# WRONG — a plausible-looking diffstat and a green suite. Both are satisfied by
# a resolution that swallowed one test into the body of the one above it: the
# swallowed body still compiles, and the pass count nobody wrote down went down.
git diff origin/main --numstat         # some insertions, some deletions — fine?
cargo test -p kyomi-ui --features ssr  # "N passed, 0 failed" — N compared to what?

# RIGHT — compare the named things on both sides of the resolution.
F=crates/kyomi-ui/src/pages/settings/datasources/tests/oauth.rs
names() { grep -A1 '#\[test\]' "$1" | grep -oE 'fn [a-z0-9_]+' | sort; }

git show origin/main:"$F" > /tmp/before.rs
names /tmp/before.rs > /tmp/before.txt
names "$F"           > /tmp/after.txt
diff /tmp/before.txt /tmp/after.txt   # must be exactly your intended additions,
                                      # never a deletion you did not author

# And read every removed line, not just the count: on a resolution whose only
# legitimate change is an append, `diff` must show additions and nothing else.
diff /tmp/before.rs "$F" | grep '^<'   # each `<` line must be one you meant to remove

grep -c '^<<<<<<<\|^=======\|^>>>>>>>' "$F"   # 0 — no leftover markers
```

Real precedent — six reviews across two days, all on the same class of seam:

- **KYO-446 (2026-08-22, cycles 2 and 3 at `11:40` / `12:05`)** — 🔴. The rebase's own hand
  resolution spliced the KYO-446 comment block into the *middle* of the KYO-429 test's final
  unclosed `assert!(...)`, dropping its `);` and `}`. `git show HEAD:<path>` did not parse.
  Every green check on that branch — the reviewer's included — had run against a working tree
  carrying an uncommitted repair.
- **KYO-407 (2026-08-22 `20:47`)** — a cherry-pick lost the closing brace of KYO-413's
  `bigquery_service_account_next_disables_after_validate_then_remove`. The repair was proven by
  `diff`ing the first 10,576 lines of `origin/main`'s copy against the new file's, not by
  re-running the suite.
- **KYO-407 (2026-08-23 `00:49`)** — the second rebase of the same branch, reviewed on the
  explicit basis that *"a prior rebase of this same branch left a pre-existing test unclosed
  via this exact hazard."* Proof was numstat zero-deletions (valid here — a pure append),
  zero conflict markers, and both target tests present *and* passing, neither having replaced
  the other.
- **KYO-452 (2026-08-23 `13:52` addendum)** — the brace-counting trap, discovered live:
  *"Naive whole-file brace-counting is unreliable here — two of the three test bodies contain
  literal `{`/`}` characters inside string literals being searched for … which permanently
  desyncs a naive depth counter for the rest of the file."* Resolved by extracting all eight
  tests by indentation boundary on both sides and diffing each.
- **KYO-455 (2026-08-23 `14:58`)** — the 2,978-line `mod tests` split, verified by exactly the
  technique this rule prescribes: sorted `#[test] fn` name sets extracted independently from
  both sides and confirmed byte-identical at 74/74 with no duplicates either side, plus an
  `md5sum` of lines 1-8923 (everything above the old `mod tests {`) proving the production
  region did not move.
- **KYO-491 (2026-08-23 `23:52`)** — a "brace-hoist" conflict in `cache/store.rs` where two
  sibling test modules could have collapsed into one; conservation shown by confirming both
  `mod reconciliation_tests` (4 tests) and `mod tests` (3 tests) were present and passing.

Sibling of
[verify-the-object-that-ships-not-the-working-tree.md](verify-the-object-that-ships-not-the-working-tree.md),
whose numstat bullet is the cheap version of this check: that rule is about verifying the
*commit* rather than the tree, this one about what to compare once you have the right object.
The structural prevention is
[one test topic per file](../code-organization/one-test-topic-per-file-not-one-big-mod-tests.md)
— two branches touching different topic files have no seam to resolve.
