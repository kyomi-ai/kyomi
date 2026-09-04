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
# REMOTE AND LOCAL ARE TOMBSTONED INDEPENDENTLY (KYO-596) — READ THIS BEFORE
# "FIXING" IT BACK TO A SINGLE SHORT-CIRCUIT
#
# Earlier versions of this script treated "remote already tombstoned" as
# proof the WHOLE job was already done and exited 0 without looking at the
# local branch at all. That is wrong whenever one run tombstones the remote
# and then dies before renaming the local branch — routine here, since these
# releases exist precisely because runs die. Observed live during KYO-463's
# release (2026-09-02): `mark-branch-stranded.sh` printed ALREADY TOMBSTONED
# and exited 0 while `refs/heads/jason/kyo-463-spec-green4` was still a live
# local branch, so `check-ticket-in-flight.sh` kept reporting the ticket IN
# FLIGHT — an exit-0 that meant "unfinished," the exact shape
# docs/standards/error-handling/empty-on-failure-must-not-look-like-a-real-result.md
# warns about. The fix: the remote side and the local side of the NAMED
# `--branch` are each evaluated and, if needed, fixed independently. "ALREADY
# TOMBSTONED" with no further action is reported ONLY when BOTH sides already
# agree; when only one side needed work, the summary says plainly which side
# was already done and which one this run just fixed.
#
# THE FIX ABOVE HAD ITS OWN NARROWER VERSION OF THE SAME BUG (KYO-596 REWORK)
#
# The fix's own final summary picked its headline off LOCAL_RENAME_ATTEMPTED
# — set the instant a rename is DECIDED, before `git branch -m` runs — rather
# than off LOCAL_RENAMED, which records whether it actually succeeded. So a
# run where the remote was already tombstoned AND the local rename then
# FAILED still printed "TOMBSTONED (local branch only)" and exited 0: the
# identical "exit 0 that meant unfinished" shape, one variable over. See the
# comment above the final summary block for the full fix and the reasoning
# for which failures still exit 0 and which do not.
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
#   4. Independently of step 3's outcome — including when the remote side
#      turns out to already be tombstoned by a prior run — if a local branch
#      of that name exists and is not checked out anywhere, renames it to
#      `stranded/<branch>` too, so a local `git branch --list` matches the
#      same durability rule as the remote (KYO-596). This rename is
#      best-effort (a WARNING, not a fatal error) — see that block below for
#      the residual local-only inconsistency if it fails.
#   5. Sweeps every OTHER local branch matching this ticket (e.g. a second
#      attempt's branch left behind when a later run adopted an earlier
#      attempt's worktree for its warm `target/`) and renames each one not
#      currently checked out anywhere to `stranded/<name>` too (KYO-596).
#      This is default behaviour, not opt-in — see the block below for why.
#
# WHY THE SWEEP (STEP 5) IS DEFAULT, NOT AN OPT-IN FLAG (KYO-596)
#
# Adopting an earlier attempt's worktree to reuse its warm `target/` is a
# supported, deliberate workflow, so a ticket routinely ends its life with
# more than one local branch — not just the one named on the command line.
# Every one of those branches is exactly as much "this ticket is dead, stop
# reading it as a live claim" evidence as the named branch is, and the
# caller of this script (a release path) has already declared the whole
# ticket dead by the time it runs. The action taken is a `git branch -m`
# rename, never a delete: it is fully reversible by hand
# (`git branch -m 'stranded/<name>' '<name>'`) and never touches the remote
# or any branch `git worktree list` shows as checked out, so it cannot lose
# work or disrupt anything in progress. Given that, requiring a separate
# flag on every release-time call would only recreate the exact bug this
# ticket fixes for every OTHER branch besides the named one — silence dressed
# up as safety. No opt-out flag is provided for the same reason: there is no
# state this touches that the checked-out guard does not already protect.
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
#     between push and delete and this is a retry): the remote side reports
#     as already done. This is no longer an unconditional "nothing to do,
#     exit 0" (KYO-596, see above) — the local side of `<branch>` and the
#     ticket's other local branches are still checked and fixed if needed;
#     only when nothing needed fixing does the summary say "ALREADY
#     TOMBSTONED" outright, rather than a confusing "no such branch" error on
#     a retry of a script that is meant to be safely re-run.
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
#               MARKER FORMAT, not ticket-string parsing). The ticket-number
#               matching used by the sweep (step 5) is the same deliberate
#               duplication, this time of check-ticket-in-flight.sh's
#               `matches_ticket`.
#   --branch    required. The remote branch to tombstone.
#   --remote    default: origin.
#   --note      optional free text, folded into the printed summary.
#
# Must be run with the current working directory inside the repo whose
# remote and local branches are being tombstoned — same assumption every
# other script in this file makes (no --repo/--worktree flag; the LOCAL
# branch checks operate on `git worktree list` / `git branch` for whatever
# repo the cwd resolves to).
#
# EXIT CODES
#
#   0 — tombstoned (or already tombstoned — idempotent). Also covers a
#       failed local rename of the NAMED branch, or of a swept OTHER
#       branch, PROVIDED the remote side of the named branch was tombstoned
#       during THIS run — see the KYO-596 rework comment above the final
#       summary block for why that residue does not fail the run.
#   1 — error: branch is main, branch has a PR, a local worktree checkout
#       lacks a valid tombstone, the branch doesn't exist on the remote,
#       any git/gh step failed, a post-push verification mismatch, THE
#       NAMED BRANCH'S LOCAL RENAME FAILED WHILE THE REMOTE WAS ALREADY
#       TOMBSTONED BY A PRIOR RUN (this run's only job then accomplished
#       nothing), or the other-branches SWEEP (step 5) failed to rename at
#       least one matching branch (a swept branch has no remote counterpart
#       to fall back on — see the same comment).
#   2 — usage error (missing/unparseable ticket, missing --branch, unknown
#       flag, missing value for a flag).
#  42 — this script's own on-disk content is stale relative to origin/main
#       AND KYOMI_STALE_TOOLING_STRICT=1 is set. See
#       scripts/lib/stale-tooling-guard.sh (KYO-632) — by default this is a
#       loud warning on stderr, not a failure.
# ------------------------------------------------------------------------------

set -euo pipefail

SCRIPT_NAME="$(basename "$0")"
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=lib/stale-tooling-guard.sh
source "${SCRIPT_DIR}/lib/stale-tooling-guard.sh"
stale_tooling_guard "${BASH_SOURCE[0]}"

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
     mismatch, the named branch's local rename failed while the remote was
     already tombstoned by a prior run, or the other-branches sweep failed
     to rename a matching branch)
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

# ---- shared worktree map, built once (KYO-596) -----------------------------
# A single `git worktree list --porcelain` parse, used by BOTH the named
# branch's pre-flight checkout refusal and the other-branches sweep below, so
# the two agree on exactly what "checked out somewhere" means and this repo
# is walked only once.
declare -a WT_PATHS=()
declare -a WT_BRANCHES=()
wt_path=""
wt_branch=""
flush_wt_entry() {
    if [ -n "$wt_path" ]; then
        WT_PATHS+=("$wt_path")
        WT_BRANCHES+=("$wt_branch")
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
flush_wt_entry # in case the porcelain output has no trailing blank line

checked_out_path_for_branch() {
    # checked_out_path_for_branch <branch> — prints the worktree path and
    # returns 0 if <branch> is checked out there; returns 1 (printing
    # nothing) if it is not checked out in any worktree, INCLUDING the one
    # this script is running from (that worktree is in WT_PATHS/WT_BRANCHES
    # too, so "the branch I'm currently on" is never treated as free).
    local target="$1" i
    for i in "${!WT_BRANCHES[@]}"; do
        if [ "${WT_BRANCHES[$i]}" = "$target" ]; then
            printf '%s' "${WT_PATHS[$i]}"
            return 0
        fi
    done
    return 1
}

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

branch_matches_ticket_for_sweep() {
    # branch_matches_ticket_for_sweep <name> — same matching rule as
    # check-ticket-in-flight.sh's `matches_ticket`: case-insensitive
    # `kyo-<N>-` substring, or an exact `kyo-<N>` suffix with nothing after
    # it. Deliberately duplicated (see the header's USAGE section) rather
    # than sourcing that script, which is not designed to be sourced.
    local name="$1" lower
    lower="$(printf '%s' "$name" | tr '[:upper:]' '[:lower:]')"
    case "$lower" in
        *"${SLUG_BARE}-"*) return 0 ;;
        *"$SLUG_BARE") return 0 ;;
    esac
    return 1
}

# ---- idempotency: --branch already under stranded/ --------------------------
# Handled as a variant of the "named branch already tombstoned" state rather
# than an unconditional early exit, so the other-branches sweep (step 5)
# still runs for this ticket even when the caller passed an already-stranded
# name (KYO-596).
NAMED_BRANCH_ALREADY_STRANDED_FORM=0
case "$BRANCH" in
    stranded/*)
        NAMED_BRANCH_ALREADY_STRANDED_FORM=1
        ;;
esac

if [ "$NAMED_BRANCH_ALREADY_STRANDED_FORM" -eq 1 ]; then
    STRANDED_BRANCH="$BRANCH"
    NAMED_REMOTE_ALREADY_DONE=1
    NAMED_REMOTE_TOMBSTONED_THIS_RUN=0
    FETCHED_SHA=""
else
    STRANDED_BRANCH="stranded/${BRANCH}"

    # ---- refuse if the branch has a PR (any state) ---------------------------
    # Server-side EXACT head-branch filter, not a substring/body/title search —
    # see check-ticket-in-flight.sh's KYO-471 header note on why that's the
    # only safe way to tie a branch to a PR. Extraction follows check-ticket-in-flight.sh's
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
    LOCAL_BRANCH_EXISTS=0
    LOCAL_BRANCH_CHECKED_OUT_AT=""
    if git show-ref --verify --quiet "refs/heads/${BRANCH}"; then
        LOCAL_BRANCH_EXISTS=1
        if checked_out_at="$(checked_out_path_for_branch "$BRANCH")"; then
            LOCAL_BRANCH_CHECKED_OUT_AT="$checked_out_at"
        fi
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

    NAMED_REMOTE_ALREADY_DONE=0
    NAMED_REMOTE_TOMBSTONED_THIS_RUN=0
    FETCHED_SHA=""

    if [ "$ORIG_REF_FOUND" -eq 0 ]; then
        # The original ref is gone. Either the branch never existed, or a prior
        # run of this script already completed (or died between push and
        # delete). Distinguish by checking whether stranded/<branch> exists.
        check_stranded_stderr="$(mktemp)"
        if stranded_ref_line="$(git ls-remote --exit-code "$REMOTE" "refs/heads/${STRANDED_BRANCH}" 2>"$check_stranded_stderr")"; then
            rm -f "$check_stranded_stderr"
            NAMED_REMOTE_ALREADY_DONE=1
            FETCHED_SHA="$(printf '%s' "$stranded_ref_line" | cut -f1)"
        else
            rm -f "$check_stranded_stderr"
            echo "ERROR: refs/heads/${BRANCH} does not exist on $REMOTE (and neither does refs/heads/${STRANDED_BRANCH})." >&2
            exit 1
        fi
    else
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

        NAMED_REMOTE_TOMBSTONED_THIS_RUN=1
    fi
fi

# ---- local handling for the NAMED branch, independent of the remote path
# taken above (KYO-596) ------------------------------------------------------
# LOCAL_BRANCH_EXISTS/LOCAL_BRANCH_CHECKED_OUT_AT were computed above for the
# non-stranded-form path; for the stranded-form path (BRANCH == STRANDED_BRANCH)
# there is nothing to rename, so default them so the logic below is a no-op.
if [ "$NAMED_BRANCH_ALREADY_STRANDED_FORM" -eq 1 ]; then
    LOCAL_BRANCH_EXISTS=0
    LOCAL_BRANCH_CHECKED_OUT_AT=""
fi

LOCAL_RENAMED=0
LOCAL_RENAME_ATTEMPTED=0
if [ "$BRANCH" != "$STRANDED_BRANCH" ] && [ "$LOCAL_BRANCH_EXISTS" -eq 1 ] && [ -z "$LOCAL_BRANCH_CHECKED_OUT_AT" ]; then
    LOCAL_RENAME_ATTEMPTED=1
    if git branch -m "$BRANCH" "$STRANDED_BRANCH" 2>/dev/null; then
        LOCAL_RENAMED=1
    else
        echo "WARNING: failed to rename the local branch '$BRANCH' to '$STRANDED_BRANCH'." >&2
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

# ---- sweep every OTHER local branch for this ticket (KYO-596, step 5) ------
# See the header's "WHY THE SWEEP IS DEFAULT" section for why this always
# runs rather than requiring a flag. Only reached once the named branch's
# handling above has completed without a fatal error, so a refusal earlier
# in the script (PR exists, unresolved worktree checkout, remote branch
# missing, a failed git/gh step) never touches any OTHER branch either.
declare -a SWEPT_RENAMED=()
declare -a SWEPT_LEFT_CHECKED_OUT=()
declare -a SWEPT_RENAME_FAILED=()

while IFS= read -r other_branch; do
    [ -n "$other_branch" ] || continue
    [ "$other_branch" = "$BRANCH" ] && continue
    [ "$other_branch" = "$STRANDED_BRANCH" ] && continue
    case "$other_branch" in
        stranded/*) continue ;;
    esac
    branch_matches_ticket_for_sweep "$other_branch" || continue
    if other_wt_path="$(checked_out_path_for_branch "$other_branch")"; then
        SWEPT_LEFT_CHECKED_OUT+=("${other_branch} (checked out at ${other_wt_path})")
        continue
    fi
    if git branch -m "$other_branch" "stranded/${other_branch}" 2>/dev/null; then
        SWEPT_RENAMED+=("${other_branch} -> stranded/${other_branch}")
    else
        echo "WARNING: failed to rename other local branch '$other_branch' to 'stranded/${other_branch}'." >&2
        echo "         Rename it by hand: git branch -m '$other_branch' 'stranded/${other_branch}'" >&2
        SWEPT_RENAME_FAILED+=("$other_branch")
    fi
done < <(git branch --list --format='%(refname:short)')

print_sweep_summary() {
    if [ "${#SWEPT_RENAMED[@]}" -gt 0 ]; then
        echo "  - Other branches tombstoned (${TICKET_KEY}):"
        printf '       %s\n' "${SWEPT_RENAMED[@]}"
    fi
    if [ "${#SWEPT_LEFT_CHECKED_OUT[@]}" -gt 0 ]; then
        echo "  - Other branches left alone, checked out elsewhere (${TICKET_KEY}):"
        printf '       %s\n' "${SWEPT_LEFT_CHECKED_OUT[@]}"
    fi
    if [ "${#SWEPT_RENAME_FAILED[@]}" -gt 0 ]; then
        echo "  - WARNING: failed to rename other local branch(es) for ${TICKET_KEY}: ${SWEPT_RENAME_FAILED[*]}"
        echo "             rename them by hand: git branch -m '<name>' 'stranded/<name>'"
    fi
}

# ---- local-state summary line for the named branch, shared by every exit
# path below --------------------------------------------------------------
print_local_summary() {
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
}

# ---- final summary, pasteable into a Trakkt release comment ---------------
#
# THE HEADLINE MUST KEY OFF LOCAL_RENAMED, NEVER LOCAL_RENAME_ATTEMPTED
# (KYO-596 REWORK) — READ THIS BEFORE "SIMPLIFYING" IT BACK
#
# LOCAL_RENAME_ATTEMPTED is set to 1 the instant the script DECIDES to try
# `git branch -m`, before the command runs, so it stays 1 whether the rename
# succeeds or fails. A code reviewer caught this block picking its "local
# branch only" SUCCESS headline off ATTEMPTED instead of off LOCAL_RENAMED
# (which records the real outcome): a run where the rename actually FAILED
# still printed "TOMBSTONED (local branch only)" and exited 0 — the exact
# "exit 0 that means unfinished" bug KYO-596 exists to remove, reintroduced
# one variable over. Reproduced live: pre-creating a conflicting local
# `stranded/<branch>` ref forces `git branch -m` to fail, and the old
# headline claimed success anyway while the local branch sat unrenamed and
# check-ticket-in-flight.sh kept reporting the ticket IN FLIGHT forever.
#
# WHY THE PLAIN "TOMBSTONED (remote)" PATH BELOW STILL EXITS 0 ON A FAILED
# LOCAL RENAME, BUT THE "REMOTE ALREADY DONE" PATH DOES NOT
#
# On the plain path, the remote rename happened THIS run. The remote is
# what every OTHER machine reads (via `stranded/` on $REMOTE), so the
# tombstone materially succeeded regardless of what happens to the local
# branch afterward — a failed local rename there is local-only residue on
# THIS machine, already surfaced in detail by print_local_summary with a
# documented by-hand remedy. Exit 0 is defensible, but the headline is
# qualified "(remote)" so it never reads as a claim about the local side
# too — that claim belongs to the "Local:" line beneath it.
#
# On the "remote already done" path, a PRIOR run already tombstoned the
# remote, so THIS run's entire deliverable for the named branch was the
# local rename. If that fails, this run accomplished nothing at all, and
# the headline must say so plainly, with a non-zero exit — anything else
# tells the caller (a release script) that the release succeeded when the
# ticket is exactly as stuck as it was before this run started.
#
# THE SWEEP (STEP 5) GETS THE SAME "NO REMOTE TO FALL BACK ON" TREATMENT
#
# A swept branch (SWEPT_RENAME_FAILED) never has a remote counterpart at
# all — every one is a local-only leftover from an adopted worktree, never
# pushed anywhere. So the "the remote materially succeeded elsewhere"
# carve-out that excuses the named branch's local residue on the plain path
# above never applies to a swept branch: no other machine can ever see it
# tombstoned, and nothing else will ever clean it up. check-ticket-in-flight.sh's
# check 4 matches a stray swept branch exactly like it matches the named
# branch, so excluding sweep failures from the exit code would ship the
# identical bug one function over, for a branch nobody was even watching by
# name. A sweep failure therefore always forces a non-zero exit, regardless
# of which headline above it fires.
EXIT_CODE=0
if [ "$NAMED_BRANCH_ALREADY_STRANDED_FORM" -eq 1 ]; then
    echo "ALREADY TOMBSTONED: '$BRANCH' is already under the stranded/ namespace — nothing to do."
    echo "  - Ticket:  ${TICKET_KEY}"
    print_sweep_summary
elif [ "$NAMED_REMOTE_ALREADY_DONE" -eq 1 ] && [ "$LOCAL_RENAME_ATTEMPTED" -eq 0 ]; then
    # Both sides already agreed before this run touched anything: the remote
    # was tombstoned by a prior run, and the local branch either doesn't
    # exist here or is legitimately checked out elsewhere with its own
    # STRANDED.md. Nothing needed fixing (KYO-596).
    echo "ALREADY TOMBSTONED: refs/heads/${BRANCH} is gone and refs/heads/${STRANDED_BRANCH} already exists on $REMOTE — a prior run already completed this."
    echo "  - Ticket:  ${TICKET_KEY}"
    echo "  - Remote:  ${REMOTE}"
    echo "  - Ref:     refs/heads/${STRANDED_BRANCH}"
    echo "  - Sha:     ${FETCHED_SHA}"
    print_local_summary
    print_sweep_summary
elif [ "$NAMED_REMOTE_ALREADY_DONE" -eq 1 ] && [ "$LOCAL_RENAMED" -eq 1 ]; then
    # THE KYO-596 CASE, successful: the remote side was already tombstoned
    # by a prior run, but the local branch had NOT been renamed — this run
    # just fixed that. Reported distinctly from plain "ALREADY TOMBSTONED"
    # so a reader (or a Trakkt release comment) can see that real work
    # happened here. Gated on LOCAL_RENAMED, not LOCAL_RENAME_ATTEMPTED —
    # see the block comment above.
    echo "TOMBSTONED (local branch only): refs/heads/${BRANCH} was already gone and refs/heads/${STRANDED_BRANCH} already existed on $REMOTE (a prior run completed the remote side); the local branch had not been tombstoned yet."
    echo "  - Ticket:  ${TICKET_KEY}"
    echo "  - Remote:  ${REMOTE} (already done)"
    echo "  - Ref:     refs/heads/${STRANDED_BRANCH}"
    echo "  - Sha:     ${FETCHED_SHA}"
    print_local_summary
    print_sweep_summary
elif [ "$NAMED_REMOTE_ALREADY_DONE" -eq 1 ]; then
    # The remaining combination: LOCAL_RENAME_ATTEMPTED=1 and
    # LOCAL_RENAMED=0 — the rename was tried and it FAILED, and the remote
    # was already done by a prior run, so this run's only job accomplished
    # nothing. See the block comment above for why this is the one case
    # that exits non-zero rather than treating the failure as residue.
    echo "NOT TOMBSTONED: local branch rename FAILED for '${BRANCH}' (refs/heads/${STRANDED_BRANCH} already existed on $REMOTE from a prior run — the local rename was this run's only job, and it did not happen)."
    echo "  - Ticket:  ${TICKET_KEY}"
    echo "  - Remote:  ${REMOTE} (already done)"
    echo "  - Ref:     refs/heads/${STRANDED_BRANCH}"
    echo "  - Sha:     ${FETCHED_SHA}"
    print_local_summary
    print_sweep_summary
    EXIT_CODE=1
else
    # The remote rename happened during THIS run — see the block comment
    # above for why the headline is qualified "(remote)" rather than an
    # unqualified "TOMBSTONED", even though the common case also renames
    # the local branch successfully: the local outcome is always the
    # "Local:" line below, never implied by this headline alone.
    echo "TOMBSTONED (remote): ${BRANCH} -> ${STRANDED_BRANCH}"
    echo "  - Ticket:  ${TICKET_KEY}"
    echo "  - Remote:  ${REMOTE}"
    echo "  - Sha:     ${FETCHED_SHA}"
    print_local_summary
    print_sweep_summary
fi
if [ -n "$NOTE" ]; then
    echo "  - Note:    ${NOTE}"
fi

if [ "${#SWEPT_RENAME_FAILED[@]}" -gt 0 ]; then
    # See the block comment above the branching above — a swept branch has
    # no remote counterpart to fall back on, so its failure always fails
    # the run, even when the headline above otherwise reports success.
    EXIT_CODE=1
fi

exit "$EXIT_CODE"
