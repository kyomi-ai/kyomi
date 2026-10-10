#!/usr/bin/env bash
# ------------------------------------------------------------------------------
# scripts/retire-worktree.sh — the ONLY supported way to remove a linked
# `kyomi-wt-*` git worktree. (KYO-733)
#
# WHY THIS EXISTS
#
# On 2026-09-10 a bulk worktree cleanup ran `git worktree remove` on 19
# `kyomi-wt-*` trees. One of them had a live agent writing into it seconds
# earlier — its staged, uncommitted work was destroyed. The same sweep also
# deleted a tree that had been explicitly tombstoned with STRANDED.md (see
# mark-worktree-stranded.sh) for salvage, which by definition is "do not
# delete". `ps` alone is a known-insufficient liveness signal: an agent
# sitting between tool calls has no child process running in the tree at
# all, so a `ps`-only check sees nothing live seconds before it destroys
# unrecoverable work. The reliable signal is recent non-target/ file
# writes, the same one /backlog-fast Step 0.5 check 3 already computes by
# hand in prose — this script is the single, tested, executable answer,
# following the same KYO-422 principle check-ticket-in-flight.sh is built
# on: callers invoke one script, they do not restate the rule themselves.
#
# WHAT IT REFUSES ON (checks a–e — every one is evaluated, and every
# reason found is reported, not just the first)
#
#   a. RECENT WRITES. Any file outside `target/` and `.git` modified within
#      the last LIVENESS_WINDOW_MINUTES minutes.
#   b. A LIVE PROCESS CWD. Any process (other than this script's own PID
#      and its direct children — see PROC SELF-EXCLUSION below) whose
#      current working directory is inside the tree. Corroboration only,
#      never the sole signal this script relies on — but a hit here still
#      refuses on its own, same as any other reason.
#   c. UNCOMMITTED WORK. `git status --porcelain` is non-empty (tracked
#      changes, staged changes, or untracked non-ignored files).
#   d. UNPUSHED WORK. Commits on HEAD unreachable from any `origin`
#      remote-tracking ref, computed AFTER `git fetch --prune origin` (a
#      stale tracking ref for a since-deleted remote branch would
#      otherwise read as "pushed" when it is not).
#   e. A STRANDED.md TOMBSTONE. Written by mark-worktree-stranded.sh — the
#      tree is preserved for salvage by definition, full stop.
#
# Also a usage error (exit 2, not overridable by --force — see below) for:
# a path that does not exist; a path that is not a registered linked
# worktree of the repository it lives in (checked via
# `git worktree list --porcelain`); or the PRIMARY worktree / canonical
# clone (the first entry `git worktree list` ever reports).
#
# PROC SELF-EXCLUSION — WHY ONLY $$ AND ITS DIRECT CHILDREN
#
# This script never `cd`s into the target path — every git/find command
# below is invoked with an explicit path argument (`git -C`, `find
# "$TARGET"`), never a bare command relying on cwd. Because of that, EVERY
# subprocess it spawns (mktemp, find, git, date, readlink, …) inherits its
# cwd from whatever process invoked this script, not from the tree — the
# only way one of this script's own subprocesses could end up with a cwd
# inside the tree is if the INVOKING SHELL already had cwd there. That
# case is exactly rule (b)'s "the invoking shell's cwd being inside the
# tree SHOULD refuse" — the invoking shell is a distinct process (this
# script's PPID, not its own PID) and is deliberately left IN the scan, so
# it is caught there on its own merits. Excluding only `$$` (this script's
# own PID) and PIDs whose immediate parent is `$$` (its direct children)
# is therefore sufficient to stop this script's own plumbing from being
# reported as a second, redundant "live process" hit for the very same
# underlying cause, without suppressing the real signal. Grandchildren of
# this script are not walked — nothing this script spawns forks a further
# child while holding a cwd inside the tree, since nothing here ever `cd`s
# there.
#
# /PROC FAIL-CLOSED BOUNDARY — DECIDED, NOT ACCIDENTAL
#
#   - /proc missing or not readable AT ALL (not a directory, or this
#     process cannot list it) → the ENTIRE check could not be completed →
#     exit 3, fail closed, nothing removed. This machine's /proc is always
#     present and listable; the check exists for defence in depth and for
#     any environment where it is not (there is no macOS/BSD target here —
#     see scripts/reconcile-merged-tickets.sh's own precedent for the same
#     stance on `date -d` below).
#   - A PER-PID failure — EACCES reading another user's
#     /proc/<pid>/{stat,cwd}, or the pid exiting between the directory
#     listing and the read (a normal race on any busy box) — is SKIPPED,
#     not treated as a reason to refuse everything. Requiring every single
#     process on the box to be individually readable by this script's
#     invoking user would make the check permanently unusable on any
#     shared or CI runner, which is a worse failure mode than occasionally
#     missing one process we have no permission to see anyway (an
#     unprivileged process cannot act on this tree either).
#
# UNCOMMITTED/UNPUSHED FAIL-CLOSED (KYO-511 SHAPE — READ BEFORE "FIXING" IT
# BACK)
#
# Every external command whose result gates a decision is captured via
# `if var="$(cmd 2>&1)"; then ... else ...; fi` — the `if` inspects that
# command's own exit status directly. Nothing here pipes a status-bearing
# command into `wc -l`/`grep -c`/`head`, which is exactly the shape that
# made `git ls-remote ... | wc -l` report "0" (looks like "nothing found")
# on a network failure indistinguishably from a genuinely empty result in
# check-ticket-in-flight.sh's own KYO-511 incident. `find` on this
# machine is `bfs`, which REJECTS GNU's relative `-newermt '30 minutes
# ago'` outright — it exits 1 and prints NOTHING, which is the fail-open
# shape this rule exists to catch if its exit status were ever discarded.
# The cutoff is therefore computed once, up front, as an absolute
# ISO-8601 timestamp via `date -d`, and handed to `find -newermt`, which
# both GNU find and bfs accept.
#
# `date -d` IS GNU-ONLY, DELIBERATELY (no BSD/macOS `-v` fallback) — same
# stance scripts/reconcile-merged-tickets.sh already takes ("there is no
# macOS/BSD target here"). This repo's CI and every worktree on this
# machine run Linux; adding a `date -v` branch nothing exercises is dead
# code, not portability.
#
# WHY --force NEVER OVERRIDES A "COULD NOT COMPLETE" RESULT (exit 3)
#
# --force is documented, both here and in the ticket, as the override for
# a KNOWN, understood finding — e.g. "this branch's commits are unpushed
# because the remote branch was deleted after a squash-merge, and a human
# has confirmed the PR shipped". That is a categorically different thing
# from "this script could not determine whether the work is safe to
# discard" (fetch failed, `find` failed, /proc was unreadable). Forcing
# past an *unknown* is not an informed override, it is exactly the
# fail-open shape KYO-511 and this ticket both exist to close. --force
# therefore overrides reasons (a)–(e) — actual findings — and NEVER
# converts an exit-3 "could not complete" into anything but exit 3. It
# also never overrides the two usage errors (primary worktree, not a
# registered worktree) — those are not findings about the tree's state,
# they are "this isn't a request retire-worktree.sh can execute at all".
#
# USAGE
#
#   retire-worktree.sh <path>
#   retire-worktree.sh --force <path> <path>
#
#   <path>            the linked worktree to remove. Resolved to its
#                     canonical absolute toplevel before anything else.
#   --force           override refusal reasons (a)–(e) after printing every
#                     one it is overriding. The path MUST be given twice,
#                     identically (after canonicalization) — a mismatch is
#                     a usage error (exit 2), not a refusal. Requiring the
#                     second copy is a deliberate typo guard on a command
#                     that is about to run `git worktree remove --force`.
#
# This script removes ONLY the worktree (`git worktree remove`). IT DOES
# NOT DELETE THE BRANCH — that is the caller's decision to make separately,
# with its own tooling (this workflow already has
# scripts/mark-branch-stranded.sh for "the branch's work might still be
# needed"; an outright `git branch -D` is nowhere in this script on
# purpose). It also never runs `git worktree prune` and never touches any
# worktree other than the one named.
#
# EXIT CODES (mirrors scripts/check-ticket-in-flight.sh's 0/1/2/3 shape)
#
#   0 — removed.
#   1 — refused: at least one of checks (a)–(e) found live or unsaved
#       work, and --force was not given (or found nothing to override but
#       the git-level removal itself failed unexpectedly — see below).
#   2 — usage error: wrong argument shape, --force's two paths do not
#       match after canonicalization, path missing, path is not a git
#       working tree, path is not a registered linked worktree of its
#       repository, or path IS the primary worktree / canonical clone.
#       Never overridden by --force.
#   3 — a check could not be completed (unsupported git version for
#       `--path-format=absolute`, `git worktree list` itself failed,
#       `find` failed, /proc was wholly unreadable, `git fetch --prune
#       origin` failed, or `git rev-list` failed), OR `git worktree
#       remove` itself failed after every check passed (or was
#       overridden) — in both cases nothing was removed and this is
#       treated exactly like a refusal. Never overridden by --force.
#
# Pure bash + git + coreutils (find, date, realpath, readlink). No `gh`,
# no Rust toolchain, no network beyond the `git fetch --prune origin` that
# check (d) itself performs.
# ------------------------------------------------------------------------------

set -euo pipefail

SCRIPT_NAME="$(basename "$0")"
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=lib/stale-tooling-guard.sh
source "${SCRIPT_DIR}/lib/stale-tooling-guard.sh"
stale_tooling_guard "${BASH_SOURCE[0]}"

# The only knob this script has. Named once, used everywhere it matters
# (the `find -newermt` cutoff computation and every message that cites it)
# rather than repeating the literal 30 (see task requirement: "keep the
# window in one named constant").
readonly LIVENESS_WINDOW_MINUTES=30

usage() {
    cat >&2 <<EOF
Usage: $SCRIPT_NAME <path>
       $SCRIPT_NAME --force <path> <path>

  <path>     the linked worktree to remove (resolved to its canonical
             absolute toplevel).
  --force    override refusal reasons a-e (recent writes, a live process
             cwd, uncommitted work, unpushed commits, a STRANDED.md
             tombstone) after printing each one being overridden. The path
             must be given TWICE, identically, as a typo guard. Never
             overrides a usage error or a "could not complete a check"
             result.

Removes ONLY the worktree (git worktree remove). Does NOT delete the
branch - that is the caller's job.

Exit codes:
  0  removed
  1  refused - a check found live/unsaved work (see printed reasons)
  2  usage error (bad arguments, path missing, not a worktree, not
     registered, or the primary worktree / canonical clone) - --force
     never overrides this
  3  a check could not be completed, or the removal itself failed after
     all checks passed - treated exactly like a refusal, nothing removed
     - --force never overrides this
EOF
}

if [ "$#" -eq 0 ]; then
    usage
    exit 2
fi

FORCE=0
PATH_ARG_1=""
PATH_ARG_2=""

if [ "$1" = "--force" ]; then
    FORCE=1
    shift
    if [ "$#" -ne 2 ]; then
        echo "ERROR: --force requires the path to be given TWICE: $SCRIPT_NAME --force <path> <path>" >&2
        exit 2
    fi
    PATH_ARG_1="$1"
    PATH_ARG_2="$2"
else
    if [ "$#" -ne 1 ]; then
        usage
        exit 2
    fi
    PATH_ARG_1="$1"
    PATH_ARG_2="$1"
fi

# ---- both path arguments must exist as directories -------------------------
for p in "$PATH_ARG_1" "$PATH_ARG_2"; do
    if [ ! -d "$p" ]; then
        echo "ERROR: path does not exist or is not a directory: $p" >&2
        exit 2
    fi
done

# ---- resolve the first path to its canonical absolute toplevel -------------
# --path-format=absolute requires git >= 2.31 (same as mark-worktree-
# stranded.sh's own use of this flag). This script's exit-code contract has
# a dedicated "could not complete a check" code (3), unlike mark-worktree-
# stranded.sh's 0/1/2 shape, so an unsupported git version is routed there
# instead of to a generic error - resolving the canonical path is itself a
# check this script cannot safely skip.
if ! git -C "$PATH_ARG_1" rev-parse --is-inside-work-tree >/dev/null 2>&1; then
    echo "ERROR: not a git working tree: $PATH_ARG_1" >&2
    exit 2
fi
if ! TARGET="$(git -C "$PATH_ARG_1" rev-parse --path-format=absolute --show-toplevel 2>&1)"; then
    echo "ERROR: could not resolve the canonical path of $PATH_ARG_1: $TARGET" >&2
    echo "       ('git rev-parse --path-format=absolute' requires git >= 2.31; refusing to guess.)" >&2
    exit 3
fi

# ---- --force: the second path must resolve to the exact same worktree -----
if [ "$FORCE" -eq 1 ]; then
    if ! TARGET_2="$(git -C "$PATH_ARG_2" rev-parse --path-format=absolute --show-toplevel 2>&1)"; then
        echo "ERROR: --force's second path does not resolve to a git worktree: $PATH_ARG_2 ($TARGET_2)" >&2
        exit 2
    fi
    if [ "$TARGET" != "$TARGET_2" ]; then
        echo "ERROR: --force requires the identical path twice as a typo guard; '$PATH_ARG_1' and '$PATH_ARG_2' resolve to different worktrees ($TARGET vs $TARGET_2)" >&2
        exit 2
    fi
fi

# TARGET_REAL: symlink-resolved form of TARGET, used only for comparing
# against /proc/<pid>/cwd below (the kernel always reports a fully-resolved
# cwd). TARGET itself - straight from git's own absolute-path resolution -
# is what is compared against git worktree list's own output, git commands,
# find, and the STRANDED.md path, so it stays byte-identical to what git
# reports elsewhere. Falls back to TARGET if realpath somehow fails (it
# should not, on an already-confirmed-existing directory).
TARGET_REAL="$(realpath -e "$TARGET" 2>/dev/null || printf '%s' "$TARGET")"

# ---- must be a REGISTERED LINKED worktree, and must NOT be the primary ----
if ! WT_LIST="$(git -C "$TARGET" worktree list --porcelain 2>&1)"; then
    echo "ERROR: 'git worktree list --porcelain' failed: $WT_LIST" >&2
    exit 3
fi

PRIMARY_PATH=""
FOUND=0
IS_PRIMARY=0
CURRENT_PATH=""
while IFS= read -r line; do
    case "$line" in
        "worktree "*)
            CURRENT_PATH="${line#worktree }"
            if [ -z "$PRIMARY_PATH" ]; then
                PRIMARY_PATH="$CURRENT_PATH"
            fi
            if [ "$CURRENT_PATH" = "$TARGET" ]; then
                FOUND=1
                if [ "$CURRENT_PATH" = "$PRIMARY_PATH" ]; then
                    IS_PRIMARY=1
                fi
            fi
            ;;
        *) ;;
    esac
done <<<"$WT_LIST"

if [ "$FOUND" -ne 1 ]; then
    echo "ERROR: $TARGET is not a registered linked worktree of its repository (git worktree list does not name it)" >&2
    exit 2
fi
if [ "$IS_PRIMARY" -eq 1 ]; then
    echo "ERROR: refusing to operate on the primary worktree / canonical clone: $TARGET" >&2
    echo "       retire-worktree.sh only ever removes LINKED worktrees." >&2
    exit 2
fi

echo "Evaluating worktree for removal: $TARGET"
[ "$FORCE" -eq 1 ] && echo "(--force given: refusal reasons a-e will be overridden, not the usage checks above.)"
echo

declare -a REASONS=()    # findings a-e -- overridable by --force
declare -a INCOMPLETE=() # a check that could not run -- NEVER overridable

# ==== check (a): recent writes outside target/ and .git =====================
if ! CUTOFF="$(date -d "${LIVENESS_WINDOW_MINUTES} minutes ago" +%Y-%m-%dT%H:%M:%S 2>&1)"; then
    INCOMPLETE+=("recent-write check: could not compute the ${LIVENESS_WINDOW_MINUTES}-minute cutoff ('date -d' failed: $CUTOFF)")
else
    find_stderr_file="$(mktemp)"
    if RECENT_FILES="$(find "$TARGET" \( -path "$TARGET/target" -o -path "$TARGET/.git" \) -prune -o -type f -newermt "$CUTOFF" -print 2>"$find_stderr_file")"; then
        if [ -n "$RECENT_FILES" ]; then
            declare -a recent_arr=()
            readarray -t recent_arr <<<"$RECENT_FILES"
            sample=""
            i=0
            for f in "${recent_arr[@]}"; do
                i=$((i + 1))
                [ "$i" -le 3 ] && sample="${sample}${sample:+, }${f}"
            done
            REASONS+=("recent write(s): ${#recent_arr[@]} file(s) outside target/ and .git modified within the last ${LIVENESS_WINDOW_MINUTES}m, e.g. ${sample}")
        fi
    else
        # bfs (this machine's `find`) exits 1 and prints NOTHING on a
        # rejected expression - this branch is what makes that loud instead
        # of silently reading as "no recent files" (see header, KYO-511 shape).
        INCOMPLETE+=("recent-write check: 'find' failed: $(cat "$find_stderr_file")")
    fi
    rm -f "$find_stderr_file"
fi

# ==== check (b): a process whose cwd is inside the tree =====================
# pid_ppid <pid> -- print the pid's parent pid on stdout, or fail. Reads
# /proc/<pid>/stat; the comm field (2nd, in parens) can itself contain
# spaces and parens, so the split is anchored on the LAST ")" rather than
# on field position. Wrapped in a function so `set --` only touches this
# function's own positional parameters, never the script's real $1/$2.
pid_ppid() {
    local pid="$1" stat_content rest
    stat_content="$(cat "/proc/$pid/stat" 2>/dev/null)" || return 1
    rest="${stat_content##*) }"
    set -- $rest
    [ -n "${2:-}" ] || return 1
    printf '%s\n' "$2"
}

if [ ! -d /proc ] || [ ! -r /proc ]; then
    INCOMPLETE+=("process-cwd check: /proc is not a readable directory on this system - cannot determine whether any process has this tree as its cwd")
else
    self_pid="$$"
    declare -a proc_hits=()
    for pid_dir in /proc/[0-9]*; do
        [ -d "$pid_dir" ] || continue
        pid="${pid_dir#/proc/}"
        [ "$pid" = "$self_pid" ] && continue

        # Skip this script's own direct children (mktemp/find/git/date/etc
        # it spawned) - see PROC SELF-EXCLUSION in the header for why only
        # one level is excluded and the invoking shell is deliberately not.
        if parent="$(pid_ppid "$pid")" && [ "$parent" = "$self_pid" ]; then
            continue
        fi

        # A pid that vanished between the glob and here, or one this
        # user cannot read /proc/<pid>/cwd for (another user's process),
        # is skipped - not a reason to refuse everything (see header).
        cwd_link="$(readlink "$pid_dir/cwd" 2>/dev/null)" || continue
        [ -n "$cwd_link" ] || continue
        case "$cwd_link" in
            "$TARGET_REAL" | "$TARGET_REAL"/*)
                proc_hits+=("pid ${pid} (cwd ${cwd_link})")
                ;;
        esac
    done
    if [ "${#proc_hits[@]}" -gt 0 ]; then
        REASONS+=("live process cwd inside the tree: ${proc_hits[*]}")
    fi
fi

# ==== check (c): uncommitted work ============================================
if ! STATUS_OUT="$(git -C "$TARGET" status --porcelain 2>&1)"; then
    INCOMPLETE+=("uncommitted-work check: 'git status --porcelain' failed: $STATUS_OUT")
else
    if [ -n "$STATUS_OUT" ]; then
        declare -a status_arr=()
        readarray -t status_arr <<<"$STATUS_OUT"
        sample=""
        i=0
        for l in "${status_arr[@]}"; do
            i=$((i + 1))
            [ "$i" -le 3 ] && sample="${sample}${sample:+; }${l}"
        done
        REASONS+=("uncommitted work: git status --porcelain shows ${#status_arr[@]} line(s), e.g. ${sample}")
    fi
fi

# ==== check (d): unpushed work ===============================================
# Refresh first - a stale tracking ref for a remote branch since deleted
# would otherwise read as "pushed" when it is not (see header).
if ! FETCH_OUT="$(git -C "$TARGET" fetch --prune origin 2>&1)"; then
    INCOMPLETE+=("unpushed-work check: 'git fetch --prune origin' failed: $FETCH_OUT")
else
    if ! UNPUSHED="$(git -C "$TARGET" rev-list HEAD --not --remotes=origin 2>&1)"; then
        INCOMPLETE+=("unpushed-work check: 'git rev-list HEAD --not --remotes=origin' failed: $UNPUSHED")
    else
        if [ -n "$UNPUSHED" ]; then
            declare -a unpushed_shas=()
            readarray -t unpushed_shas <<<"$UNPUSHED"
            declare -a named=()
            for sha in "${unpushed_shas[@]}"; do
                [ -n "$sha" ] || continue
                subject="$(git -C "$TARGET" log -1 --format='%s' "$sha" 2>/dev/null || printf '(subject unavailable)')"
                short="$(git -C "$TARGET" rev-parse --short "$sha" 2>/dev/null || printf '%s' "$sha")"
                named+=("${short} ${subject}")
            done
            # Squash-merges mean a merged branch's commits are never on
            # origin/main - they land on origin/<branch> instead, and if
            # that remote branch was deleted after merge they will show up
            # here as unpushed. That is the intended fail-closed behaviour,
            # not a bug: --force is the override once a human has confirmed
            # the work actually shipped (e.g. the PR merged).
            REASONS+=("unpushed commit(s) on HEAD, unreachable from any origin ref: $(
                IFS='; '
                printf '%s' "${named[*]}"
            ) - if this has already shipped (PR merged, remote branch deleted), re-run with --force once confirmed")
        fi
    fi
fi

# ==== check (e): STRANDED.md tombstone =======================================
if [ -f "$TARGET/STRANDED.md" ]; then
    REASONS+=("STRANDED.md tombstone present at ${TARGET}/STRANDED.md - preserved for salvage by definition (see mark-worktree-stranded.sh)")
fi

# ---- verdict -----------------------------------------------------------------
echo

if [ "${#INCOMPLETE[@]}" -gt 0 ]; then
    echo "RESULT: COULD NOT COMPLETE ALL CHECKS - refusing to remove ${TARGET}"
    for i in "${INCOMPLETE[@]}"; do
        echo "  x ${i}"
    done
    echo
    echo "Nothing was removed. This is never overridden by --force."
    exit 3
fi

if [ "${#REASONS[@]}" -gt 0 ]; then
    if [ "$FORCE" -eq 1 ]; then
        echo "OVERRIDING (--force) the following refusal reason(s) for ${TARGET}:"
        for r in "${REASONS[@]}"; do
            echo "  ! ${r}"
        done
        echo
    else
        echo "RESULT: REFUSED - ${TARGET} looks live or has unsaved/unpushed work:"
        for r in "${REASONS[@]}"; do
            echo "  - ${r}"
        done
        echo
        echo "Nothing was removed. If every reason above is a false positive you have"
        echo "personally verified, re-run as:"
        echo "  $SCRIPT_NAME --force ${TARGET} ${TARGET}"
        exit 1
    fi
else
    echo "RESULT: CLEAR - no recent writes, no live process cwd, no uncommitted or"
    echo "        unpushed work, no STRANDED.md tombstone."
fi

# ---- remove ------------------------------------------------------------------
# KNOWN, ACCEPTED RACE WINDOW: nothing re-checks the tree between the last
# check above (d, the unpushed-work fetch) and the `git worktree remove`
# call below. Accepted because: no network call happens in that window (the
# only fetch already ran as part of check (d)); the window itself is
# milliseconds wide, bounded by however long `git worktree remove` takes to
# start; and a writer arriving inside it is, by construction, outside what
# any pre-removal check can prevent - the check can only attest to the
# tree's state as of when IT ran, and re-running it immediately before the
# removal call would just move the window, not close it.
# Run from the primary worktree, not from inside TARGET itself - TARGET is
# about to be deleted, and PRIMARY_PATH is guaranteed to still exist after.
declare -a remove_args=(worktree remove)
[ "$FORCE" -eq 1 ] && remove_args+=(--force)
remove_args+=("$TARGET")

if ! REMOVE_OUT="$(git -C "$PRIMARY_PATH" "${remove_args[@]}" 2>&1)"; then
    echo "ERROR: 'git ${remove_args[*]}' failed after all checks passed (or were overridden): $REMOVE_OUT" >&2
    echo "       Nothing was removed. This is treated as a check that could not" >&2
    echo "       complete, not as a refusal - it is never overridden by --force." >&2
    exit 3
fi

echo "Removed worktree: ${TARGET}"
echo "NOTE: the branch itself was NOT deleted - that is the caller's responsibility."
exit 0
