#!/usr/bin/env bash
# ------------------------------------------------------------------------------
# scripts/append-review-log.sh — append a code-review entry to the canonical
# daily review log, from any worktree. (KYO-396)
#
# WHY THIS EXISTS
#
# KYO-387 fixed review logs being destroyed by `git worktree remove` by
# rewriting the *instruction* in ~/.claude/agents/code-review-architect.md to
# resolve the canonical clone before writing. That is correct, but it is
# only an instruction: it holds only as long as every reviewer invocation
# actually follows it, and that is exactly the property that made the
# original bug hard to see in the first place. Three observed cases where
# an invocation didn't follow it: KYO-371 (2 of 3 review cycles wrote to the
# worktree), KYO-386 (wrote to the canonical clone unprompted, i.e. without
# being told to — the compliance was accidental, not guaranteed), KYO-397
# (wrote to the worktree — one `git worktree remove` from permanent loss,
# caught only because someone checked by hand before cleanup).
#
# This script makes the write path mechanical instead of instructional, the
# same way scripts/mine-review-logs.sh (KYO-386) made the read path
# mechanical: it resolves the canonical clone itself via the shared helper
# in scripts/lib/canonical-root.sh (KYO-396) rather than relying on an agent
# to reason its way there correctly on every single invocation. The agent
# definition is expected to call this script instead of describing the
# resolution in prose — but ~/.claude/agents/code-review-architect.md is
# local-only and untracked, outside any repo, so that half of the fix
# cannot be delivered by a PR to this repo (see docs/standards — a clean
# diff does not mean a complete ticket when the mechanism lives in
# gitignored or out-of-repo files, KYO-393). The exact replacement prose for
# that file is in this ticket's PR description.
#
# CONCURRENCY
#
# Multiple review cycles for the same ticket can run from different
# worktrees (or the canonical clone itself) and append around the same
# time. Every append resolves to the same canonical file, so an `flock` on
# a lock file next to it serializes writers — each append happens as one
# atomic write while holding that lock — so concurrent-ish invocations land
# as whole, non-interleaved entries in the order they acquired the lock.
#
# EXIT CODES
#   0 — entry appended successfully.
#   1 — usage/environment error: unexpected arguments, not inside a git
#       repository, or git is older than 2.31 and doesn't support
#       `--path-format=absolute` (see scripts/lib/canonical-root.sh).
#   2 — stdin was empty (or all-whitespace). Refuses to write a blank
#       entry rather than silently appending an empty section — a caller
#       that got here with nothing to say has a bug of its own.
#
# USAGE
#   scripts/append-review-log.sh <<'EOF'
#   ...review entry markdown, in the format documented in
#   ~/.claude/agents/code-review-architect.md's
#   "Review Log — Append After Every Review" section...
#   EOF
#
#   Or piped:
#     some-command-producing-an-entry | scripts/append-review-log.sh
#
#   Takes no arguments. Appends to
#   <canonical-root>/docs/review-logs/$(date +%F).md (today's date),
#   creating the directory and/or file if they don't exist yet.
# ------------------------------------------------------------------------------

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=lib/canonical-root.sh
source "${SCRIPT_DIR}/lib/canonical-root.sh"

if [ "$#" -ne 0 ]; then
    echo "ERROR: no arguments expected — the review entry is read from stdin." >&2
    echo "Usage: $0 <<'EOF' ... EOF   (or: some-command | $0)" >&2
    exit 1
fi

entry="$(cat)"

# Reject empty/whitespace-only stdin rather than writing a blank entry.
if [ -z "${entry//[[:space:]]/}" ]; then
    echo "ERROR: stdin was empty — refusing to append a blank review-log entry." >&2
    exit 2
fi

# resolve_review_logs_dir() already prints a diagnostic to stderr on
# failure (either "not inside a git repository" or "unsupported git
# version" — see scripts/lib/canonical-root.sh).
if ! logs_dir=$(resolve_review_logs_dir); then
    exit 1
fi

mkdir -p "$logs_dir"

target_file="${logs_dir}/$(date +%F).md"
lock_file="${logs_dir}/.append.lock"

# Serialize concurrent appenders so entries never interleave mid-write, and
# never land in an order that mixes two entries together. `flock` on FD 200
# is held for the whole critical section: the trailing-newline check and
# the append itself both happen while the lock is held, so a writer never
# observes (or corrupts) a partial write from another one.
(
    flock 200
    if [ -s "$target_file" ]; then
        last_char="$(tail -c1 -- "$target_file")"
        if [ -n "$last_char" ]; then
            printf '\n' >> "$target_file"
        fi
    fi
    printf '%s\n' "$entry" >> "$target_file"
) 200>"$lock_file"

echo "==> Appended entry to ${target_file}" >&2
