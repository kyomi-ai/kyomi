#!/usr/bin/env bash
# ------------------------------------------------------------------------------
# scripts/release-macos-desktop-test.sh — self-test for release-macos-desktop.sh
#
# The script builds a Mac installer, which only an Apple-silicon Mac can do, so
# this covers what any host (CI runs it on Linux) can: argument handling, the
# loud-failure paths, and reading the tool pins out of the real repository so a
# renamed pin in desktop.yml or setup-trunk fails here, not on release day.
# The build itself is exercised by running the script with --dry-run on a Mac.
#
# Usage: ./scripts/release-macos-desktop-test.sh
# ------------------------------------------------------------------------------

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
SCRIPT="$SCRIPT_DIR/release-macos-desktop.sh"
REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"

TMP="$(mktemp -d -t release-macos-desktop-test-XXXXXX)"
trap 'rm -rf "$TMP"' EXIT

fails=0
checks=0

pass() { printf 'PASS  %s\n' "$1"; }
fail() {
    printf 'FAIL  %s\n' "$1"
    [ -z "${2:-}" ] || sed 's/^/      /' "$2"
    fails=$((fails + 1))
}

# assert_run <label> <expected-exit> <stderr-substring-or-empty> <command...>
assert_run() {
    local label="$1" expected="$2" needle="$3" actual
    shift 3
    checks=$((checks + 1))
    set +e
    "$@" > "$TMP/out" 2> "$TMP/err"
    actual=$?
    set -e
    if [ "$actual" -ne "$expected" ]; then
        fail "$label (exit $actual, expected $expected)" "$TMP/err"
    elif [ -n "$needle" ] && ! grep -qF -- "$needle" "$TMP/err"; then
        fail "$label (stderr lacks: $needle)" "$TMP/err"
    else
        pass "$label (exit $actual)"
    fi
}

# assert_value <label> <expected> <actual>
assert_value() {
    checks=$((checks + 1))
    if [ "$2" = "$3" ]; then
        pass "$1"
    else
        printf 'FAIL  %s\n      expected: %s\n      actual:   %s\n' "$1" "$2" "$3"
        fails=$((fails + 1))
    fi
}

# Run a function from the script in a subshell, so die() ends only that call.
in_script() { (source "$SCRIPT" && "$@"); }

# --- argument handling ---------------------------------------------------------
assert_run "no arguments is a usage error" 2 "Usage:" "$SCRIPT"
assert_run "--dry-run without a ref is a usage error" 2 "Usage:" "$SCRIPT" --dry-run
assert_run "too many arguments is a usage error" 2 "Usage:" "$SCRIPT" v2.7.17 extra
assert_run "an option in the ref position is a usage error" 2 "Usage:" "$SCRIPT" --dry-run --upload
assert_run "a non-tag is refused without --dry-run" 2 "is not a release tag" "$SCRIPT" main
assert_run "a malformed tag is refused" 2 "is not a release tag" "$SCRIPT" v2.7
assert_value "a release tag selects release mode" "release v2.7.17" \
    "$(in_script parse_args v2.7.17)"
assert_value "--dry-run accepts any ref" "dry-run origin/main" \
    "$(in_script parse_args --dry-run origin/main)"

# --- loud failures ---------------------------------------------------------------
assert_run "a missing tool is named and stops the run" 1 "required tool not found on PATH: no-such-tool-xyz" \
    in_script require_tool no-such-tool-xyz
if [ "$(uname -s)-$(uname -m)" != "Darwin-arm64" ]; then
    assert_run "a host that is not an Apple-silicon Mac is refused" 1 "must" \
        "$SCRIPT" v2.7.17
fi

# --- pins are read from the repository, not hardcoded ----------------------------
mkdir -p "$TMP/tree/.github/actions/setup-trunk" "$TMP/tree/.github/workflows"
cat > "$TMP/tree/.github/actions/setup-trunk/action.yml" <<'YML'
runs:
  steps:
    - id: pin
      shell: bash
      run: echo "version=9.8.7" >> "$GITHUB_OUTPUT"
YML
cat > "$TMP/tree/.github/workflows/desktop.yml" <<'YML'
env:
  CARGO_TERM_COLOR: always
  WASM_BINDGEN_VERSION: 1.2.3
  TAILWIND_VERSION: v4.5.6
  BINARYEN_VERSION: version_321
YML
assert_value "trunk pin is read from setup-trunk" "9.8.7" "$(in_script pin_trunk "$TMP/tree")"
assert_value "wasm-bindgen pin is read from desktop.yml" "1.2.3" "$(in_script pin_wasm_bindgen "$TMP/tree")"
assert_value "tailwind pin is read from desktop.yml" "v4.5.6" "$(in_script pin_tailwind "$TMP/tree")"
assert_value "binaryen pin is read from desktop.yml" "version_321" "$(in_script pin_binaryen "$TMP/tree")"

grep -v TAILWIND_VERSION "$TMP/tree/.github/workflows/desktop.yml" > "$TMP/tree/gone.yml"
mv "$TMP/tree/gone.yml" "$TMP/tree/.github/workflows/desktop.yml"
assert_run "a pin that is gone from the workflow fails loudly" 1 "no version found" \
    in_script pin_tailwind "$TMP/tree"
echo "  BINARYEN_VERSION: version_322" >> "$TMP/tree/.github/workflows/desktop.yml"
assert_run "a pin written twice fails loudly" 1 "more than one version" \
    in_script pin_binaryen "$TMP/tree"
assert_run "a tree without the file fails loudly" 1 "does not exist" \
    in_script pin_trunk "$TMP/nowhere"

# --- the real repository still carries every pin the script reads ---------------
assert_value "real trunk pin is a version" "ok" \
    "$(in_script pin_trunk "$REPO_ROOT" | grep -qE '^[0-9]+\.[0-9]+\.[0-9]+$' && echo ok)"
assert_value "real wasm-bindgen pin is a version" "ok" \
    "$(in_script pin_wasm_bindgen "$REPO_ROOT" | grep -qE '^[0-9]+\.[0-9]+\.[0-9]+$' && echo ok)"
assert_value "real tailwind pin is a version" "ok" \
    "$(in_script pin_tailwind "$REPO_ROOT" | grep -qE '^v[0-9]+\.[0-9]+\.[0-9]+$' && echo ok)"
assert_value "real binaryen pin is a version" "ok" \
    "$(in_script pin_binaryen "$REPO_ROOT" | grep -qE '^version_[0-9]+$' && echo ok)"

echo
if [ "$fails" -ne 0 ]; then
    echo "$fails of $checks checks FAILED"
    exit 1
fi
echo "$checks/$checks checks passed"
