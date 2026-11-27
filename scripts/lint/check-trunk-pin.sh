#!/usr/bin/env bash
# ------------------------------------------------------------------------------
# scripts/lint/check-trunk-pin.sh — one pinned Trunk everywhere (KYO-1001)
#
# Every workflow that builds a WASM app installs Trunk through the composite
# action .github/actions/setup-trunk, which is the only place the version is
# written. Before this, ci.yml and rust-stable-canary.yml took whatever
# `taiki-e/install-action@trunk` resolved to that day while release.yml and
# desktop.yml pinned a version of their own, so the chart app a release shipped
# was not built by the Trunk CI had tested.
#
# Fails when
#   - the action is missing, or does not pin one exact X.Y.Z release, or
#   - any other workflow or action installs Trunk itself (the unpinned
#     install-action form, a download from the Trunk releases, a cargo install,
#     or a TRUNK_VERSION of its own).
#
# Usage: check-trunk-pin.sh [<github-dir>]   (default: the repo's .github)
# Exit:  0 clean, 1 violations, 2 usage / unreadable input
# ------------------------------------------------------------------------------

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
GITHUB_DIR="${1:-$SCRIPT_DIR/../../.github}"
ACTION="$GITHUB_DIR/actions/setup-trunk/action.yml"

if [ ! -d "$GITHUB_DIR" ]; then
    echo "ERROR: $GITHUB_DIR is not a directory" >&2
    exit 2
fi

violations=0

if [ ! -f "$ACTION" ]; then
    echo "::error::$ACTION is missing: it is where the pinned Trunk version lives."
    exit 1
fi

pins="$(grep -Eo 'version=[0-9]+\.[0-9]+\.[0-9]+([^0-9.]|$)' "$ACTION" || true)"
if [ "$(printf '%s' "$pins" | grep -c .)" -ne 1 ]; then
    echo "::error::$ACTION must pin exactly one Trunk release written as version=X.Y.Z."
    violations=$((violations + 1))
fi

# Every other workflow and action: none may install Trunk by itself.
while IFS= read -r file; do
    [ "$file" = "$ACTION" ] && continue
    hits="$(grep -nE 'install-action@trunk|tool: *trunk|trunk-rs/trunk|cargo (install|binstall) +trunk|TRUNK_VERSION' "$file" || true)"
    if [ -n "$hits" ]; then
        echo "::error file=$file::installs or pins Trunk itself; use ./.github/actions/setup-trunk instead:"
        echo "$hits"
        violations=$((violations + 1))
    fi
done < <(find "$GITHUB_DIR" -type f \( -name '*.yml' -o -name '*.yaml' \) | sort)

if [ "$violations" -gt 0 ]; then
    exit 1
fi
echo "Trunk is installed only by .github/actions/setup-trunk, pinned to one release."
