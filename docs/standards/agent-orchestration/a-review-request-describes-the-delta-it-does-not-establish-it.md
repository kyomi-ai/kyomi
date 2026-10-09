# A review request describes the delta — it does not establish it

A re-review arrives pre-narrowed. The request says what moved since the last signature:
*"comment-only"*, *"byte-identical to cycle 1"*, *"`scripts/foo.sh` was untouched this
cycle"*, *"nothing is committed on this branch"*, *"that file is out of this phase's
scope"*. The narrowing is the whole economy of a re-review — it is why cycle 3 costs
minutes instead of repeating everything cycle 1 proved.

It is also the one input to the review that nothing checks by default. It is not part of
the diff, so no command disagrees with it. It is prose, written by the agent whose work is
under review, and written from what that agent *intended* to change rather than derived
from git — which is exactly why it is usually right, and why the times it is wrong are the
times nobody notices.

When it is wrong the damage is specific rather than diffuse. The region the description
excluded is precisely where the unreviewed change sits, and the approval signature is bound
to the whole staged diff, not to the subset the request named. So the change nobody read
ships *signed*, with a review entry recording a scope that never covered it.

Three shapes, all seen in the `2026-09-09` and `2026-09-11` review logs:

- **The named delta undercounts.** "Nothing is committed" when the branch carries three
  commits plus a staged delta; "that script was untouched" when it has two hunks against
  `HEAD`. Both are one `git` command away, and both were stated confidently.
- **"Comment-only" is asserted rather than derived.** This is the dangerous one because it
  is true far more often than it is false, and because a diff whose *added* lines are all
  comments can still have moved an executable line.
- **The scope claim carries a justification.** "Out of phase-A scope" is not a description
  of the delta at all — it is a claim about the cost of extending it, and reading the
  proposed fix settles it. When the fix turns out to be one line in an already-adjacent
  file, the justification does not cover what it was offered for.

**Rule:** Re-derive the delta from git before you let the request bound the review, and
record the derivation in the review entry alongside what the request claimed. Prove
"comment-only" mechanically — strip comments and blank lines from both revisions and diff
the remainder, or filter the diff to non-comment lines and show the result is empty — never
by reading the hunks. Settle "file X is untouched" and "nothing is committed" with
`git diff <baseline> --name-status` and `git log <baseline>..HEAD`, and name the baseline.
Treat a stated justification for *not* looking at a file as a claim with the same standing
as any other: open the fix it declines and judge its size yourself. When git and the
request disagree, review what git says and write the discrepancy down — the discrepancy is
usually benign, and recording it is what makes the next cycle's claim checkable.

```sh
# WRONG — the request says the delta is comment-only and one script is untouched, so the
# review reads the two named files' hunks and re-signs. Nothing contradicts it.
git diff --cached -- crates/foo/src/lib.rs   # eyeball: yes, looks like comments
# → "confirmed comment-only; other files unchanged since cycle 2" — never derived

# RIGHT — derive each half, and say what the baseline was.
git diff origin/main --name-status      # the real file set, not the claimed one
git log --oneline origin/main..HEAD     # "nothing is committed" is checkable

# Comment-only, proven: drop the +++/--- file headers first (they are not content),
# then drop added/removed comment and blank lines. Empty output is the proof.
git diff --cached -U0 | grep -E '^[+-]' | grep -vE '^(\+\+\+|---)' \
    | grep -vE '^[+-][[:space:]]*(#|$)'

# Or compare executable-line streams directly against the blob you signed — the form
# that survives a diff which moved a line rather than only adding one.
diff <(git show "$signed_rev:scripts/check-ticket-in-flight.sh" | grep -vE '^[[:space:]]*(#|$)') \
     <(git show ":scripts/check-ticket-in-flight.sh"            | grep -vE '^[[:space:]]*(#|$)')
```

(Both forms were run in a scratch repo before being written here: on a comments-only edit
each returns nothing, and on an `echo hi` → `echo HI` edit each prints that one line. The
comment filters are `#`-shaped — for Rust or SQL, substitute the comment syntax rather than
assuming the pipeline transfers.)

Four instances across three tickets and two log days, all caught by the reviewer rather
than by the requester:

- **KYO-692 / KYO-713 (PR #505), `2026-09-11` log, the *cycle 4* entry under "rename and
  correct the headless sub-agent standard"** — *"the branch carries **three commits**
  (`3da98da8`, `486e4b10`, `15a92e68`) plus a staged delta — the request's 'nothing is
  committed' is inaccurate; reviewed against `origin/main`, which is the correct baseline
  either way."* The reviewer had already run `git diff origin/main --name-status` for its
  own reasons, which is the only reason the mismatch surfaced.
- **Same ticket, the *cycle 5* entry** — *"`scripts/audit-agent-run-deaths.sh` **was**
  touched this cycle — two hunks vs `HEAD` … the request's claim that it was untouched is
  inaccurate, but the content is correct and consistent with the standard."* The same cycle
  also verified a **self-directed fix** in `repro-headless-subagent-survival.sh` that no
  previous cycle had reviewed — *"(no prior reviewer saw it)"* — reachable only by diffing
  the branch rather than reading the hunks the request pointed at.
- **KYO-704 Phase A, `2026-09-11` log, the initial entry under "retire BigQuery
  `kyomi_oauth`, default to `service_account`"** — 🟡, the justification shape. A doc
  comment asserted that `onboarding_service.rs` fails closed on the retired status; the
  reviewer read it and found *"an explicit two-value allowlist, not a catch-all"*, and
  closed on the scope claim directly: the repair is *"One-line fix … not a signature
  change, so the implementer's 'out of phase-A scope' justification for not touching this
  file doesn't actually cover it."*
- **KYO-711, `2026-09-09` log, the *cycle 2* re-review under "self-test + CI registration
  for `repro-headless-subagent-survival.sh`"** — the claim was unestablished rather than
  false, and establishing it took real work: *"the requester could not isolate the
  increment via `git diff --cached` (that shows the whole uncommitted PR against HEAD,
  since nothing in this branch is committed yet)."* The reviewer recovered the
  previously-signed blob via `git fsck --unreachable --dangling` and only then confirmed
  *"every changed line since that blob is a `#` comment line."*

The derivation this rule asks for is already house practice, which is the argument for
writing it down rather than a reason not to:

- **KYO-703, `2026-09-09` log, the *cycle 2* entry under "remove the PR-listing ceiling
  from `check-ticket-in-flight.sh`"** — *"**Both changes are provably comment-only.**
  Stripped every `#`-comment and blank line from each file and diffed the remainder:
  `check-ticket-in-flight.sh` **335 executable lines identical** to the blob I signed as
  `6cf70b92…`."*
- **KYO-688, `2026-09-09` log, the initial entry under "cron sub-agent survival re-test"**
  — *"every added/removed line … begins with `#`, confirmed via
  `git diff --cached | grep -vE '^[+-]#'` returning nothing."*
- **KYO-662, `2026-09-05` log, the *cycle 2* entry under "real-world identifier lint
  gate"** — the entry opens by naming why the scope claim matters at all (*"Diff changed
  after cycle-1 sign-off … invalidating the signature"*) and then derives it: *"Isolated the
  non-comment/non-blank changed lines in the pre-commit diff by hand … no other executable
  line moved."*
- **KYO-644, `2026-09-05` log, the *cycle 2* entry under "chunk + offload catalog embedding
  batches off the async runtime"** — *"Verified the fix is comment-only — diffed … against
  the previously-reviewed state: every call site …, the chunking implementation,
  `EMBED_BATCH_SIZE`, and both regression tests are byte-identical to cycle 1."*

Mirror of
[a-review-finding-is-a-claim-to-verify-not-an-instruction.md](a-review-finding-is-a-claim-to-verify-not-an-instruction.md):
that rule governs the report travelling back from the reviewer, and tells the implementer
not to execute it unverified. This one governs the brief travelling out to the reviewer,
and tells the reviewer not to accept its scope unverified. Same handoff, same failure —
a claim read as a given — in the opposite direction.

Distinct from
[../version-control-working-tree/verify-the-object-that-ships-not-the-working-tree.md](../version-control-working-tree/verify-the-object-that-ships-not-the-working-tree.md):
that rule assumes you are deriving the delta and tells you which view answers the question
(`git diff --cached` isolates a re-review's fix-up; `git diff origin/main...HEAD` buries
it). Here the derivation never happens, because prose already supplied an answer. Distinct
from
[../version-control-working-tree/resolve-a-file-claim-in-the-tree-under-review.md](../version-control-working-tree/resolve-a-file-claim-in-the-tree-under-review.md):
there the question is right and the directory is wrong. Here the directory is right and
nobody asked git the question.

See also
[../build-toolchain/name-the-check-you-could-not-run.md](../build-toolchain/name-the-check-you-could-not-run.md):
a review narrowed on an underived claim is an undisclosed version of exactly what that rule
forbids — a check that did not happen, with nothing in the record saying so. Recording the
derivation, or recording that the request's description is what you relied on, is the
disclosure that rule asks for.
