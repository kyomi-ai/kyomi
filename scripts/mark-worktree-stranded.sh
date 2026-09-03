#!/usr/bin/env bash
# ------------------------------------------------------------------------------
# scripts/mark-worktree-stranded.sh — write the STRANDED.md tombstone marker
# a preserved dead worktree needs so check-ticket-in-flight.sh stops reading
# it as a live claim. (KYO-529)
#
# WHY THIS EXISTS
#
# /backlog-fast Step 0.5 deliberately preserves the worktree of a run that
# died mid-ticket, so its unpushed work can be salvaged by hand. But
# check-ticket-in-flight.sh's checks 3/4 (KYO-471) cannot tell a preserved
# dead worktree apart from a live worker's worktree — both are just "a
# worktree whose branch matches the ticket". From the moment a ticket is
# released as stranded, every future worker was told "in flight" forever,
# and the ticket went back to Backlog looking available while never being
# picked up again. See check-ticket-in-flight.sh's "STRANDED-WORKTREE
# TOMBSTONES" header section for the full rationale — this script is the
# writer half of that fix, and the ONLY place the marker's format lives
# (KYO-422 principle: callers invoke one script, they do not restate the
# rule in prose across three skill files).
#
# WHAT IT WRITES
#
# `<worktree>/STRANDED.md`, containing at minimum: the ticket key, an
# ISO-8601 UTC release timestamp, the worktree's path, its branch, and an
# explanatory body stating the tree is preserved for salvage, is not a
# claim, and that anyone adopting it to resume work must delete this file
# first (check-ticket-in-flight.sh has no way to detect a human has since
# started using the tree again — leaving the marker in place would keep
# suppressing the claim signal against live work).
#
# check-ticket-in-flight.sh only honours a marker that NAMES the ticket
# (`kyo-<N>`, case-insensitive) — a marker that doesn't is not a tombstone
# for any ticket. This script always writes the ticket it was given, so
# that requirement costs this script nothing.
#
# REFUSALS (fail closed — these are not warnings, they are hard errors)
#
#   - The primary worktree. Compared via `git rev-parse --path-format=absolute
#     --git-dir` against `--git-common-dir`: in the primary worktree these
#     are the same directory, in any linked worktree they differ. Writing
#     STRANDED.md into the canonical clone's own root would make the
#     canonical clone itself read as tombstoned.
#   - Branch `main`. No ticket branch is ever `main` (same assumption
#     check-ticket-in-flight.sh makes for self-exclusion), and a "stranded
#     main" is not a concept this workflow has any use for.
#   - Detached HEAD. There is no branch to tombstone.
#
# `--path-format=absolute` requires git >= 2.31 (see
# scripts/lib/canonical-root.sh's header for why this is load-bearing, not
# decoration: the unqualified form of --git-common-dir is relative when run
# from the primary worktree itself). This script fails closed — exits 1 —
# rather than guessing, if the installed git predates it.
#
# USAGE
#
#   mark-worktree-stranded.sh <TICKET> [--worktree <path>] [--note <text>]
#
#   TICKET      KYO-529, kyo-529, or 529 (all equivalent)
#   --worktree  the worktree to tombstone (default: the current worktree's
#               root, via `git rev-parse --show-toplevel`)
#   --note      optional free-text line appended to the marker body (e.g.
#               why the run died, what's left to salvage)
#
# Overwriting an existing STRANDED.md is allowed and idempotent — releasing
# the same stranded claim twice, or re-tombstoning after refreshing the
# timestamp, must not require deleting the old file first. It says so on
# stderr rather than doing it silently.
#
# EXIT CODES
#
#   0 — marker written.
#   1 — error: not a git worktree, primary worktree, branch is main or
#       detached HEAD, git too old for --path-format=absolute, or the file
#       write failed.
#   2 — usage error (missing/unparseable ticket argument, unknown flag,
#       missing value for --worktree/--note).
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
Usage: $SCRIPT_NAME <TICKET> [--worktree <path>] [--note <text>]

  TICKET               KYO-529, kyo-529, or 529 (all equivalent)
  --worktree <path>    worktree to tombstone (default: current worktree root)
  --note <text>        optional free-text line appended to the marker body

Exit codes:
  0  marker written
  1  error (primary worktree, branch main/detached, git too old, write failed)
  2  usage error
EOF
}

if [ "$#" -eq 0 ]; then
    usage
    exit 2
fi

TICKET_ARG="$1"
shift

TARGET_WORKTREE=""
NOTE=""

while [ "$#" -gt 0 ]; do
    case "$1" in
        --worktree)
            if [ "$#" -lt 2 ]; then
                echo "ERROR: --worktree requires a value" >&2
                exit 2
            fi
            TARGET_WORKTREE="$2"
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

# ---- normalize the ticket argument to a bare digit string ------------------
# Same normalization as check-ticket-in-flight.sh — deliberately duplicated
# rather than factored into a shared lib, since it is ~10 stable lines and
# the thing this ticket consolidates is the MARKER FORMAT, not ticket
# parsing (see the header above).
ticket_lower="$(printf '%s' "$TICKET_ARG" | tr '[:upper:]' '[:lower:]')"
case "$ticket_lower" in
    kyo-*) ticket_num="${ticket_lower#kyo-}" ;;
    *) ticket_num="$ticket_lower" ;;
esac

case "$ticket_num" in
    '' | *[!0-9]*)
        echo "ERROR: could not parse a ticket number out of '$TICKET_ARG' (expected KYO-529, kyo-529, or 529)" >&2
        usage
        exit 2
        ;;
esac

TICKET_KEY="KYO-${ticket_num}"

# ---- resolve the target worktree -------------------------------------------
if [ -z "$TARGET_WORKTREE" ]; then
    if ! toplevel="$(git rev-parse --show-toplevel 2>&1)"; then
        echo "ERROR: not inside a git worktree, and no --worktree given: $toplevel" >&2
        exit 1
    fi
    TARGET_WORKTREE="$toplevel"
else
    if [ ! -d "$TARGET_WORKTREE" ]; then
        echo "ERROR: --worktree path does not exist: $TARGET_WORKTREE" >&2
        exit 1
    fi
    resolved="$(git -C "$TARGET_WORKTREE" rev-parse --show-toplevel 2>&1)" || {
        echo "ERROR: --worktree path is not a git worktree: $TARGET_WORKTREE ($resolved)" >&2
        exit 1
    }
    TARGET_WORKTREE="$resolved"
fi

# ---- refuse the primary worktree -------------------------------------------
# --path-format=absolute requires git >= 2.31; fail closed rather than guess
# if it's unsupported, exactly like scripts/lib/canonical-root.sh does for
# the same flag.
if ! git_dir="$(git -C "$TARGET_WORKTREE" rev-parse --path-format=absolute --git-dir 2>&1)"; then
    echo "ERROR: 'git rev-parse --path-format=absolute' is unsupported by this git version" >&2
    echo "       (requires git >= 2.31). Refusing to guess whether this is the primary worktree." >&2
    exit 1
fi
if ! common_dir="$(git -C "$TARGET_WORKTREE" rev-parse --path-format=absolute --git-common-dir 2>&1)"; then
    echo "ERROR: could not resolve --git-common-dir for $TARGET_WORKTREE: $common_dir" >&2
    exit 1
fi
if [ "$git_dir" = "$common_dir" ]; then
    echo "ERROR: refusing to tombstone the primary worktree ($TARGET_WORKTREE)." >&2
    echo "       Writing STRANDED.md at the canonical clone's own root would make the" >&2
    echo "       canonical clone itself read as a preserved, dead tree." >&2
    exit 1
fi

# ---- refuse branch main / detached HEAD ------------------------------------
if ! branch="$(git -C "$TARGET_WORKTREE" rev-parse --abbrev-ref HEAD 2>&1)"; then
    echo "ERROR: could not resolve the checked-out branch for $TARGET_WORKTREE: $branch" >&2
    exit 1
fi
if [ "$branch" = "HEAD" ]; then
    echo "ERROR: $TARGET_WORKTREE is in detached HEAD state — no branch to tombstone." >&2
    exit 1
fi
if [ "$branch" = "main" ]; then
    echo "ERROR: refusing to tombstone a worktree on branch 'main'." >&2
    exit 1
fi

# ---- write the marker -------------------------------------------------------
MARKER_PATH="${TARGET_WORKTREE}/STRANDED.md"
RELEASED_AT="$(date -u +%Y-%m-%dT%H:%M:%SZ)"

if [ -e "$MARKER_PATH" ]; then
    echo "NOTE: overwriting existing marker at $MARKER_PATH" >&2
fi

{
    echo "# STRANDED WORKTREE — ${TICKET_KEY}"
    echo
    echo "This worktree was released by a stranded-claim recovery because its"
    echo "owning run died mid-ticket. It is preserved on disk so its unpushed work"
    echo "can be salvaged — **it is not a claim** on ${TICKET_KEY} and must not be"
    echo "treated as one by scripts/check-ticket-in-flight.sh (KYO-529)."
    echo
    echo "- Ticket:    ${TICKET_KEY}"
    echo "- Released:  ${RELEASED_AT} (UTC)"
    echo "- Path:      ${TARGET_WORKTREE}"
    echo "- Branch:    ${branch}"
    echo
    echo "If you are a human adopting this tree to resume work: **delete this file"
    echo "first.** As long as STRANDED.md exists here, check-ticket-in-flight.sh"
    echo "will keep reporting this worktree as preserved-but-not-claimed rather"
    echo "than as work in progress — leaving it in place while you work risks a"
    echo "second worker claiming ${TICKET_KEY} out from under you."
    if [ -n "$NOTE" ]; then
        echo
        echo "## Note"
        echo
        echo "$NOTE"
    fi
} >"$MARKER_PATH" || {
    echo "ERROR: failed to write $MARKER_PATH" >&2
    exit 1
}

echo "Wrote tombstone for ${TICKET_KEY} at ${MARKER_PATH}"
exit 0
