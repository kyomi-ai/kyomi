#!/usr/bin/env bash
# ------------------------------------------------------------------------------
# scripts/mark-branch-stranded.sh — tombstone a pushed-but-abandoned remote
# branch so check-ticket-in-flight.sh stops reading it as a live claim forever
# (KYO-567). The writer half of the `stranded/` remote namespace; the reader
# half is check-ticket-in-flight.sh's checks 1 and 4.
#
# WHY THIS EXISTS — READ THIS BEFORE "FIXING" IT BACK TO OPTION 1
#
# A /backlog-fast or /backlog run that gets as far as `git push` and then dies
# leaves its ticket In Progress with a pushed branch and no PR. Nothing ever
# releases it: Step 0.5's stranded check short-circuits at "a branch exists on
# the remote ⇒ not stranded" and never reaches the elapsed-time/worktree-quiet
# checks that would otherwise flag it, check-ticket-in-flight.sh's check 1
# then reports that remote branch as in-flight forever, and /merge-sweeper
# cannot help because there is no PR and the ticket is not In Review. Two
# tickets were found stuck this way: KYO-534 (~12.7h) and KYO-463 (its third
# death, ~3.5 days).
#
# The ticket text's own top suggestion — open a PR during release so
# /merge-sweeper can pick it up — was investigated and rejected on evidence,
# not speculation. Both reproduction branches contain ONLY a mined-coding-
# standards commit, never the ticket's actual work:
#
#   jason/kyo-534-per-surface-max-tokens → 1 commit, a docs/standards/**.md
#     file. The real fix (5 modified source files) is UNCOMMITTED in the
#     worktree — never reached the branch at all.
#   jason/kyo-463-spec-green-run → 1 commit, two docs/standards/**.md files.
#     Zero ticket work.
#
# This is not a coincidence: the workflow's own defensive convention-mining
# step commits standards to the branch BEFORE ticket work starts, precisely
# because that step has died repeatedly. So the first (and often only)
# pushed commit is routinely unrelated to the ticket — "a branch was pushed"
# carries no evidence the ticket work exists. Opening a `Closes KYO-NNN` PR
# on such a branch would hand /merge-sweeper a green, mergeable PR that marks
# a real, unfixed bug Done — a SILENT FALSE COMPLETION, strictly worse than
# the permanent skip this ticket fixes. Making it a draft PR instead fails
# differently, not better: /merge-sweeper skips drafts outright (see its
# SKILL.md), so a draft PR has no consumer at all — the same invisibility,
# now wearing a PR's clothes.
#
# So the release path is a TOMBSTONE, not a PR: rename the branch to
# `stranded/<branch>` on the remote (symmetric with KYO-529's STRANDED.md for
# worktrees). The work stays published — more durable than local-worktree
# preservation — and there is no PR for anything to wrongly merge.
#
# THE ASYMMETRY WITH STRANDED.md IS LOAD-BEARING
#
# check-ticket-in-flight.sh's header states, correctly, that a local
# STRANDED.md tombstone must NEVER suppress a remote-branch hit (check 1) or
# a PR hit (check 2) — only local evidence (checks 3/4). This script's
# `stranded/` ref is a narrow, principled EXCEPTION to that: it suppresses a
# remote-branch hit (check 1) directly. That is not a contradiction, because
# the two markers have different durability:
#
#   - A local STRANDED.md is a file ANY process with filesystem access can
#     write, on a machine nobody else can see. It must never be able to
#     unblock PUBLISHED work — a local claim has no business overruling
#     evidence the whole team can see.
#   - A `stranded/<branch>` ref lives on the remote ITSELF, right alongside
#     the branch it tombstones, and can only be created by something with
#     push access to that remote — the same bar the original branch had to
#     clear to become "in flight" in the first place.
#
# The general rule: A TOMBSTONE MAY ONLY SUPPRESS EVIDENCE NO MORE DURABLE
# THAN THE TOMBSTONE ITSELF. A local file may not suppress a remote branch;
# a remote ref may suppress a remote branch. Neither may EVER suppress a PR
# (check 2) — a PR has a live consumer in /merge-sweeper, and no tombstone
# of any kind is a substitute for that consumer having looked at it and
# closed it out. See check-ticket-in-flight.sh's own header for how it
# applies this rule on the reading side.
#
# WHAT IT DOES
#
#   1. Refuses if `<branch>` already has a PR in any state — that branch
#      belongs to /merge-sweeper, not this script.
#   2. Refuses if `<branch>`, as a local branch, is checked out in some
#      OTHER worktree without that worktree already carrying a valid
#      STRANDED.md for this ticket — done BEFORE any remote mutation, so a
#      refusal here never leaves the remote half-migrated.
#   3. Resolves the remote sha of `<branch>`, fetches it locally, pushes it
#      to `stranded/<branch>`, and CONFIRMS the new ref exists on the remote
#      and points at that same sha before deleting the original ref. Never
#      deletes before the copy is verified. Any failed step aborts leaving
#      the original ref exactly as it was.
#   4. If a local branch of that name exists and is not checked out
#      anywhere, renames it to `stranded/<branch>` too, so a local
#      `git branch --list` matches the same durability rule as the remote.
#      This rename is best-effort (a WARNING, not a fatal error) — see that
#      block below for the residual local-only inconsistency if it fails.
#

# REFUSALS (fail closed)
#
#   - Branch `main` — same assumption mark-worktree-stranded.sh makes; no
#     ticket branch is ever main.
#   - A branch with a PR in any state, matched via `gh pr list --head
#     <branch>` (server-side EXACT head-branch filter — not a body/title
#     search, per check-ticket-in-flight.sh's KYO-471 note on why that's the
#     only safe way to match a branch to a PR).
#   - A branch already under `stranded/` — this is idempotency, not an
#     error: it reports "already tombstoned" and exits 0.
#   - If the remote `refs/heads/<branch>` is already gone AND
#     `stranded/<branch>` already exists (a prior run completed, or died
#     between push and delete and this is a retry): also reports "already
#     tombstoned" and exits 0, rather than a confusing "no such branch"
#     error on a retry of a script that is meant to be safely re-run.
#
# USAGE
#
#   mark-branch-stranded.sh <TICKET> --branch <name> [--remote <name>] [--note <text>]
#
#   TICKET      KYO-567, kyo-567, or 567 (all equivalent) — normalization is
#               deliberately duplicated from check-ticket-in-flight.sh /
#               mark-worktree-stranded.sh rather than factored into a shared
#               lib; see mark-worktree-stranded.sh's header for why (it's
#               ~10 stable lines, and the thing KYO-422 consolidates is the
#               MARKER FORMAT, not ticket-string parsing).
#   --branch    required. The remote branch to tombstone.
#   --remote    default: origin.
#   --note      optional free text, folded into the printed summary.
#
# Must be run with the current working directory inside the repo whose
# remote and local branches are being tombstoned — same assumption every
# other script in this file makes (no --repo/--worktree flag; the LOCAL
# branch check operates on `git worktree list` / `git branch` for whatever
# repo the cwd resolves to).
#
# EXIT CODES
#
#   0 — tombstoned (or was already tombstoned — idempotent).
#   1 — error: branch is main, branch has a PR, a local worktree checkout
#       lacks a valid tombstone, the branch doesn't exist on the remote,
#       any git/gh step failed, or a post-push verification mismatch.
#   2 — usage error (missing/unparseable ticket, missing --branch, unknown
#       flag, missing value for a flag).
# ------------------------------------------------------------------------------

set -euo pipefail

SCRIPT_NAME="$(basename "$0")"
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

usage() {
    cat >&2 <<EOF
Usage: $SCRIPT_NAME <TICKET> --branch <name> [--remote <name>] [--note <text>]

  TICKET            KYO-567, kyo-567, or 567 (all equivalent)
  --branch <name>   required — the remote branch to tombstone
  --remote <name>   remote to operate on (default: origin)
  --note <text>     optional free text folded into the printed summary

Exit codes:
  0  tombstoned (or already tombstoned — idempotent)
  1  error (main, has a PR, worktree checkout lacks a tombstone, branch
     missing on remote, a git/gh step failed, post-push verification
     mismatch)
  2  usage error
EOF
}

if [ "$#" -eq 0 ]; then
    usage
    exit 2
fi

TICKET_ARG="$1"
shift

BRANCH=""
REMOTE="origin"
NOTE=""

while [ "$#" -gt 0 ]; do
    case "$1" in
        --branch)
            if [ "$#" -lt 2 ]; then
                echo "ERROR: --branch requires a value" >&2
                exit 2
            fi
            BRANCH="$2"
            shift 2
            ;;
        --remote)
            if [ "$#" -lt 2 ]; then
                echo "ERROR: --remote requires a value" >&2
                exit 2
            fi
            REMOTE="$2"
            shift 2
            ;;
        --note)
            if [ "$#" -lt 2 ]; then
                echo "ERROR: --note requires a value" >&2
                exit 2
            fi
            NOTE="$2"
            shift 2
            ;;
        *)
            echo "ERROR: unknown argument: $1" >&2
            usage
            exit 2
            ;;
    esac
done

if [ -z "$BRANCH" ]; then
    echo "ERROR: --branch is required" >&2
    usage
    exit 2
fi

# ---- normalize the ticket argument to a bare digit string ------------------
# Deliberately duplicated — see the header above.
ticket_lower="$(printf '%s' "$TICKET_ARG" | tr '[:upper:]' '[:lower:]')"
case "$ticket_lower" in
    kyo-*) ticket_num="${ticket_lower#kyo-}" ;;
    *) ticket_num="$ticket_lower" ;;
esac

case "$ticket_num" in
    '' | *[!0-9]*)
        echo "ERROR: could not parse a ticket number out of '$TICKET_ARG' (expected KYO-567, kyo-567, or 567)" >&2
        usage
        exit 2
        ;;
esac

TICKET_KEY="KYO-${ticket_num}"
SLUG_BARE="kyo-${ticket_num}"

# ---- refuse main -------------------------------------------------------------
if [ "$BRANCH" = "main" ]; then
    echo "ERROR: refusing to tombstone branch 'main'." >&2
    exit 1
fi

# ---- idempotency: --branch already under stranded/ --------------------------
case "$BRANCH" in
    stranded/*)
        echo "ALREADY TOMBSTONED: '$BRANCH' is already under the stranded/ namespace — nothing to do."
        exit 0
        ;;
esac

STRANDED_BRANCH="stranded/${BRANCH}"

# ---- refuse if the branch has a PR (any state) -------------------------------
# Server-side EXACT head-branch filter, not a substring/body/title search —
# see check-ticket-in-flight.sh's KYO-471 header note on why that's the only
# safe way to tie a branch to a PR. Extraction follows check-ticket-in-flight.sh's
# own convention: gh's built-in --jq to TSV, looped and counted by hand — never
# a status-discarding pipe into wc -l/grep -c/head on gh's output (KYO-511).
pr_stderr_file="$(mktemp)"
if pr_lines="$(gh pr list --head "$BRANCH" --state all --json number,state \
    --jq '.[] | [.number, .state] | @tsv' 2>"$pr_stderr_file")"; then
    rm -f "$pr_stderr_file"
    pr_count=0
    declare -a pr_rows=()
    while IFS=$'\t' read -r pr_number pr_state; do
        [ -n "$pr_number" ] || continue
        pr_count=$((pr_count + 1))
        pr_rows+=("#${pr_number} (${pr_state})")
    done <<<"$pr_lines"
    if [ "$pr_count" -gt 0 ]; then
        echo "ERROR: refusing to tombstone '$BRANCH' — it has ${pr_count} PR(s):" >&2
        printf '       %s\n' "${pr_rows[@]}" >&2
        echo "       A branch with a PR belongs to /merge-sweeper, not this script." >&2
        exit 1
    fi
else
    echo "ERROR: 'gh pr list --head $BRANCH' failed — cannot verify '$BRANCH' has no PR, refusing to guess: $(cat "$pr_stderr_file")" >&2
    rm -f "$pr_stderr_file"
    exit 1
fi

# ---- pre-flight: local branch checked out elsewhere needs its own tombstone
# Done BEFORE any remote mutation below: a refusal here must never leave the
# remote half-migrated (renamed) while the local worktree still reads as a
# live, unexplained claim to check-ticket-in-flight.sh's check 4.
worktree_tombstone_names_ticket() {
    # worktree_tombstone_names_ticket <worktree_path> — true iff
    # <worktree_path>/STRANDED.md exists and names THIS ticket. Mirrors
    # check-ticket-in-flight.sh's tombstone_names_ticket(): `kyo-<N>`,
    # case-insensitive, not immediately followed by another digit (so
    # ticket 42 is not satisfied by a marker that only names kyo-422).
    local worktree_path="$1" marker="${1}/STRANDED.md" content lower
    [ -f "$marker" ] || return 1
    content="$(cat "$marker" 2>/dev/null)" || return 1
    lower="$(printf '%s' "$content" | tr '[:upper:]' '[:lower:]')"
    case "$lower" in
        *"$SLUG_BARE") return 0 ;;
        *"$SLUG_BARE"[!0-9]*) return 0 ;;
    esac
    return 1
}

LOCAL_BRANCH_EXISTS=0
LOCAL_BRANCH_CHECKED_OUT_AT=""
if git show-ref --verify --quiet "refs/heads/${BRANCH}"; then
    LOCAL_BRANCH_EXISTS=1
    wt_path=""
    wt_branch=""
    flush_wt_entry() {
        if [ "$wt_branch" = "$BRANCH" ]; then
            LOCAL_BRANCH_CHECKED_OUT_AT="$wt_path"
        fi
        wt_path=""
        wt_branch=""
    }
    while IFS= read -r line; do
        case "$line" in
            "worktree "*) wt_path="${line#worktree }" ;;
            "branch refs/heads/"*) wt_branch="${line#branch refs/heads/}" ;;
            "") flush_wt_entry ;;
            *) ;;
        esac
    done < <(git worktree list --porcelain)
    flush_wt_entry
fi

if [ -n "$LOCAL_BRANCH_CHECKED_OUT_AT" ]; then
    if ! worktree_tombstone_names_ticket "$LOCAL_BRANCH_CHECKED_OUT_AT"; then
        echo "ERROR: local branch '$BRANCH' is checked out at $LOCAL_BRANCH_CHECKED_OUT_AT," >&2
        echo "       which has no valid STRANDED.md for ${TICKET_KEY}. Run this first:" >&2
        echo "" >&2
        echo "       $SCRIPT_DIR/mark-worktree-stranded.sh $TICKET_KEY --worktree $LOCAL_BRANCH_CHECKED_OUT_AT" >&2
        echo "" >&2
        echo "       then re-run this script. Without that, the worktree tombstone that" >&2
        echo "       suppresses check 4 for this branch (TOMBSTONED_BRANCHES) would be" >&2
        echo "       missing, and releasing the remote branch alone would not have" >&2
        echo "       unstuck the ticket — the exact KYO-529 trap." >&2
        exit 1
    fi
fi

# ---- resolve the remote sha, with a retry-friendly "already done" check ----
resolve_stderr_file="$(mktemp)"
ORIG_REF_FOUND=0
ORIG_SHA=""
if orig_ref_line="$(git ls-remote --exit-code "$REMOTE" "refs/heads/${BRANCH}" 2>"$resolve_stderr_file")"; then
    ORIG_REF_FOUND=1
    ORIG_SHA="$(printf '%s' "$orig_ref_line" | cut -f1)"
else
    ls_remote_status=$?
    # `git ls-remote --exit-code` exits 2 specifically for "no matching
    # refs" — distinct from a genuine connectivity/auth failure.
    if [ "$ls_remote_status" -ne 2 ]; then
        echo "ERROR: could not query $REMOTE for refs/heads/${BRANCH}: $(cat "$resolve_stderr_file")" >&2
        rm -f "$resolve_stderr_file"
        exit 1
    fi
fi
rm -f "$resolve_stderr_file"

if [ "$ORIG_REF_FOUND" -eq 0 ]; then
    # The original ref is gone. Either the branch never existed, or a prior
    # run of this script already completed (or died between push and
    # delete). Distinguish by checking whether stranded/<branch> exists.
    check_stranded_stderr="$(mktemp)"
    if stranded_ref_line="$(git ls-remote --exit-code "$REMOTE" "refs/heads/${STRANDED_BRANCH}" 2>"$check_stranded_stderr")"; then
        rm -f "$check_stranded_stderr"
        echo "ALREADY TOMBSTONED: refs/heads/${BRANCH} is gone and refs/heads/${STRANDED_BRANCH} already exists on $REMOTE — a prior run already completed this."
        echo "  - Ticket:  ${TICKET_KEY}"
        echo "  - Remote:  ${REMOTE}"
        echo "  - Ref:     refs/heads/${STRANDED_BRANCH}"
        printf '%s' "$stranded_ref_line" | cut -f1 | sed 's/^/  - Sha:     /'
        exit 0
    fi
    rm -f "$check_stranded_stderr"
    echo "ERROR: refs/heads/${BRANCH} does not exist on $REMOTE (and neither does refs/heads/${STRANDED_BRANCH})." >&2
    exit 1
fi

# ---- fetch the object locally, and re-verify the sha did not move ----------
fetch_stderr_file="$(mktemp)"
if ! git fetch -q "$REMOTE" "refs/heads/${BRANCH}" 2>"$fetch_stderr_file"; then
    echo "ERROR: failed to fetch $REMOTE refs/heads/${BRANCH}: $(cat "$fetch_stderr_file")" >&2
    rm -f "$fetch_stderr_file"
    exit 1
fi
rm -f "$fetch_stderr_file"

FETCHED_SHA="$(git rev-parse FETCH_HEAD)"
if [ "$FETCHED_SHA" != "$ORIG_SHA" ]; then
    echo "ERROR: refs/heads/${BRANCH} moved between resolution ($ORIG_SHA) and fetch ($FETCHED_SHA)." >&2
    echo "       Refusing to tombstone a moving target — re-run this script." >&2
    exit 1
fi

# ---- push the copy — VERIFY BEFORE DESTROYING (fail closed) ----------------
push_stderr_file="$(mktemp)"
if ! git push "$REMOTE" "${FETCHED_SHA}:refs/heads/${STRANDED_BRANCH}" 2>"$push_stderr_file"; then
    echo "ERROR: failed to push refs/heads/${STRANDED_BRANCH} to $REMOTE: $(cat "$push_stderr_file")" >&2
    echo "       Original refs/heads/${BRANCH} is untouched." >&2
    rm -f "$push_stderr_file"
    exit 1
fi
rm -f "$push_stderr_file"

verify_stderr_file="$(mktemp)"
if ! verify_ref_line="$(git ls-remote --exit-code "$REMOTE" "refs/heads/${STRANDED_BRANCH}" 2>"$verify_stderr_file")"; then
    echo "ERROR: pushed refs/heads/${STRANDED_BRANCH} but could not verify it afterward: $(cat "$verify_stderr_file")" >&2
    echo "       Refusing to delete refs/heads/${BRANCH} without confirmation. Original is untouched." >&2
    rm -f "$verify_stderr_file"
    exit 1
fi
rm -f "$verify_stderr_file"

VERIFIED_SHA="$(printf '%s' "$verify_ref_line" | cut -f1)"
if [ "$VERIFIED_SHA" != "$FETCHED_SHA" ]; then
    echo "ERROR: refs/heads/${STRANDED_BRANCH} exists but points at $VERIFIED_SHA, expected $FETCHED_SHA." >&2
    echo "       Refusing to delete refs/heads/${BRANCH} without confirmation. Original is untouched." >&2
    exit 1
fi

# ---- only now delete the original remote ref --------------------------------
delete_stderr_file="$(mktemp)"
if ! git push "$REMOTE" ":refs/heads/${BRANCH}" 2>"$delete_stderr_file"; then
    echo "ERROR: verified refs/heads/${STRANDED_BRANCH} but failed to delete the original refs/heads/${BRANCH}: $(cat "$delete_stderr_file")" >&2
    echo "       Both refs now exist, pointing at the same commit — safe to leave as-is, or delete" >&2
    echo "       refs/heads/${BRANCH} by hand, or simply re-run this script." >&2
    rm -f "$delete_stderr_file"
    exit 1
fi
rm -f "$delete_stderr_file"

# ---- rename the local branch too, if it exists and isn't checked out ------
LOCAL_RENAMED=0
if [ "$LOCAL_BRANCH_EXISTS" -eq 1 ] && [ -z "$LOCAL_BRANCH_CHECKED_OUT_AT" ]; then
    if git branch -m "$BRANCH" "$STRANDED_BRANCH" 2>/dev/null; then
        LOCAL_RENAMED=1
    else
        echo "WARNING: renamed the remote branch but failed to rename the local branch '$BRANCH' to '$STRANDED_BRANCH'." >&2
        echo "         Rename it by hand: git branch -m '$BRANCH' '$STRANDED_BRANCH'" >&2
        # DO NOT treat this as fatal — the remote is already correct and is
        # what every OTHER worker reads. This is local-only residue on the
        # releasing machine: a same-machine run of check-ticket-in-flight.sh
        # would count the stale local '$BRANCH' as a `local branch:` HIT
        # (check 4), unless it happens to be the current branch. Remedy:
        # rename/delete the stale local branch by hand (command above), or
        # pass --ignore-branch '$BRANCH' to check-ticket-in-flight.sh.
    fi
fi

# ---- summary, pasteable into a Trakkt release comment -----------------------
echo "TOMBSTONED: ${BRANCH} -> ${STRANDED_BRANCH}"
echo "  - Ticket:  ${TICKET_KEY}"
echo "  - Remote:  ${REMOTE}"
echo "  - Sha:     ${FETCHED_SHA}"
if [ "$LOCAL_RENAMED" -eq 1 ]; then
    echo "  - Local:   renamed local branch '${BRANCH}' -> '${STRANDED_BRANCH}'"
elif [ -n "$LOCAL_BRANCH_CHECKED_OUT_AT" ]; then
    echo "  - Local:   left in place, checked out at ${LOCAL_BRANCH_CHECKED_OUT_AT} (already tombstoned via STRANDED.md)"
elif [ "$LOCAL_BRANCH_EXISTS" -eq 0 ]; then
    echo "  - Local:   no local branch of that name in this repo"
else
    # The only remaining combination: a local branch exists, isn't checked
    # out anywhere, and `git branch -m` above failed. Every other combination
    # of LOCAL_RENAMED/LOCAL_BRANCH_CHECKED_OUT_AT/LOCAL_BRANCH_EXISTS is
    # unreachable by construction (LOCAL_RENAMED is only ever set to 1 inside
    # the guarded rename block, and LOCAL_BRANCH_CHECKED_OUT_AT is only ever
    # set when LOCAL_BRANCH_EXISTS=1), so this is a safe catch-all rather
    # than a fourth condition that could itself be incomplete.
    echo "  - Local:   NOT renamed — 'git branch -m' failed for local branch '${BRANCH}'."
    echo "             Remote is correct; other workers are unaffected. This machine has a"
    echo "             stale local branch '${BRANCH}' that a later same-machine run of"
    echo "             check-ticket-in-flight.sh would count as a false local-branch HIT."
    echo "             Remedy: rename or delete it by hand"
    echo "             (git branch -m '${BRANCH}' '${STRANDED_BRANCH}' or git branch -D '${BRANCH}'),"
    echo "             or pass --ignore-branch '${BRANCH}' to check-ticket-in-flight.sh."
fi
if [ -n "$NOTE" ]; then
    echo "  - Note:    ${NOTE}"
fi

exit 0
