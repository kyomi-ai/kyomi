# Resolve a file claim in the tree under review, not in another clone of the same repo

Work on this repo happens in several checkouts at once: the canonical clone at
`~/repos/kyomi` plus a per-ticket worktree for each branch in flight. Every one of them has
the same paths. `grep -n`, `sed -n '1897p'`, `python3` against a fixture — all of them
succeed in any of those directories, return a plausible-looking answer, and give no hint
about which tree answered. A reviewer who opens a second terminal, or an agent whose cwd is
reset between calls, will get a real answer to a different question.

Two shapes, and the second is the one no git command will catch:

- **The other clone sits at a different revision.** Local `main` in the canonical clone is
  chronically behind `origin/main`, and a branch worktree is cut from `origin/main` at a
  specific sha. Symbols and line numbers resolve in both trees, to different numbers. A
  `file:line` citation checked in the wrong tree reads as *fabricated* rather than as
  *stale*, because the numbers do not match any revision the reader tries.
- **The other clone holds an untracked or ignored copy of the file.** Then there is no
  revision to compare and no `git status` line to notice: an e2e script left untracked in
  one tree, or a `docs/` file that this repo's `.gitignore` keeps out of git entirely, can
  carry completely different content in each directory, indefinitely.

**Rule:** bind the review to its source before resolving any path. At the top of the
review, record the absolute workspace path, its `HEAD` SHA, the pinned base SHA supplied
at dispatch, and whether the review covers committed, staged, or working-tree content.
Use `git -C <absolute-workspace>` for git commands and an explicit working directory or
absolute path for source reads; do not rely on a terminal's inherited cwd.

Resolve current `file:line` and fixture claims in the actual source under review:

- For a working-tree review, read the file on disk in that workspace.
- For a staged review, read the index (`git -C <workspace> show :<path>`). Check
  `git -C <workspace> diff -- <path>` before using disk line numbers: unstaged edits may
  make them differ from the staged file.
- For a committed review, read the bound commit (`git -C <workspace> show <HEAD-SHA>:<path>`).

A historical claim about the branch's starting revision needs a separate read of
`git -C <workspace> show <pinned-base-SHA>:<path>`. That read establishes the historical
claim only: the base contains neither staged nor uncommitted changes and cannot replace
reading the source under review. `origin/main` is a moving ref, so capture the fetched
base SHA when creating the worktree and pass that immutable value to the reviewer; do not
infer the original base from whatever `origin/main` points to later. Before calling a
citation false, establish which source it claims to describe and check that source.

When a check genuinely needs a file that only exists in another clone —
`docs/review-logs/` is gitignored here and lives only in the canonical clone — name that
clone, say why you went there, and say what class of claim it
is being used for, so the reader can tell it apart from a code `file:line` check that should
never have left the worktree. The same applies to running a script: a repo-relative tool
copied to `/tmp` resolves its own root somewhere else and can pass on an empty target set.

```bash
# WRONG — the numbers are real, and they describe a tree nobody is reviewing.
$ cd ~/repos/kyomi                      # canonical clone, local main is 37 commits behind
$ grep -n 'fn reset_bq_projects_signals' crates/kyomi-ui/src/pages/settings/datasources.rs
1897:fn reset_bq_projects_signals(...)
# → filed as CRITICAL: "the cited :1971 is false, the numbers were never right"

# RIGHT — this historical citation names the immutable revision it describes.
$ cd /home/jason/repos/kyomi-wt-kyo-658-column-embed-fk
# Review receipt also records this workspace's actual HEAD and the review scope.
$ git rev-parse --show-toplevel
$ git rev-parse HEAD
$ git rev-parse a4ce98e7bd1fbe22c33d3e8156b9e90b5790bc8c
$ git show a4ce98e7bd1fbe22c33d3e8156b9e90b5790bc8c:crates/kyomi-ui/src/pages/settings/datasources.rs \
    | grep -n 'fn reset_bq_projects_signals'
1971:fn reset_bq_projects_signals(...)
# → the historical citation is correct; current-source claims still require the
#   staged file, working-tree file, or bound commit selected for this review.
```

Real precedent — three reviews, three different ways of reading the wrong directory:

- **KYO-667**, review log `2026-09-05`, heading *"KYO-658 pre-work: new standard 'a review
  finding names a sample, not the population'"*. The entry carries a correction block
  appended after the fact:
  > **The 🔴 in this entry is FALSE — do not mine a standard from it.** It claims the file's
  > `datasources.rs` line numbers "were never right". They were correct against
  > `origin/main` (`a4ce98e7`), the revision the worktree under review was cut from …
  > The reviewer resolved the file against the canonical clone `/home/jason/repos/kyomi`,
  > whose local `main` was `49c1ad5a`, **37 commits behind**, and reported that tree's
  > numbers (`1897`/`1911`).

  The finding was rated CRITICAL and its reasoning is void. Note the specific damage: the
  reviewer did not report the citation as *stale*, they reported it as never having been
  right, because none of the numbers matched the one revision they looked at.

- **KYO-602 / KYO-558**, review log `2026-09-03`, the untimed `(session)` heading *"Two new
  coding standards: guard-fixed-for-one-branch + prove-a-flagged-secret-is-fake"* — the
  untracked-file shape, caught before it was filed:
  > independently reran the length/mod-4 check in Python against the actual fixture at
  > `scripts/e2e-regression/bigquery-create-modal.cjs:59-62` in the worktree (not the stale
  > untracked copy in the main repo, which has different placeholder values —
  > `fireball-printing-e2e` vs. `kyomi-e2e-project` — and initially caused a false alarm in
  > this review before the cwd mismatch was caught).

  No currency check would have helped: the file was untracked in the other clone, so there
  was no revision to be behind.

- **KYO-558**, review log `2026-09-03`, heading *"KYO-558 disposal-safety cfg(test) skip
  broadening (re-review, cycle 3)"* — the same hazard applied to a *tool* rather than a
  file, hit and corrected mid-review:
  > Independently re-ran the real-tree no-collateral-change check myself … rather than
  > copying the script to `/tmp` (which breaks its own `REPO_ROOT` computation — hit this
  > trap myself first before correcting it)

  and, in the same entry's Notes: copying the linter out of its own directory "silently
  yields a vacuous empty-target pass."

The habit that closes it is one sentence at the top of the review, and it is already in use:
`2026-09-05`, heading *"KYO-658 column-embedding FK-violation race (populate.rs /
indexing_service.rs / catalog_scheduler.rs)"* opens "All facts verified in this worktree,
not the canonical clone," names the worktree path and the `origin/main` sha it was cut from,
and its cycle-2 entry opens with the same sentence shortened to "All facts verified in this
worktree." The legitimate exception is stated the same way in
`2026-09-05`, heading *"Two new coding standards: reach-for-the-canonical-helper +
re-run-the-duplicate-sweep-at-rebase"*, which reads the review logs from the canonical clone
and says so explicitly — "since review logs are untracked/gitignored and don't exist in this
worktree at all — this is a local artifact check, not a code file:line check."

This is the third member of a family, and the discriminator is *which* view was wrong.
[verify-tree-is-current-before-concluding.md](verify-tree-is-current-before-concluding.md)
is about the right directory holding a tree behind the **remote**; its remedy (`git fetch`
plus a `rev-list --left-right --count`) would have caught KYO-667 and cannot help at all
with the untracked-copy case, because the file is not in git in either tree.
[verify-the-object-that-ships-not-the-working-tree.md](verify-the-object-that-ships-not-the-working-tree.md)
is about the right directory holding a tree ahead of your own **commit**. This rule is about
the **wrong directory** — a tree that is neither behind nor ahead, just not the one under
review. Sibling of
[../comments-documentation/anchor-a-citation-to-a-symbol-not-a-line-number.md](../comments-documentation/anchor-a-citation-to-a-symbol-not-a-line-number.md),
which reduces the blast radius when this happens by making citations survive drift; and of
[../comments-documentation/verify-a-precedent-claim-against-its-source.md](../comments-documentation/verify-a-precedent-claim-against-its-source.md),
which says check the claim against its source — this rule says make sure you opened the
source, and not its double one directory over.
