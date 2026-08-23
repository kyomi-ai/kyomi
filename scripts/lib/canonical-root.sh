# ------------------------------------------------------------------------------
# scripts/lib/canonical-root.sh — shared canonical-clone resolution (KYO-396)
#
# Meant to be `source`d, not executed directly (no shebang, not chmod +x).
#
# WHY THIS EXISTS
#
# Before this file, the "find the canonical clone from any worktree" logic
# existed in two hand-written places: scripts/mine-review-logs.sh (the read
# side, KYO-386) and prose in ~/.claude/agents/code-review-architect.md (the
# write side, KYO-387). scripts/append-review-log.sh (KYO-396) makes the
# write side mechanical too, which would have made a third hand-written copy
# of the same resolution — exactly the trigger condition in
# docs/standards/code-organization/third-copy-of-test-helper-is-extraction-trigger.md
# ("two copies can be justified independently; a third means the
# justification was wrong and the copies will drift"). This file is the
# extraction: both scripts source it and call the same functions.
#
# WHY --path-format=absolute IS LOAD-BEARING, NOT DECORATION
#
# `git rev-parse --git-common-dir` returns the shared .git directory for a
# worktree (the main checkout's .git, not the worktree's own .git
# file-pointer stub) — that's what lets any linked worktree find the
# canonical clone. But its *un-prefixed* output is absolute when run from a
# worktree and a bare relative `.git` when run from the main checkout. So
# `dirname` on the unqualified form silently yields `.` in the one case that
# currently looks like it works (running from the main clone), sending the
# caller to its own cwd instead of the canonical clone — a wrong path, not a
# loud error. `--path-format=absolute` forces an absolute path in both
# cases, and was added in git 2.31.
#
# This machine runs git 2.54.0 (`git --version`), so the failure branch in
# resolve_canonical_root() below cannot be exercised by actually running an
# old git here. It is verified two other ways instead of by execution on
# real old git: (1) by reasoning from the documented behavior above, and (2)
# scripts/append-review-log-test.sh stubs a `git` on PATH that rejects
# `--path-format=...` the way git < 2.31 does, and asserts the guard fires.
# See that test file's "old git" case for the executed half of that proof.
#
# USAGE
#   SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
#   source "${SCRIPT_DIR}/lib/canonical-root.sh"
#   logs_dir=$(resolve_review_logs_dir) || exit 1
# ------------------------------------------------------------------------------

# resolve_canonical_root — print the absolute path to the canonical clone's
# top level (the directory containing .git) on stdout, or print a
# diagnostic to stderr and return 1. Never falls back to a relative path.
resolve_canonical_root() {
    if ! git rev-parse --is-inside-work-tree >/dev/null 2>&1; then
        echo "ERROR: not inside a git repository — cannot locate the canonical clone." >&2
        return 1
    fi

    local common_dir
    # `set -e` does not abort on a failing command used as an `if`
    # condition, so capturing the output directly here is safe — matches
    # the fail-loud pattern in scripts/mine-review-logs.sh.
    if ! common_dir=$(git rev-parse --path-format=absolute --git-common-dir 2>/dev/null); then
        echo "ERROR: 'git rev-parse --path-format=absolute' is unsupported by this git version" >&2
        echo "       (requires git >= 2.31). Refusing to guess the canonical clone path." >&2
        return 1
    fi

    dirname "$common_dir"
}

# resolve_review_logs_dir — print the absolute path to the canonical
# docs/review-logs directory on stdout, or return 1 (resolve_canonical_root
# has already printed the diagnostic).
#
# THIS IS THE SINGLE POINT OF CHANGE for KYO-394 (proposed move of the
# canonical review-log location to ~/repos/kyomi-private): if/when that
# ticket lands, only the path expression below needs to change. Both the
# read side (mine-review-logs.sh) and the write side (append-review-log.sh)
# call this function rather than building the path themselves, so they pick
# up the move automatically.
resolve_review_logs_dir() {
    local canonical_root
    canonical_root=$(resolve_canonical_root) || return 1
    echo "${canonical_root}/docs/review-logs"
}
