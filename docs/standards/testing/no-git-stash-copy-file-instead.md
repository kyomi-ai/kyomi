# Never `git stash` to get a clean tree — copy the file instead

The rule above tells you to restore from a pre-mutation copy. `git stash` looks like a cheaper way to get the same clean tree for a before/after comparison. It is not: `stash`/`pop` destroys state that lives outside the index, and it destroys it silently.

The sharpest case is a merge in progress. `git stash` drops `MERGE_HEAD` / `MERGE_MSG` / `MERGE_MODE`, and `pop` does not put them back — a staged conflict resolution becomes an ordinary unstaged diff with no record that it was ever a merge. Recovering means re-identifying the original `MERGE_HEAD` commit and hand-writing those three files back into the gitdir, which for a linked worktree is `.git/worktrees/<name>/`, not the top-level `.git`. A plain stash/pop on a non-merge tree is less destructive but still unstages content that was deliberately staged.

**Rule:** When you need to compare against a clean tree mid-verification, `cp` the files you care about to a scratch location and compare against `git show <ref>:<path>`, or use a throwaway `git worktree`. Never `git stash` — and never at all when `git status` says a merge, rebase, or cherry-pick is in progress. Afterwards, confirm `git diff --cached --stat` matches what you staged before you started.

Two incidents in two days, both during review verification: the KYO-329 review (2026-08-11) stashed mid-investigation and silently unstaged the file under review; the KYO-327 merge-conflict review (2026-08-12) stashed with a resolved merge staged and lost the merge markers entirely. Both were caught and fully recovered only because the reviewer re-checked `git status` and diffed byte-for-byte before signing.
