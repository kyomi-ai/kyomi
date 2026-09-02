#!/usr/bin/env bash
# ------------------------------------------------------------------------------
# scripts/check-ticket-in-flight.sh — is anyone else already working on this
# ticket? (KYO-422)
#
# WHY THIS EXISTS
#
# On 2026-08-21 two agent workers implemented KYO-416 concurrently, producing
# PRs #367 and #368. #368 merged; #367 went CONFLICTING and had to be closed
# and cleaned up by hand. Prose guidance to check for in-flight work already
# existed in the /backlog-fast skill file before this script, but prose (a)
# can drift between the several places that need to agree with it, (b) had
# never been demonstrated to work — nobody had proven it catches a real
# double-pickup — and (c) didn't exist at all in the sibling /backlog skill.
# This script is the single, tested, executable answer both skills (and any
# future caller) invoke instead of restating the check in prose. Self-test:
# scripts/check-ticket-in-flight-test.sh.
#
# WHAT IT CHECKS (KYO-422, extended by KYO-471 — see below)
#
#   1. Remote branches   — `git ls-remote --heads <remote>`
#   2. Pull requests     — `gh pr list`, matched on `headRefName` ONLY
#   3. Local worktrees   — `git worktree list --porcelain`
#   4. Local branches    — `git branch --list`
#
# Checks 3 and 4 close a hole found in the first design (KYO-471): a worker
# whose run dies between `git commit` and `git push` leaves a complete
# implementation that is invisible to checks 1 and 2 — visible only in its
# own worktree and local branch list.
#
# DO NOT MATCH ON PR BODY OR TITLE — READ THIS BEFORE "FIXING" IT BACK (KYO-471)
#
# The KYO-422 ticket text, as originally written, recommended finding
# duplicates by searching PR *bodies* for "Closes KYO-NN". That is wrong and
# was reverted: this repo's own PR convention requires a PR body to link
# every ticket it *considered and deferred*, not just the one it implemented
# (see CLAUDE.md's "When You Discover Something Mid-Task"). A well-behaved,
# already-merged PR for ticket A routinely says "Closes KYO-A" and links
# "KYO-B" and "KYO-C" as deferred follow-ups in the same body. Searching
# bodies for "Closes KYO-<N>" produced false "in flight" hits for KYO-411,
# KYO-413, and KYO-406 — every one of them a merged PR that merely *listed*
# that ticket as a deferral, never touched it. Branch name
# (`headRefName`) is the reliable signal instead: a worker pushes
# `jason/kyo-<NN>-<slug>` before it opens anything, and a branch name is not
# reused across unrelated tickets the way a body's prose references are.
#
# MATCHING RULE — the trailing hyphen is load-bearing
#
# The slug is `kyo-<NN>-`, matched case-insensitively, WITH the trailing
# hyphen. Without it, ticket 42 would match branch `jason/kyo-422-foo` as a
# substring. The one exception: a branch that is *exactly* `...kyo-<NN>` with
# no slug after it (checked via a same-string suffix match, which is exact
# down to the character — `kyo-4220` cannot suffix-match `kyo-422`, nor can
# `kyo-14220`, because the trailing digits differ). Do not "simplify" this to
# a bare substring match.
#
# FAIL CLOSED — READ THIS BEFORE "FIXING" IT BACK (KYO-511)
#
# A false "clear" (exit 0 when work actually exists) costs a full duplicate
# implementation of a ticket. A false "in flight" (exit 3 or 1 when nothing
# exists) costs one skipped work cycle. Those are wildly asymmetric, so any
# check we cannot complete must be treated as "in flight", never as "clear".
#
# KYO-511, in this very workflow, was two bugs that failed OPEN by piping a
# command's output into `wc -l` or `head` and discarding its exit status —
# `git ls-remote ... | wc -l` returns "0" (looks like "nothing found") on a
# network failure exactly as it does on a genuinely empty result, because
# the exit status that would have told them apart was thrown away by the
# pipe. This script never pipes a status-bearing command into anything;
# every external command whose success gates a decision is captured via
# `if var="$(cmd 2>stderr_file)"; then ... else ... fi` — the `if` inspects
# that command's own exit status directly, with nothing downstream of it to
# swallow it. Do not introduce a pipe into `wc -l`/`grep -c`/`head` on the
# output of `git ls-remote` or `gh pr list` — that reintroduces KYO-511.
#
# A TRUNCATED LISTING IS ALSO A CHECK WE DID NOT COMPLETE (same rule)
#
# `gh pr list --limit N` silently returns at most N rows. This script shipped
# with `--limit 200` while the repo already had 411 PRs, so it saw back only
# as far as PR #212 and reported CLEAR for every duplicate older than that —
# the identical fail-open species as KYO-511, reached by a different route.
# The suite passed only by luck, because the PRs it asserts on (#367/#368)
# happen to be recent.
#
# Raising the number on its own does not fix this; it moves the cliff to the
# next round number and hides it again. So both halves are required, and both
# are load-bearing:
#
#   1. the limit is a named, env-overridable constant, PR_LIST_LIMIT below;
#   2. the number of rows actually returned is compared against it, and a
#      listing that comes back with >= PR_LIST_LIMIT rows is treated as
#      possibly truncated — i.e. the PR check could not be completed — and
#      exits 3, the same as a `gh` that failed outright.
#
# Do not "fix" that back to a plain exit 0 on the theory that a full page is
# a complete answer; it is exactly the case where it may not be. Raise
# PR_LIST_LIMIT instead. The row count is accumulated inside the same loop
# that already walks the one captured fetch — one `gh` call, and no pipe into
# `wc -l`, per the rule above.
#
# SELF-EXCLUSION (KYO-593 — READ THIS BEFORE "FIXING" IT BACK)
#
# The script is meant to be called twice per ticket lifecycle (see
# scripts/README.md) — once at pickup, once again right before dispatching
# code review. On both calls the caller's own branch legitimately exists, so
# it must not count as "someone else". Excluded, by exact short-name match:
#   - the branch checked out in the invoking worktree right now (unless
#     detached HEAD, or `main` — nobody's ticket branch is ever `main`)
#   - the branch named by `--self <branch>`, if given (see below)
#   - every `--ignore-branch <name>` passed on the command line
#
# THE CWD-DERIVED EXCLUSION ALONE IS NOT ENOUGH FOR THE SECOND CALL. At
# pickup, cwd is the canonical clone the worker claims the ticket from, so
# `git rev-parse --abbrev-ref HEAD` correctly resolves to the worker's own
# branch. But the skill's second call happens right before dispatching code
# review, by which point the implementation may live in a worktree while the
# caller's shell is still sitting in the canonical clone (or the reverse). In
# that shape `CURRENT_BRANCH` resolves to whatever happens to be checked out
# where the *command* runs — often `main`, which this script already
# discards as never being anyone's ticket branch — and the caller's own,
# fully-staged branch is then reported as somebody else's in-flight work.
# Observed live on the KYO-291 run, 2026-09-02: the script exited 1 naming
# the caller's own worktree; the skill's documented response to a non-zero
# exit here is "stop, do not dispatch the reviewer, treat it as a lost
# race"; a finished, staged implementation was abandoned chasing a collision
# that didn't exist.
#
# `--self <branch>` is the fix: it lets a caller name its own ticket branch
# explicitly, so self-exclusion no longer depends on which directory the
# check happens to run from. Passing it makes the check cwd-independent;
# omitting it preserves the cwd-derived behavior above unchanged, so a
# caller that genuinely does run from its own worktree needs no flag at all.
#
# `--self` IS VALIDATED; `--ignore-branch` IS NOT — DELIBERATE, NOT AN
# OVERSIGHT. `--self` adds no new suppression power over what
# `CURRENT_BRANCH` / `--ignore-branch` already have — it only changes *how*
# the caller's own branch is identified. But "my own branch" is a narrow,
# checkable claim, so it is run through the same `matches_ticket` gate every
# other candidate branch in this script is: the value MUST match KYO-<N>, or
# the script exits 2 rather than silently accepting it. Skipping that check
# would turn `--self` into an unvalidated silencer indistinguishable from
# tampering — a caller (or a bug upstream of the caller) could point it at a
# genuine competitor's branch and suppress a real collision. `--ignore-branch`
# stays deliberately unvalidated: it is the operator escape hatch, used by a
# human who already knows what they are suppressing and why, not a claim the
# script can or should be checking on their behalf. Do not add validation to
# `--ignore-branch`, and do not remove it from `--self`.
#
# `--self`'s value is captured during argument parsing but validated only
# after the ticket number is normalized, because `matches_ticket` depends on
# `SLUG_HYPHEN`/`SLUG_BARE`, which do not exist yet at parse time. Do not
# move the slug computation earlier just to validate inline in the parse
# loop — validate right after `matches_ticket` is defined instead.
#
# STRANDED-WORKTREE TOMBSTONES (KYO-529) — READ THIS BEFORE "FIXING" IT BACK
#
# Checks 3 and 4 (KYO-471) and Step 0.5's stranded-claim recovery in
# /backlog-fast deadlocked each other. Step 0.5 deliberately *preserves* the
# dead worktree of a run that died mid-ticket, so its unpushed work can be
# salvaged by hand — but a preserved dead worktree is byte-for-byte
# indistinguishable from a live worker's worktree to checks 3/4. From the
# moment a ticket is released as stranded, every future worker sees "in
# flight" forever: the ticket goes back to Backlog looking available and is
# never picked up again, because Step 0.5 itself only iterates tickets in
# In Progress. Observed on KYO-448 (twice) and KYO-463 — three permanently
# unclaimable tickets found on this machine.
#
# The fix is a marker file, `STRANDED.md`, written at a preserved worktree's
# root by scripts/mark-worktree-stranded.sh — the one place that owns the
# marker's format (KYO-422 principle: callers invoke one script rather than
# each restating the rule). This script reads it in check 3: a worktree that
# matches the ticket AND holds a valid tombstone for it is reported
# separately as "tombstoned" instead of counted in HITS, and its branch name
# is recorded so check 4 (local branches) skips that same branch too — the
# same branch checked out in that worktree would otherwise be flagged twice.
#
#   - THE TICKET-KEY REQUIREMENT IS NOT OPTIONAL. To be honoured for ticket
#     N, STRANDED.md must contain `kyo-<N>` (case-insensitive, not
#     immediately followed by another digit — the same substring-safety
#     concern `matches_ticket`'s trailing-hyphen rule exists for, so ticket
#     42 cannot be satisfied by a marker that only names kyo-422). A marker
#     that does not name the ticket is NOT honoured and the worktree stays a
#     normal hit. This keeps the fail-closed default intact against a stray
#     or copy-pasted marker, and it costs nothing: the writer always knows
#     the ticket it is tombstoning.
#   - ONLY LOCAL EVIDENCE IS SUPPRESSED — WITH ONE NARROW, PRINCIPLED
#     EXCEPTION (KYO-567, see below). STRANDED.md, a file any process with
#     filesystem access can write, suppresses the local worktree hit and the
#     local branch hit for that worktree's branch, and NOTHING ELSE. It must
#     never suppress a PR hit (check 2) — a PR has a live consumer in
#     /merge-sweeper, and no tombstone is ever a substitute for that
#     consumer having looked at it. STRANDED.md alone must also never
#     suppress a remote-branch hit (check 1) — a local file has no business
#     overruling evidence the whole team can see on the remote. Do not
#     extend STRANDED.md's suppression to checks 1/2.
#   - A SUPPRESSED TREE IS STILL REPORTED. Every tombstoned worktree is
#     printed under its own heading in the verdict block, on every verdict
#     (not only exit 0), so unsalvaged work is never silently forgotten.
#   - STALE-TOMBSTONE CAVEAT. There is no automatic staleness detection, by
#     design — this script has no way to know a human has since adopted a
#     preserved tree and resumed work in it. If that happens, the human
#     must delete STRANDED.md themselves; until they do, the tree keeps
#     reading as "preserved, not a claim" and a second worker can walk right
#     past it.
#
# STRANDED REMOTE BRANCHES (KYO-567) — READ THIS BEFORE "FIXING" IT BACK
#
# A worker that gets as far as `git push` and then dies leaves its ticket
# In Progress with a pushed branch and no PR — forever, because check 1
# (remote branches) sees that branch and reports IN FLIGHT on every future
# check, and /merge-sweeper cannot help since there is no PR to merge.
# scripts/mark-branch-stranded.sh (the writer, run at release time — see
# its own header for why the release path is a rename and not "open a PR")
# renames such a branch on the remote to `stranded/<original-name>`. This
# script honours that rename as a tombstone in BOTH check 1 (remote
# branches) and check 4 (local branches, for the symmetric local rename
# mark-branch-stranded.sh also performs when safe): a matching ref whose
# name begins `stranded/` is reported under the preserved-work heading
# instead of counted in HITS.
#
# THIS IS A DELIBERATE, NARROW EXCEPTION TO "ONLY LOCAL EVIDENCE IS
# SUPPRESSED" ABOVE — NOT A CONTRADICTION OF IT. The general rule this
# script follows is: A TOMBSTONE MAY ONLY SUPPRESS EVIDENCE NO MORE DURABLE
# THAN THE TOMBSTONE ITSELF.
#
#   - STRANDED.md is a file on one machine's disk. It cannot outrank a
#     remote branch (check 1) or a PR (check 2) — both are visible to, and
#     were created by, anyone with access to the remote. A local file must
#     never suppress evidence that durable.
#   - A `stranded/<branch>` ref lives ON THE REMOTE ITSELF, beside the
#     branch it tombstones, and can only be created by something with push
#     access to that remote — the same bar the original branch had to clear
#     to become "in flight" in the first place. It is therefore AT LEAST AS
#     durable as the remote-branch evidence it suppresses, so suppressing
#     check 1 is sound. It is still not as durable as a PR — a PR has a
#     live consumer (/merge-sweeper) with its own review/merge lifecycle —
#     so a `stranded/` rename must NEVER suppress check 2 either, and does
#     not: mark-branch-stranded.sh itself refuses to tombstone any branch
#     that has a PR in any state, and this script performs no PR-side
#     suppression at all regardless.
#
# Do not generalize this further. A local file may not suppress a remote
# branch; a remote ref may. Neither may ever suppress a PR.
#
# USAGE
#
#   check-ticket-in-flight.sh <TICKET> [--remote <name>] [--ignore-branch <name>]... [--self <branch>]
#
#   TICKET is KYO-422, kyo-422, or 422 — all equivalent.
#   --remote defaults to origin. --ignore-branch is repeatable.
#   --self <branch> names the caller's own ticket branch explicitly, so
#   self-exclusion no longer depends on cwd (KYO-593 — see SELF-EXCLUSION
#   above). Validated against the ticket; may be given at most once.
#
# EXIT CODES
#
#   0 — clear. THE ONLY CODE THAT PERMITS CLAIMING THE TICKET. May still
#       print preserved tombstoned worktrees (KYO-529) — that heading is not
#       a hit, but the path is worth a look for salvageable work.
#   1 — work in flight found (remote branch, PR, local worktree, or local
#       branch, matched and not excluded — an untombstoned local worktree
#       hit counts here too). Do not claim. If no --self was given, the
#       verdict block adds a one-line HINT to try --self (KYO-593) — a
#       message only, it changes no exit code or classification.
#   2 — usage error (missing/unparseable ticket argument, unknown flag,
#       --self given more than once, --self with no value, or a --self
#       value that does not match the ticket — see SELF-EXCLUSION above).
#   3 — a check could not be completed (remote unreachable, `gh` missing or
#       failing, or the PR listing came back at PR_LIST_LIMIT rows and may
#       therefore be truncated). Treat exactly like exit 1: do not claim.
#       --self never turns this into exit 0 — the FAILURES check still runs
#       before the HITS check, unchanged.
#
# Pure bash + git + gh. No Rust toolchain, no jq binary — the one JSON
# extraction needed (from `gh pr list`) uses gh's own built-in `--jq`, since
# gh bundles its own jq evaluator and this script should not gain a
# dependency the box might not have.
# ------------------------------------------------------------------------------

set -euo pipefail

SCRIPT_NAME="$(basename "$0")"

# How many PRs to ask `gh pr list` for. This MUST exceed the repo's total PR
# count: `gh pr list` caps the listing at `--limit` (and defaults to 30), so
# anything older than the newest N PRs is simply not looked at. Measured
# 2026-08-24: `gh pr list --state all --limit 1000 --json number --jq 'length'`
# returned 411, while the then-current `--limit 200` reached back only to PR
# #212. The same trap is written up in `.claude/build-test.md` under "Before
# Claiming a Ticket — check the remote, not just Trakkt".
#
# A big number alone is not the fix — see the truncation section of the header
# above. It is env-overridable for two reasons: an operator hitting the guard
# can raise it without editing this file, and the self-test drives it down to
# a handful of rows so it can exercise the truncation path cheaply against a
# stub `gh`.
PR_LIST_LIMIT="${PR_LIST_LIMIT:-500}"

usage() {
    cat >&2 <<EOF
Usage: $SCRIPT_NAME <TICKET> [--remote <name>] [--ignore-branch <name>]... [--self <branch>]

  TICKET                   KYO-422, kyo-422, or 422 (all equivalent)
  --remote <name>          remote to check (default: origin)
  --ignore-branch <name>   branch to exclude from matching (repeatable)
  --self <branch>          the caller's own ticket branch, excluded from
                           matching like --ignore-branch, but validated
                           against TICKET (must be given at most once)

Environment:
  PR_LIST_LIMIT            how many PRs to fetch (default: 500). Must exceed
                           the repo's total PR count; a listing that comes
                           back at this many rows may be truncated and exits 3.

Exit codes:
  0  clear — nothing in flight (the only code that permits claiming)
  1  work in flight found — do not claim
  2  usage error (including --self given twice, with no value, or naming a
     branch that doesn't match TICKET)
  3  a check could not be completed (including a possibly-truncated PR
     listing) — treat like exit 1, do not claim
EOF
}

case "$PR_LIST_LIMIT" in
    '' | *[!0-9]* | 0)
        echo "ERROR: PR_LIST_LIMIT must be a positive integer, got '$PR_LIST_LIMIT'" >&2
        exit 2
        ;;
esac

if [ "$#" -eq 0 ]; then
    usage
    exit 2
fi

TICKET_ARG="$1"
shift

REMOTE="origin"
declare -a IGNORE_BRANCHES=()
SELF_BRANCH=""
SELF_BRANCH_SET=""

while [ "$#" -gt 0 ]; do
    case "$1" in
        --remote)
            if [ "$#" -lt 2 ]; then
                echo "ERROR: --remote requires a value" >&2
                exit 2
            fi
            REMOTE="$2"
            shift 2
            ;;
        --ignore-branch)
            if [ "$#" -lt 2 ]; then
                echo "ERROR: --ignore-branch requires a value" >&2
                exit 2
            fi
            IGNORE_BRANCHES+=("$2")
            shift 2
            ;;
        --self)
            if [ "$#" -lt 2 ]; then
                echo "ERROR: --self requires a value" >&2
                exit 2
            fi
            if [ -n "$SELF_BRANCH_SET" ]; then
                echo "ERROR: --self may only be given once (already set to '$SELF_BRANCH')" >&2
                exit 2
            fi
            SELF_BRANCH="$2"
            SELF_BRANCH_SET=1
            shift 2
            ;;
        *)
            echo "ERROR: unknown argument: $1" >&2
            usage
            exit 2
            ;;
    esac
done

# ---- normalize the ticket argument to a bare digit string ------------------
ticket_lower="$(printf '%s' "$TICKET_ARG" | tr '[:upper:]' '[:lower:]')"
case "$ticket_lower" in
    kyo-*) ticket_num="${ticket_lower#kyo-}" ;;
    *) ticket_num="$ticket_lower" ;;
esac

case "$ticket_num" in
    '' | *[!0-9]*)
        echo "ERROR: could not parse a ticket number out of '$TICKET_ARG' (expected KYO-422, kyo-422, or 422)" >&2
        usage
        exit 2
        ;;
esac

SLUG_HYPHEN="kyo-${ticket_num}-"
SLUG_BARE="kyo-${ticket_num}"

# ---- matching: case-insensitive, trailing-hyphen slug or exact-suffix bare -
matches_ticket() {
    local name="$1" lower
    lower="$(printf '%s' "$name" | tr '[:upper:]' '[:lower:]')"
    case "$lower" in
        *"$SLUG_HYPHEN"*) return 0 ;;
        *"$SLUG_BARE") return 0 ;;
    esac
    return 1
}

# ---- --self validation (KYO-593 — see SELF-EXCLUSION in the header) --------
# Deferred to here, rather than validated inline in the parse loop above,
# because matches_ticket needs SLUG_HYPHEN/SLUG_BARE, which do not exist
# until the ticket argument has been normalized (just above). --self is
# validated (unlike --ignore-branch) because it claims a narrow, checkable
# fact — "this is MY branch for THIS ticket" — and an unvalidated --self
# would be an unvalidated silencer for a genuine competitor's branch.
if [ -n "$SELF_BRANCH_SET" ] && ! matches_ticket "$SELF_BRANCH"; then
    echo "ERROR: --self '$SELF_BRANCH' does not look like a branch for KYO-${ticket_num} (expected it to contain '${SLUG_HYPHEN}' or end in '${SLUG_BARE}')" >&2
    exit 2
fi

# ---- tombstone detection (KYO-529 — see the header section above) ----------
# tombstone_names_ticket <marker_file> — true iff the file exists, is a
# readable regular file, and its content names THIS ticket: `kyo-<N>`,
# case-insensitive, not immediately followed by another digit (so ticket 42
# is not satisfied by a marker that only names kyo-422 — the same
# substring-safety concern SLUG_BARE/matches_ticket exist for above, applied
# to free-form marker text instead of a branch name).
tombstone_names_ticket() {
    local marker="$1" content lower
    [ -f "$marker" ] || return 1
    content="$(cat "$marker" 2>/dev/null)" || return 1
    lower="$(printf '%s' "$content" | tr '[:upper:]' '[:lower:]')"
    case "$lower" in
        *"$SLUG_BARE") return 0 ;;
        *"$SLUG_BARE"[!0-9]*) return 0 ;;
    esac
    return 1
}

# ---- self-exclusion set -----------------------------------------------------
CURRENT_BRANCH="$(git rev-parse --abbrev-ref HEAD 2>/dev/null || echo HEAD)"

declare -a EXCLUDE=()
if [ "$CURRENT_BRANCH" != "HEAD" ] && [ "$CURRENT_BRANCH" != "main" ]; then
    EXCLUDE+=("$CURRENT_BRANCH")
fi
if [ -n "$SELF_BRANCH_SET" ]; then
    EXCLUDE+=("$SELF_BRANCH")
fi
if [ "${#IGNORE_BRANCHES[@]}" -gt 0 ]; then
    EXCLUDE+=("${IGNORE_BRANCHES[@]}")
fi

is_excluded() {
    local name="$1" e
    if [ "${#EXCLUDE[@]}" -eq 0 ]; then
        return 1
    fi
    for e in "${EXCLUDE[@]}"; do
        if [ "$name" = "$e" ]; then
            return 0
        fi
    done
    return 1
}

echo "Checking ticket KYO-${ticket_num} for in-flight work (remote: $REMOTE)"
if [ "${#EXCLUDE[@]}" -gt 0 ]; then
    echo "Excluding branches: ${EXCLUDE[*]}"
else
    echo "Excluding branches: (none)"
fi
echo

declare -a HITS=()
declare -a FAILURES=()
declare -a TOMBSTONED=()          # printable "path (branch X)" entries (KYO-529)
declare -a TOMBSTONED_BRANCHES=() # branch names check 4 must not re-flag

is_tombstoned_branch() {
    local name="$1" b
    if [ "${#TOMBSTONED_BRANCHES[@]}" -eq 0 ]; then
        return 1
    fi
    for b in "${TOMBSTONED_BRANCHES[@]}"; do
        if [ "$name" = "$b" ]; then
            return 0
        fi
    done
    return 1
}

# ---- Check 1: remote branches ----------------------------------------------
# A ref under `stranded/` (KYO-567 — see header) is a tombstone written by
# mark-branch-stranded.sh, not a live claim: it is reported separately in
# TOMBSTONED, matched by stripping the `stranded/` prefix before applying
# the same matches_ticket rule as a normal branch.
remote_stderr_file="$(mktemp)"
if remote_refs="$(git ls-remote --heads "$REMOTE" 2>"$remote_stderr_file")"; then
    while IFS=$'\t' read -r _sha ref; do
        [ -n "$ref" ] || continue
        branch="${ref#refs/heads/}"
        case "$branch" in
            stranded/*)
                orig="${branch#stranded/}"
                is_excluded "$orig" && continue
                if matches_ticket "$orig"; then
                    TOMBSTONED+=("remote branch $REMOTE/$branch (was $REMOTE/$orig)")
                fi
                ;;
            *)
                is_excluded "$branch" && continue
                if matches_ticket "$branch"; then
                    HITS+=("remote branch: $REMOTE/$branch")
                fi
                ;;
        esac
    done <<<"$remote_refs"
else
    FAILURES+=("remote branches ($REMOTE): $(cat "$remote_stderr_file")")
fi
rm -f "$remote_stderr_file"

# ---- Check 2: pull requests (headRefName only — see KYO-471 note above) ---
# Rows are counted in this same loop, over the SAME single captured fetch, so
# the truncation guard below costs no second `gh` call and nothing is piped
# into `wc -l` (see the KYO-511 note in the header).
gh_stderr_file="$(mktemp)"
if pr_lines="$(gh pr list --state all --limit "$PR_LIST_LIMIT" --json number,state,headRefName \
    --jq '.[] | [.number, .state, .headRefName] | @tsv' 2>"$gh_stderr_file")"; then
    pr_row_count=0
    while IFS=$'\t' read -r pr_number pr_state pr_branch; do
        # A zero-PR listing is the empty string, which a herestring still
        # feeds through as one empty line. That is not a row.
        [ -n "$pr_number" ] || continue
        pr_row_count=$((pr_row_count + 1))
        [ -n "$pr_branch" ] || continue
        is_excluded "$pr_branch" && continue
        if matches_ticket "$pr_branch"; then
            HITS+=("PR #${pr_number} (${pr_state}) branch ${pr_branch}")
        fi
    done <<<"$pr_lines"
    if [ "$pr_row_count" -ge "$PR_LIST_LIMIT" ]; then
        FAILURES+=("gh pr list: returned $pr_row_count rows, at the PR_LIST_LIMIT of $PR_LIST_LIMIT — the listing may be truncated, so any older PR went unchecked; re-run with a higher PR_LIST_LIMIT (e.g. PR_LIST_LIMIT=$((PR_LIST_LIMIT * 2)))")
    fi
else
    FAILURES+=("gh pr list: $(cat "$gh_stderr_file")")
fi
rm -f "$gh_stderr_file"

# ---- Check 3: local worktrees ----------------------------------------------
# A worktree that matches the ticket AND carries a valid tombstone for it
# (KYO-529 — see header) is reported separately in TOMBSTONED instead of
# HITS, and its branch is recorded in TOMBSTONED_BRANCHES so check 4 does
# not re-flag the very same worktree via its branch name.
wt_path=""
wt_branch=""
flush_worktree_entry() {
    if [ -n "$wt_branch" ]; then
        if ! is_excluded "$wt_branch" && matches_ticket "$wt_branch"; then
            if tombstone_names_ticket "${wt_path}/STRANDED.md"; then
                TOMBSTONED+=("worktree ${wt_path} (branch ${wt_branch})")
                TOMBSTONED_BRANCHES+=("$wt_branch")
            else
                HITS+=("local worktree at ${wt_path} (branch ${wt_branch})")
            fi
        fi
    fi
    wt_path=""
    wt_branch=""
}
while IFS= read -r line; do
    case "$line" in
        "worktree "*) wt_path="${line#worktree }" ;;
        "branch refs/heads/"*) wt_branch="${line#branch refs/heads/}" ;;
        "") flush_worktree_entry ;;
        *) ;;
    esac
done < <(git worktree list --porcelain)
flush_worktree_entry # in case the porcelain output has no trailing blank line

# ---- Check 4: local branches ------------------------------------------------
# Same `stranded/` handling as check 1, for the symmetric local rename
# mark-branch-stranded.sh performs when the branch isn't checked out
# anywhere (KYO-567).
while IFS= read -r branch; do
    [ -n "$branch" ] || continue
    case "$branch" in
        stranded/*)
            orig="${branch#stranded/}"
            is_excluded "$orig" && continue
            is_tombstoned_branch "$orig" && continue
            if matches_ticket "$orig"; then
                TOMBSTONED+=("local branch ${branch} (was ${orig})")
            fi
            ;;
        *)
            is_excluded "$branch" && continue
            is_tombstoned_branch "$branch" && continue
            if matches_ticket "$branch"; then
                HITS+=("local branch: $branch")
            fi
            ;;
    esac
done < <(git branch --list --format='%(refname:short)')

# ---- verdict -----------------------------------------------------------------
echo
if [ "${#TOMBSTONED[@]}" -gt 0 ]; then
    # Printed on every verdict, not only exit 0 — a tombstone suppresses the
    # claim signal, never the fact that unsalvaged/published work is sitting
    # somewhere (KYO-529 worktrees, KYO-567 remote/local branches). Each
    # entry is prefixed "worktree", "remote branch", or "local branch" so a
    # reader can tell which kind of preserved work they're looking at.
    echo "PRESERVED STRANDED WORK (not a claim — unsalvaged work lives here):"
    for t in "${TOMBSTONED[@]}"; do
        echo "  ~ $t"
    done
    echo
fi

if [ "${#FAILURES[@]}" -gt 0 ]; then
    echo "RESULT: COULD NOT COMPLETE ALL CHECKS — failing closed, do not claim KYO-${ticket_num}"
    for f in "${FAILURES[@]}"; do
        echo "  ✗ $f"
    done
    exit 3
fi

if [ "${#HITS[@]}" -gt 0 ]; then
    echo "RESULT: IN FLIGHT — do not claim KYO-${ticket_num}"
    for h in "${HITS[@]}"; do
        echo "  - $h"
    done
    # KYO-593: a message only — never changes the exit code or which entries
    # landed in HITS above. Only shown when the caller didn't already tell us
    # its own branch, since --self is exactly the fix this is pointing at.
    if [ -z "$SELF_BRANCH_SET" ]; then
        echo "  HINT: if one of the above is your own branch, you are probably running this"
        echo "        from outside your worktree — re-run with --self <your-branch>."
    fi
    exit 1
fi

echo "RESULT: CLEAR — nothing in flight for KYO-${ticket_num}"
exit 0
