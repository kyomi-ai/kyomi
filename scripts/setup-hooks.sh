#!/bin/bash
# setup-hooks.sh — Point this clone at the repo's tracked git hooks.
#
# `core.hooksPath` is a per-clone git config value — it cannot be committed.
# It lives in the shared `.git/config`, not per-worktree, so one run here
# covers the clone and every worktree of it. Because the configured path is
# relative, git resolves it against each worktree's own top level, so each
# worktree correctly activates its own tracked `.githooks/` instead of every
# worktree pointing at wherever this script happened to run (KYO-358). Run
# this once after cloning.
#
# What running this enables:
#   pre-commit — blocks new lint suppressions, the server_fn/REST divergence
#                lint (KYO-122), and unsigned/stale code-review approvals.
#   pre-push   — blocks direct pushes to main (matches the remote ref being
#                updated, see .githooks/pre-push for why).
#
# Safe to re-run.

set -euo pipefail
cd "$(git rev-parse --show-toplevel)"

HOOKS_DIR=".githooks"

if [ ! -d "$HOOKS_DIR" ]; then
    echo "ERROR: ${HOOKS_DIR}/ not found — run this from a Kyomi checkout." >&2
    exit 1
fi

# Relative path: git resolves core.hooksPath relative to the worktree's own
# top level, so this works correctly in the main clone and in every
# worktree. An absolute path here would pin every worktree at whatever
# clone happened to run this script (the exact bug KYO-358 fixes).
git config core.hooksPath "$HOOKS_DIR"

echo "==> core.hooksPath set to '${HOOKS_DIR}' (relative)"

missing_exec=""
hook_count=0
for hook in "$HOOKS_DIR"/*; do
    [ -f "$hook" ] || continue
    hook_count=$((hook_count + 1))
    if [ ! -x "$hook" ]; then
        missing_exec="${missing_exec}  ${hook}\n"
    fi
done

if [ "$hook_count" -eq 0 ]; then
    echo "ERROR: ${HOOKS_DIR}/ contains no hook files." >&2
    exit 1
fi

if [ -n "$missing_exec" ]; then
    echo "ERROR: the following hooks are tracked but not executable:" >&2
    printf '%b' "$missing_exec" >&2
    echo "Fix with: chmod +x <hook> && git add <hook>" >&2
    echo "(git silently skips non-executable hooks — this would otherwise fail open)" >&2
    exit 1
fi

echo "==> Verified $hook_count hook(s) are executable."
echo ""
echo "Active hooks:"
for hook in "$HOOKS_DIR"/*; do
    [ -f "$hook" ] || continue
    echo "  - $(basename "$hook")"
done
