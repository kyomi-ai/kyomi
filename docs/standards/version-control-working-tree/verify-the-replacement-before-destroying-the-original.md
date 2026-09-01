# Verify the replacement exists before destroying the original

Several scripts in `scripts/` mutate state nobody can reconstruct: a remote ref that carries
the only copy of a dead agent run's commits, a live file that may hold unpushed edits, a
worktree that another agent is still using. The tempting shape is do-then-check — push,
delete, then report what happened — because on the happy path it is indistinguishable from
the safe shape and one command shorter.

It diverges only on the paths nobody exercises. The push was rejected. The ref landed at a
different sha than the one fetched. The `mv` had nowhere to go because the target name was
already taken. In every one of those states the do-then-check script has already deleted the
thing it existed to preserve, and the operator learns about it from a line that scrolled
past. Worse, this is precisely the class of script that runs unattended: the destructive
step executes hours after anyone could have interrupted it.

The `-f` flags are the same mistake in one character. `git push -f`, `mv -f`, `rm` before
`cp` — each removes the check that would have caught the bad state. When a safe write is
*rejected*, that rejection is the verification: a non-force push to a ref that already
points at an unrelated commit can only fail, and its failing is what makes it impossible to
silently discard the original's history.

**Rule:** Before removing or overwriting anything a human cannot reconstruct, prove the
replacement exists and matches — re-read it from the authoritative source and compare
sha/bytes, rather than trusting the write's own exit status — and perform the destructive
step only after that comparison passes. Do all refusals (wrong branch, open PR, missing
tombstone) before the first mutation, not between mutations. Never reach for the flag that
makes the write unconditional. Every failure path must state in its own message that the
original is untouched and exit non-zero. Then pin it in the script's `*-test.sh`: force the
failure — pre-create the conflicting name, revoke the permission — and assert the original
survived. A happy-path test can never see this property, so an untested verify-before-destroy
is an unverified claim about the only code path that matters.

```sh
# WRONG — do-then-check. The delete has already run by the time anything is
# compared, and `-f` guarantees the push cannot fail even when it should.
git push -f "$REMOTE" "${SHA}:refs/heads/${STRANDED_BRANCH}"
git push "$REMOTE" ":refs/heads/${BRANCH}"
git ls-remote "$REMOTE" "refs/heads/${STRANDED_BRANCH}"   # report, not a gate

# RIGHT — non-force write, independent re-read, sha comparison, and only then
# the delete. Every abort says the original is intact and exits non-zero.
if ! git push "$REMOTE" "${FETCHED_SHA}:refs/heads/${STRANDED_BRANCH}" 2>"$push_stderr_file"; then
    echo "ERROR: failed to push refs/heads/${STRANDED_BRANCH}: $(cat "$push_stderr_file")" >&2
    echo "       Original refs/heads/${BRANCH} is untouched." >&2
    exit 1
fi
if ! verify_ref_line="$(git ls-remote --exit-code "$REMOTE" "refs/heads/${STRANDED_BRANCH}" 2>"$verify_stderr_file")"; then
    echo "ERROR: pushed refs/heads/${STRANDED_BRANCH} but could not verify it afterward." >&2
    echo "       Refusing to delete refs/heads/${BRANCH} without confirmation. Original is untouched." >&2
    exit 1
fi
VERIFIED_SHA="$(printf '%s' "$verify_ref_line" | cut -f1)"
if [ "$VERIFIED_SHA" != "$FETCHED_SHA" ]; then
    echo "ERROR: refs/heads/${STRANDED_BRANCH} points at $VERIFIED_SHA, expected $FETCHED_SHA." >&2
    echo "       Refusing to delete refs/heads/${BRANCH} without confirmation. Original is untouched." >&2
    exit 1
fi
git push "$REMOTE" ":refs/heads/${BRANCH}"
```

Real precedent — three scripts in one week, each one reviewed on this axis specifically:

- **KYO-567** (review log `2026-08-30`, `15:40`, `16:51`, `16:57`) —
  `scripts/mark-branch-stranded.sh`, which renames an abandoned pushed branch to
  `stranded/<branch>` so `check-ticket-in-flight.sh` stops reading it as a live claim. The
  branch being renamed is often the *only* copy of a dead run's commits. The reviewer traced
  it rather than assuming: *"all refusals … run and can abort before any remote mutation.
  The remote push is verified via a post-push `git ls-remote --exit-code` sha comparison,
  and the original ref is deleted only after that verification passes; every failure path
  leaves an explicit 'original is untouched' message and a non-zero exit."* Cycle 2
  re-derived why the non-force push is load-bearing: *"the script never force-pushes … so
  the push can only succeed as a fast-forward. If `stranded/<branch>` already points at a
  different, non-ancestor sha, git's own push protection rejects it — no code path can
  silently discard the original branch's commits."*
- **KYO-568** (review log `2026-08-31`, `16:37`, `kyomi-private`) —
  `scripts/link-agent-skills.sh`, which replaces live skill files with symlinks into the
  tracked copies and can therefore clobber unpushed edits. Refusal path leaves the live file
  untouched; `--force` always `cp -p`s a backup first. The reviewer proved the backup was
  load-bearing rather than decorative by removing it: *"Deleted the backup step (`cp -p --
  "$live" "$backup"`) → correctly went 31 passed / 2 failed (Test 5, Test 7 — the exact tests
  guarding the 'unpushed content must be backed up before any clobber' property)."*
- **KYO-529** (review log `2026-08-25`, `00:00`) — the reading side of the same discipline,
  in `scripts/check-ticket-in-flight.sh`. A tombstone marker *suppresses* evidence, so a
  misread suppression is destructive in effect: *"Checked every fail path in
  `tombstone_names_ticket`: missing file (`-f` false → return 1), non-regular file, and
  unreadable/permission-denied … all land on 'not tombstoned,' i.e. fail closed onto the
  pre-existing HIT path. No fail-open found."* The reviewer's own mutation (make
  `tombstone_names_ticket` unconditionally true) killed Test 18, confirming the ticket-key
  requirement is *"load-bearing, not decorative"* — the same self-test bar this rule asks for.

Distinct from [prove-a-conflict-resolution-conserved-content.md](prove-a-conflict-resolution-conserved-content.md):
that rule is about proving, after the fact, that a hand-resolved merge kept every line of
both parents in your own working tree. This one is about *ordering* — not performing the
destructive half at all until the constructive half is independently confirmed — and applies
to code that ships and runs unattended, not to a one-off resolution. Distinct from
[verify-the-object-that-ships-not-the-working-tree.md](verify-the-object-that-ships-not-the-working-tree.md),
which is about *which artifact* your verification actually covered; this is about what you
destroyed on the way there. It is
[../testing/no-git-stash-copy-file-instead.md](../testing/no-git-stash-copy-file-instead.md)
generalised beyond the reviewer's desk: that rule says back a file up with `cp` before a
mutation test and confirm byte-identity after restoring, which is exactly this ordering at
the scale of one review — this rule is the same obligation for any script that does it to
someone else's work. See also
[../error-handling/empty-on-failure-must-not-look-like-a-real-result.md](../error-handling/empty-on-failure-must-not-look-like-a-real-result.md):
that rule stops a failed *read* from producing a value the consumer trusts; this one stops a
failed *write* from being followed by a delete.
