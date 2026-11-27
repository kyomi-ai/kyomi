#!/usr/bin/env bash
# ------------------------------------------------------------------------------
# scripts/lint/check-trunk-pin-test.sh — self-test for check-trunk-pin.sh
#
# Builds synthetic .github trees in a tmp directory and runs the linter against
# each, then proves the real tree is clean.
#
# Usage: ./scripts/lint/check-trunk-pin-test.sh
# ------------------------------------------------------------------------------

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
LINTER="$SCRIPT_DIR/check-trunk-pin.sh"

TMP="$(mktemp -d -t check-trunk-pin-test-XXXXXX)"
trap 'rm -rf "$TMP"' EXIT

fails=0
checks=0

# make_tree <name> -> prints the .github dir with a valid action and one clean workflow
make_tree() {
    local dir="$TMP/$1/.github"
    mkdir -p "$dir/actions/setup-trunk" "$dir/workflows"
    cat > "$dir/actions/setup-trunk/action.yml" <<'YML'
runs:
  using: composite
  steps:
    - id: pin
      shell: bash
      run: echo "version=0.21.14" >> "$GITHUB_OUTPUT"
YML
    cat > "$dir/workflows/ci.yml" <<'YML'
jobs:
  build:
    steps:
      - uses: ./.github/actions/setup-trunk
YML
    echo "$dir"
}

assert_exit() {
    local label="$1" expected="$2" dir="$3" actual
    checks=$((checks + 1))
    set +e
    "$LINTER" "$dir" > "$TMP/out" 2>&1
    actual=$?
    set -e
    if [ "$actual" -eq "$expected" ]; then
        printf 'PASS  %-52s (exit %d)\n' "$label" "$actual"
    else
        printf 'FAIL  %-52s (exit %d, expected %d)\n' "$label" "$actual" "$expected"
        sed 's/^/      /' "$TMP/out"
        fails=$((fails + 1))
    fi
}

dir="$(make_tree clean)"
assert_exit "clean tree passes" 0 "$dir"

dir="$(make_tree unpinned-install-action)"
printf '      - uses: taiki-e/install-action@trunk\n' >> "$dir/workflows/ci.yml"
assert_exit "install-action@trunk is rejected" 1 "$dir"

dir="$(make_tree install-action-tool)"
printf '      - uses: taiki-e/install-action@v2\n        with:\n          tool: trunk@0.21.14\n' >> "$dir/workflows/ci.yml"
assert_exit "install-action with tool: trunk is rejected" 1 "$dir"

dir="$(make_tree own-download)"
printf '      - run: curl -fsSL https://github.com/trunk-rs/trunk/releases/download/v0.21.14/trunk.tar.gz\n' >> "$dir/workflows/release.yml"
assert_exit "a download from the Trunk releases is rejected" 1 "$dir"

dir="$(make_tree own-cargo-install)"
printf '      - run: cargo install trunk --locked\n' >> "$dir/workflows/desktop.yml"
assert_exit "cargo install trunk is rejected" 1 "$dir"

dir="$(make_tree own-version)"
printf 'env:\n  TRUNK_VERSION: 0.21.13\n' >> "$dir/workflows/release.yml"
assert_exit "a TRUNK_VERSION of its own is rejected" 1 "$dir"

dir="$(make_tree no-action)"
rm -r "$dir/actions"
assert_exit "a missing action is rejected" 1 "$dir"

dir="$(make_tree no-pin)"
printf 'runs:\n  using: composite\n  steps:\n    - run: echo latest\n' > "$dir/actions/setup-trunk/action.yml"
assert_exit "an action with no pinned version is rejected" 1 "$dir"

dir="$(make_tree range-pin)"
printf '    - run: echo "version=0.21.14" >> o\n    - run: echo "version=0.21.15" >> o\n' >> "$dir/actions/setup-trunk/action.yml"
assert_exit "an action with two pins is rejected" 1 "$dir"

checks=$((checks + 1))
if "$LINTER" > "$TMP/out" 2>&1; then
    printf 'PASS  %-52s (exit 0)\n' "the real .github tree is clean"
else
    printf 'FAIL  %-52s\n' "the real .github tree is clean"
    sed 's/^/      /' "$TMP/out"
    fails=$((fails + 1))
fi

echo "$((checks - fails))/$checks checks passed"
[ "$fails" -eq 0 ]
