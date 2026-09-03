# ------------------------------------------------------------------------------
# scripts/lib/stale-tooling-guard.sh — make a stale local copy of an
# agent-workflow script LOUD instead of silent. (KYO-632)
#
# Meant to be `source`d, not executed directly (no shebang, not chmod +x) —
# same convention as scripts/lib/canonical-root.sh.
#
# WHY THIS EXISTS
#
# /backlog-fast and /backlog tell agents to run tooling as `scripts/<name>.sh`,
# and every worked example runs it from the canonical clone
# (~/repos/kyomi). Worktree *creation* is careful to cut from `origin/main`
# (KYO-373 exists because someone didn't). Nothing applied that same care to
# the SCRIPTS an agent then runs — the canonical clone's working tree is
# just a checkout that sits there until someone happens to `git pull` it,
# and nothing does that automatically.
#
# These scripts' exit codes gate real decisions — check-ticket-in-flight.sh's
# exit 0 is THE ONLY code that permits claiming a ticket; mark-worktree-
# stranded.sh / mark-branch-stranded.sh release a stranded claim;
# reconcile-merged-tickets.sh, append-review-log.sh, audit-agent-run-deaths.sh
# and mine-review-logs.sh feed other automated decisions. A stale copy of any
# of them is not cosmetic: it fails in the worst available direction, exactly
# because these scripts fail closed on purpose.
#
# CONFIRMED LIVE CONSEQUENCE: check-ticket-in-flight.sh's `--self` flag
# shipped 2026-09-02 (KYO-593, PR #459). Before this guard existed, the copy
# in ~/repos/kyomi predated that PR, so `--self` had never once taken effect
# there. The pre-`--self` script self-excludes by CWD instead — so running it
# from the canonical clone, for a ticket whose implementation lives in a
# worktree, returned exit 1 ON THE CALLER'S OWN BRANCH. The documented
# response to exit 1 is "stop, do not dispatch the reviewer" — abandoning a
# finished, reviewed implementation as a lost race that never happened
# (this is the KYO-291 incident described in check-ticket-in-flight.sh's own
# SELF-EXCLUSION section). Grepping the stale working tree for `--self`
# returns nothing, which reads as "that flag was never shipped" rather than
# "this checkout is behind" — the working-tree hazard
# docs/standards/version-control-working-tree/verify-tree-is-current-before-concluding.md
# already names for conclusions drawn from a checkout; this is that same
# hazard, but the "conclusion" is an exit code an agent acts on immediately
# rather than a claim a person later reads.
#
# WHAT THIS GUARD CHECKS, AND — MORE IMPORTANT — WHAT IT DOES NOT PROVE
#
# It compares the calling script's ON-DISK BYTES against the copy of that
# same path recorded at the LOCAL, already-cached `origin/main` ref
# (`git show origin/main:<path>`). It never runs `git fetch`.
#
# That is a deliberate trade, not an oversight, and it is the resolution
# this ticket calls "compare against the already-fetched ref": it costs
# nothing beyond one `git show` (no network, no latency added to any
# invocation — including check-ticket-in-flight.sh, which already shells out
# to `gh pr list` and could have absorbed a fetch, and the four other guarded
# scripts, which are local-only today and could not). The real trade is
# honesty about what that buys: `origin/main` AS THIS CLONE LAST FETCHED IT
# can itself be stale. If nobody has fetched from origin in this clone
# recently, a script that drifted since that last fetch passes this guard
# silently — the guard's floor is "at least as current as this clone's last
# fetch", not "at least as current as GitHub right now". Note that a git
# worktree SHARES its `origin/main` ref with the main clone and every sibling
# worktree, so a fetch performed in any one of them raises that floor for all
# of them — the floor is per-clone, not per-worktree. Fetching on every
# invocation was rejected: it would add unbounded network latency to every
# single guarded-script call on every machine, forever, to close a gap that
# `git fetch origin main` (already the documented first step of the
# working-tree-currency standard above) closes on demand. Say so out loud
# here rather than let the guard imply a stronger guarantee than it has.
#
# A mismatch is also DIRECTIONLESS on its own — content differing from
# origin/main means either (a) this checkout is BEHIND origin/main (the
# dangerous case this guard exists for), or (b) this checkout has UNPUSHED
# local edits to this very script, e.g. a feature branch actively developing
# it (expected, not dangerous — this ticket's own branch triggers it while
# under development). The guard cannot tell (a) from (b) from content alone,
# and does not pretend to: the message below names both possibilities rather
# than asserting a direction it has no evidence for
# (docs/standards/comments-documentation/no-guarantee-stronger-than-code-enforces.md).
# Teaching the guard to walk ancestry (is HEAD reachable from origin/main?)
# would resolve the ambiguity for the common case, but adds real complexity
# — detached HEAD, rebased branches, shallow clones — to a script whose only
# job is "say something instead of nothing"; the false-positive cost (one
# warning while a developer is mid-edit on the very file) is far cheaper
# than the false-negative cost (silence) this ticket exists to eliminate,
# so that complexity was not taken on.
#
# WARN VS. FAIL — KYOMI_STALE_TOOLING_STRICT
#
# Default behavior on a CONFIRMED mismatch is WARN ONLY: print the loud
# block below to stderr and return, leaving the calling script's own exit
# code untouched. Exiting non-zero by default was rejected on hard evidence,
# not caution: the canonical clone this ticket was filed against was 21
# commits behind origin/main at the time of writing, which means a
# default-fail guard would have blocked every single guarded-script
# invocation on that box today, for every ticket, not just the one this
# bug describes. That is strictly worse than the silent-wrong-answer bug
# being fixed.
#
# Set KYOMI_STALE_TOOLING_STRICT=1 to escalate a CONFIRMED mismatch to a
# hard failure: the calling script exits immediately with
# STALE_TOOLING_GUARD_EXIT_CODE (42 — deliberately outside the 0-3 range
# every guarded script's own documented exit-code contract uses today, so a
# caller that distinguishes exit codes can tell "the guard stopped me" apart
# from "the script's own logic returned this" instead of the two colliding).
#
# Escalation applies ONLY to a confirmed mismatch, NEVER to a "cannot
# determine" result (next section) — see "A SCRIPT MUST NEVER BE BROKEN BY
# ITS OWN GUARD" below for why that split is load-bearing, not an
# inconsistency.
#
# THE "CANNOT DETERMINE" CASE — LOUD, NEVER SILENT, NEVER FATAL
#
# Not inside a git repository, a git too old for `--path-format=absolute`
# (see canonical-root.sh's own note on this — < 2.31), no local
# `origin/main` ref (this clone has never fetched from a remote named
# `origin`), or the path simply does not exist at `origin/main` (a brand
# new or renamed script) all land here. A guard that stayed silent in any
# of these cases would reproduce the exact bug this ticket exists to fix,
# one layer up — so every one of them prints a loud, clearly-labeled
# message to stderr. NONE of them ever change the calling script's exit
# code, REGARDLESS of KYOMI_STALE_TOOLING_STRICT: these are limitations of
# the guard itself (an old git, an unfetched remote, a moved file), not a
# confirmed defect in the script it is guarding, and turning "the guard is
# confused" into "the script now fails" would punish the wrong thing.
#
# A SCRIPT MUST NEVER BE BROKEN BY ITS OWN GUARD
#
# Every external command this file runs is captured via
# `if var="$(cmd 2>/dev/null)"; then ... else ...; fi` — never bare, never
# piped into something that would discard its exit status — matching the
# FAIL CLOSED discipline check-ticket-in-flight.sh documents for the same
# reason. `set -e` does not abort on a failing command used as an `if`
# condition, so a git failure inside this guard degrades to the loud
# "cannot determine" warning above and returns 0, instead of aborting the
# calling script before its real work ever runs. The one path that DOES
# `exit` is the confirmed-mismatch-plus-STRICT path above, and that is a
# deliberate, documented decision, not the guard breaking on its own error.
#
# USAGE
#   SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
#   source "${SCRIPT_DIR}/lib/stale-tooling-guard.sh"
#   stale_tooling_guard "${BASH_SOURCE[0]}"
#
# Pass the calling script's OWN `${BASH_SOURCE[0]}` (not a hand-typed path
# string) so there is nothing here to drift when a script is renamed or
# moved — the relative path compared against `origin/main` is derived from
# that argument plus the script's own git-discovered repo root, never
# hardcoded.
# ------------------------------------------------------------------------------

# The guard's own escalation exit code. Deliberately outside 0-3, the range
# every one of the seven guarded scripts' own documented exit-code
# contracts uses today (see WARN VS. FAIL above) — a caller that cares can
# distinguish "the guard stopped me" from "the script's own logic decided
# this".
STALE_TOOLING_GUARD_EXIT_CODE=42

# Every line this guard writes to stderr, of either message shape, starts
# with this exact literal prefix — deliberately, so a caller (or a test of
# a guarded script) can tell "this stderr line came from the guard" apart
# from "this stderr line came from the script's own logic" by a single
# `grep -v`, without having to pattern-match two structurally different
# message shapes. Discovered necessary while wiring this guard into
# reconcile-merged-tickets.sh: its own self-test asserted one invocation
# produces zero stderr output, which the guard's diagnostic (correctly)
# breaks — the fix was making the guard's own output mechanically
# filterable, not silencing it. Keep this prefix on every line this file
# ever prints; do not add a new stderr line here without it.
STALE_TOOLING_GUARD_LOG_PREFIX="[stale-tooling-guard]"

# _stale_tooling_guard_cannot_determine <rel-or-best-effort-path> <reason>
# — the "cannot determine" message. Always loud, never fatal (see header).
_stale_tooling_guard_cannot_determine() {
    local path="$1" reason="$2" p="$STALE_TOOLING_GUARD_LOG_PREFIX"
    {
        echo "${p} WARNING: could not determine whether '${path}' matches origin/main."
        echo "${p}          Reason: ${reason}"
        echo "${p}          Proceeding WITHOUT a staleness check for this run."
    } >&2
}

# _stale_tooling_guard_mismatch <rel-path> — the confirmed-mismatch message,
# plus the STRICT escalation. Never called for a "cannot determine" result.
_stale_tooling_guard_mismatch() {
    local rel_path="$1" p="$STALE_TOOLING_GUARD_LOG_PREFIX"
    {
        echo "${p}"
        echo "${p} =================================================================="
        echo "${p} STALE TOOLING WARNING: ${rel_path}"
        echo "${p} =================================================================="
        echo "${p} This script's on-disk content does not match the copy this clone"
        echo "${p} last fetched at origin/main. This check cannot tell which of the"
        echo "${p} following is true:"
        echo "${p}"
        echo "${p}   1. This checkout is BEHIND origin/main — the code you are about"
        echo "${p}      to run may be missing a real fix. See KYO-632 / KYO-593 for"
        echo "${p}      a confirmed incident where this silently discarded a"
        echo "${p}      finished, reviewed implementation."
        echo "${p}   2. This checkout has UNPUSHED local edits to this script —"
        echo "${p}      expected if you are actively developing it right now."
        echo "${p}"
        echo "${p} If (1) is possible: run 'git fetch origin main', then diff this"
        echo "${p} file against 'origin/main:${rel_path}' before trusting this"
        echo "${p} script's exit code."
        echo "${p} =================================================================="
        echo "${p}"
    } >&2

    if [ "${KYOMI_STALE_TOOLING_STRICT:-0}" = "1" ]; then
        echo "${p} ERROR: KYOMI_STALE_TOOLING_STRICT=1 — failing instead of warning (exit ${STALE_TOOLING_GUARD_EXIT_CODE})." >&2
        exit "$STALE_TOOLING_GUARD_EXIT_CODE"
    fi
}

# stale_tooling_guard <script-source-path> — the entry point. Pass the
# calling script's own "${BASH_SOURCE[0]}". Never raises under `set -e`
# except via the deliberate STRICT-mode `exit` above.
stale_tooling_guard() {
    local script_path="$1"
    local script_dir script_abs repo_root rel_path
    local script_base

    if ! script_dir="$(cd "$(dirname -- "$script_path")" 2>/dev/null && pwd)"; then
        _stale_tooling_guard_cannot_determine "$script_path" "could not resolve the script's own directory."
        return 0
    fi
    # Captured, not bare — see the "every external command" invariant in this
    # file's header. `basename` will not realistically fail for a path we have
    # already resolved a directory for, but a bare `$(...)` here would abort the
    # HOST script under its `set -e`, which is precisely the one thing this guard
    # must never do.
    if ! script_base="$(basename -- "$script_path" 2>/dev/null)"; then
        _stale_tooling_guard_cannot_determine "$script_path" "could not resolve the script's own filename."
        return 0
    fi
    script_abs="${script_dir}/${script_base}"

    if ! repo_root="$(git -C "$script_dir" rev-parse --path-format=absolute --show-toplevel 2>/dev/null)"; then
        _stale_tooling_guard_cannot_determine "$script_abs" "not inside a git repository, or git predates 2.31's --path-format=absolute (see scripts/lib/canonical-root.sh)."
        return 0
    fi

    case "$script_abs" in
        "$repo_root"/*)
            rel_path="${script_abs#"$repo_root"/}"
            ;;
        *)
            _stale_tooling_guard_cannot_determine "$script_abs" "the script does not resolve inside its own repo root (${repo_root})."
            return 0
            ;;
    esac

    if ! git -C "$repo_root" rev-parse --verify -q origin/main >/dev/null 2>&1; then
        _stale_tooling_guard_cannot_determine "$rel_path" "no local 'origin/main' ref — this clone has never fetched from a remote named 'origin', or the remote uses a different name. Run 'git fetch origin main'."
        return 0
    fi

    local remote_content
    if ! remote_content="$(git -C "$repo_root" show "origin/main:${rel_path}" 2>/dev/null)"; then
        _stale_tooling_guard_cannot_determine "$rel_path" "'git show origin/main:${rel_path}' failed — the path may not exist at origin/main (a brand-new or renamed script)."
        return 0
    fi

    local local_content
    if ! local_content="$(cat -- "$script_abs" 2>/dev/null)"; then
        _stale_tooling_guard_cannot_determine "$rel_path" "could not read '${script_abs}'."
        return 0
    fi

    if [ "$remote_content" = "$local_content" ]; then
        return 0
    fi

    _stale_tooling_guard_mismatch "$rel_path"
    return 0
}
