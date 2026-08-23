# Verify against the object that will actually ship, not the working tree

`cargo check`, `cargo test`, `clippy` and every other verification command read the **working tree**. What gets pushed is a **commit**. Those two are the same thing only when nothing is staged-but-uncommitted and nothing is unstaged — and an agent mid-task is exactly when they diverge. A fix that exists only in the index, or only on disk, makes every green run a statement about a state that will never reach CI.

The failure is silent and total: the verification is genuinely green, the report is honest, and the pushed commit does not compile.

**Rule:** before signing a review, committing, or pushing, confirm that the thing you verified is the thing that ships.

- `git status --short` must show no unexpected ` M` (unstaged) or `??` (untracked) entries — either means the working tree you tested contains content the commit will not.
- When a fix is staged on top of an existing commit, verify the *combined* result, or amend first and verify after. `git show HEAD:<path>` reads the committed blob; that is what a fresh checkout gets.
- On a re-review, `git diff --cached` (index vs `HEAD`) isolates the fix-up under review. `git diff origin/main...HEAD` does not, when `HEAD` already contains prior accepted work — the actual delta disappears inside a wholesale new-file diff.
- After a rebase or cherry-pick that touched a shared append region, `git diff origin/main --numstat` proving zero deletions is the cheap mechanical check that no pre-existing line was eaten.

**WRONG** — every command green, the commit broken:

```bash
git add -p                      # fix layered into the index only
cargo test -p kyomi-ui          # reads the working tree → passes
./sign-review.sh && git push    # pushes HEAD, which lacks the fix
```

**RIGHT**:

```bash
git commit --amend --no-edit    # fold the fix into the commit first
git status --short              # must be empty
cargo test -p kyomi-ui          # now the tree and HEAD agree
```

Real precedent: on 2026-08-22 a KYO-446 review found (🔴) that `HEAD` (`0eb65102`) held an unclosed macro invocation in `datasources.rs` — the new comment block had been spliced into the middle of a preceding test's unterminated `assert!(...)`. A three-line repair sat staged but never committed, so *every* check/clippy/test run on that branch, the reviewer's included, had passed against a working tree that the pushed commit would not reproduce. Pushing as-is would have failed CI immediately and broken the build for anyone checking out that commit. Related: the 2026-08-23 KYO-424 re-review, where diffing against the merge-base instead of `HEAD` would have buried the entire fix-up inside a 700-line new-file diff, and the 2026-08-22 KYO-407 rebase, where a prior pass through the same `mod tests` tail had silently dropped a closing brace.

This is the commit-boundary sibling of `verify-tree-is-current-before-concluding.md`: that rule is about a tree behind the *remote*, this one about a tree ahead of your own *commit*. Both are conclusions drawn from a view that isn't the one that counts.
