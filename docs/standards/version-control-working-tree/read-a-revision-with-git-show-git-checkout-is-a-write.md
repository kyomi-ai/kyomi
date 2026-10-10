# Read a revision with `git show` — `git checkout` is a write, not a read

Verification constantly needs the *other* version of a file: the pre-fix blob to quote in a
WRONG block, `origin/main`'s copy of a script to run the suite against, the marker window as
it stood before a rename, the parent commit's body to diff a "behaviour-preserving" refactor
against. `git checkout <rev> -- <path>` produces it in one command and, in the sentence you
write about it afterwards, reads like a read.

It is a write. It overwrites both the **working tree** and the **index** for every path it
matches; a successful exit does not distinguish an intended update from an accidental one.
Run it from a worktree that has a reviewed diff staged and the diff is no longer what you reviewed — which matters
most in exactly the situation that motivates the command, because a review is the one context
where "read the old version of this file" and "a diff is staged" are simultaneously true.

Two shapes have been observed, both during review of a staged diff, and the second is the one
nobody expects:

- **Content destroyed.** The plain form. `git checkout <rev> -- .` replaces the staged fix on
  disk *and* in the index with the historical blob. When that blob matches `HEAD`, the path
  drops out of `git status --porcelain` altogether: the diff simply got smaller. A different
  historical blob can leave the path listed while silently replacing the reviewed content.
- **Index narrowed, content intact.** `--work-tree=<other-dir>` without a matching
  `--git-dir`. Git discovers the gitdir from cwd — the real worktree — so the checked-out
  *content* lands in the other directory while *this* index is reset to the historical blob
  for every matched path. On-disk content is untouched, so every subsequent `cargo test`
  still compiles the reviewed code and still passes. Only the index shrank. That is the worse
  of the two: the tree looks right, the suite is green, and the commit — together with any
  review of its diff — now covers less than what was read.

Neither shape announces itself. Compare the staged diff and working-tree diff before and
after verification, alongside `git status --porcelain`. Status detects the illustrated
changes in staged state, but unchanged status letters do not prove unchanged bytes.

**Rule:** while a diff is staged — during a review, a mutation test, or any before/after
comparison — read a historical revision without changing the active tree or index:
`git show <rev>:<path> > "$scratch"`, `git diff <old-rev> <new-rev> -- <path>`, or
`git cat-file`. If you need a runnable checkout, create a separate throwaway worktree in
a scratch directory; that writes its own tree and Git metadata. Do not reach for
`git checkout <rev> -- <path>` or `git restore --source=<rev> -- <path>` as a read:
plain `restore` overwrites working-tree content, while `restore --staged` writes the index.
Passing `--work-tree` does not isolate an index-writing command: the index is selected by
`--git-dir` (or an explicit index override), not by `--work-tree`. Snapshot
`git status --porcelain`, `git diff --cached --binary`, and `git diff --binary` before
verification and compare all three afterwards. Check their exit statuses; a failed read is
not evidence of an unchanged tree. Use scratch copies for any untracked files you inspect.
`git checkout <ref> -- <path>` is the right command only when landing that content in the tree *is* the change you intend, which is the rescue case in
[rescued-work-is-stale-by-construction.md](rescued-work-is-stale-by-construction.md).

Historical reproduction recorded on 2026-09-16; rechecked during this rescue in a throwaway
repo: three files edited and staged — `scripts/foo.txt` and `.githooks/bar.txt` inside the checked-out paths,
`docs/baz.txt` outside them.

```sh
# Starting point for each shape — BASE is HEAD; everything staged, nothing unstaged:
#   M  .githooks/bar.txt
#   M  docs/baz.txt
#   M  scripts/foo.txt

# WRONG (shape 1) — reading the pre-fix file by checking it out. `git status`
# afterwards loses the path entirely; the staged edit is gone from the index
# AND from disk, and `git show :scripts/foo.txt` returns the historical blob.
git checkout "$BASE" -- scripts/foo.txt
#   M  .githooks/bar.txt
#   M  docs/baz.txt

# WRONG (shape 2) — the same read aimed at a scratch directory. The historical
# content lands in "$OTHER"; `scripts/foo.txt` on disk still reads "reviewed
# foo". Only the index moved, for the two paths the checkout targeted:
git --work-tree="$OTHER" checkout "$BASE" -- scripts .githooks
#    M .githooks/bar.txt
#   M  docs/baz.txt
#    M scripts/foo.txt

# RIGHT — same bytes, no write. `git status --porcelain` is byte-identical
# before and after.
git show "$BASE":scripts/foo.txt > "$scratch"
```

Real precedent — two tickets in two days, recovered and corrected before final approval:

- **KYO-722, `2026-09-10` log, the entry headed "repoint the panic-context end marker + new
  standards rule"** — shape 1, self-disclosed at the end of an otherwise-clean review:
  *"mid-review, a `git checkout c964db91 -- .` used to pull the pre-diff file for the
  historical marker sweep briefly reverted the staged `feedback_context.rs` change in this
  worktree. Caught immediately via `git diff --cached --stat`, reapplied by hand, and
  confirmed byte-identical … before final tests/clippy were run."* The purpose was a read —
  a historical sweep for how many markers pinned a full signature.
- **KYO-712, `2026-09-11` log, the three entries headed "bind review signature to the diff it
  approved"** — shape 2, and the reason the ticket's fix is built the way it is. The reviewer
  ran the `--work-tree` form during *cycle 1* of the review of the very ticket that exists to
  detect a narrowed index; the cycle-2 entry records *"no repeat of the cycle-1 `--work-tree`
  mutation incident; all pre-fix/post-fix comparisons this cycle used `git show <rev>:<path>`
  into `mktemp -d` scratch files … never a working-tree mutation."* The cycle-3 entry
  reproduced the transcript in a throwaway repo: because `--git-dir` remained
  discovered from the current worktree, Git updated that worktree's index while
  writing content into the unrelated `--work-tree` directory. Its own on-disk
  files stayed unchanged.

This does not identify the uncaptured command behind KYO-676; it reproduces a
class of command observed during KYO-712. The former signing gate has been
retired. Avoid index-mutating reads regardless of which lint or review controls
are active: no gate can reconstruct an edit overwritten on disk and in the index.

Sibling of
[../testing/no-git-stash-copy-file-instead.md](../testing/no-git-stash-copy-file-instead.md):
that rule covers **restoring** after you have deliberately mutated a file — the remedy is a
`cp` backup, and the prohibited command is `git stash`, which destroys merge state. This rule
covers **reading** a revision you never intended to land, where nothing needs restoring
because nothing should have been written; the prohibited command is `git checkout`, and it
overwrites index entries rather than discarding `MERGE_HEAD`.

Distinct from
[rescued-work-is-stale-by-construction.md](rescued-work-is-stale-by-construction.md): there
`git checkout <stranded-ref> -- <path>` is the correct command and is prescribed by name,
because landing the recovered content in the tree is the whole point of the change. The
hazard here is the same command used when you want the *bytes* and not the *state* — and the
tell is whether a diff you care about is staged when you run it.

See also
[verify-the-object-that-ships-not-the-working-tree.md](verify-the-object-that-ships-not-the-working-tree.md):
that rule is why shape 2 is the expensive one — verification reads the working tree, the
commit is what ships, and a checkout that moves only the index separates them while leaving
every green run honest. And
[../comments-documentation/quote-a-wrong-block-from-the-pre-fix-commit.md](../comments-documentation/quote-a-wrong-block-from-the-pre-fix-commit.md):
its remedy, `git show <fix-sha>^:<path>`, is already the read-only form — this rule is the
reason to reach for it even when you only want to *look*.
