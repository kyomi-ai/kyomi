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
assert_run "no arguments is a usage error" 2 "Usage:" env -u CARGO_TARGET_DIR "$SCRIPT"
assert_run "--dry-run without a ref is a usage error" 2 "Usage:" env -u CARGO_TARGET_DIR "$SCRIPT" --dry-run
assert_run "too many arguments is a usage error" 2 "Usage:" env -u CARGO_TARGET_DIR "$SCRIPT" v2.7.17 extra
assert_run "an option in the ref position is a usage error" 2 "Usage:" env -u CARGO_TARGET_DIR "$SCRIPT" --dry-run --upload
assert_run "a non-tag is refused without --dry-run" 2 "is not a release tag" env -u CARGO_TARGET_DIR "$SCRIPT" main
assert_run "a malformed tag is refused" 2 "is not a release tag" env -u CARGO_TARGET_DIR "$SCRIPT" v2.7
assert_value "a release tag selects release mode" "release v2.7.17" \
    "$(in_script parse_args v2.7.17)"
assert_value "--dry-run accepts any ref" "dry-run origin/main" \
    "$(in_script parse_args --dry-run origin/main)"

# --- loud failures ---------------------------------------------------------------
assert_run "a missing tool is named and stops the run" 1 "required tool not found on PATH: no-such-tool-xyz" \
    in_script require_tool no-such-tool-xyz
if [ "$(uname -s)-$(uname -m)" != "Darwin-arm64" ]; then
    assert_run "a host that is not an Apple-silicon Mac is refused" 1 "must run on a Mac" \
        env -u CARGO_TARGET_DIR "$SCRIPT" v2.7.17
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

# --- origin remote -> GH_REPO ------------------------------------------------------
assert_value "https remote" "kyomi-ai/kyomi" "$(in_script origin_slug https://github.com/kyomi-ai/kyomi.git)"
assert_value "https remote without .git" "kyomi-ai/kyomi" "$(in_script origin_slug https://github.com/kyomi-ai/kyomi)"
assert_value "ssh remote" "kyomi-ai/kyomi" "$(in_script origin_slug git@github.com:kyomi-ai/kyomi.git)"
assert_run "a remote that is not github.com is refused" 1 "not a github.com remote" \
    in_script origin_slug https://example.com/kyomi-ai/kyomi.git

# --- dmg verification, with stand-ins for the macOS tools -------------------------
# inspect_app is what decides "arm64-only and ad-hoc signed", so it is run with
# fake lipo/plutil/codesign whose answers the test controls.
mkdir -p "$TMP/stubs" "$TMP/app/Contents/MacOS"
printf '#!/bin/sh\n' > "$TMP/app/Contents/MacOS/kyomi-desktop"
chmod +x "$TMP/app/Contents/MacOS/kyomi-desktop"
echo plist > "$TMP/app/Contents/Info.plist"
: > "$TMP/fake.dmg"
cat > "$TMP/stubs/lipo" <<'SH'
#!/bin/sh
case "$1" in
    -info) echo "Non-fat file: $2 is architecture: $STUB_ARCHS" ;;
    -archs) echo "$STUB_ARCHS" ;;
esac
SH
printf '#!/bin/sh\necho "{ plist }"\n' > "$TMP/stubs/plutil"
cat > "$TMP/stubs/codesign" <<'SH'
#!/bin/sh
if [ "$1" = "--verify" ]; then
    exit "$STUB_VERIFY_RC"
fi
printf '%s\n' "$STUB_SIGNATURE" >&2
SH
chmod +x "$TMP/stubs/lipo" "$TMP/stubs/plutil" "$TMP/stubs/codesign"
GOOD_SIGNATURE=$'Identifier=ai.kyomi.desktop\nSignature=adhoc'

inspect() { (
    export PATH="$TMP/stubs:$PATH" STUB_ARCHS="$1" STUB_VERIFY_RC="$2" STUB_SIGNATURE="$3"
    source "$SCRIPT"
    inspect_app "$TMP/app" "$TMP/fake.dmg"
); }
assert_run "an arm64, verified, ad-hoc signed app passes" 0 "" inspect arm64 0 "$GOOD_SIGNATURE"
assert_run "an x86_64 app is refused" 1 "not arm64 only" inspect x86_64 0 "$GOOD_SIGNATURE"
assert_run "a universal app is refused" 1 "not arm64 only" inspect "arm64 x86_64" 0 "$GOOD_SIGNATURE"
assert_run "a signature that does not verify is refused" 1 "does not verify" inspect arm64 1 "$GOOD_SIGNATURE"
assert_run "a Developer ID signature is refused (CI signed ad hoc)" 1 "not ad-hoc signed" \
    inspect arm64 0 $'Identifier=ai.kyomi.desktop\nSignature size=9000'
assert_run "an unexpected bundle identifier is refused" 1 "identifier is not ai.kyomi.desktop" \
    inspect arm64 0 $'Identifier=com.example.other\nSignature=adhoc'

# --- cleanup and interruption -------------------------------------------------------
assert_run "a relative CARGO_TARGET_DIR is refused before anything runs" 1 "must be an absolute path" \
    env CARGO_TARGET_DIR=target "$SCRIPT" v2.7.17

# cleanup must remove the work directory even when its start directory is gone
# (it runs under set -e; a failing step must not skip the rest) and must exit 0.
mkdir -p "$TMP/clean/work/src"
set +e
(
    set -e
    source "$SCRIPT"
    START_DIR="$TMP/no-such-dir" REPO="$TMP" WORK="$TMP/clean/work" SRC="$TMP/clean/work/src"
    cleanup
) > /dev/null 2> "$TMP/err"
rc=$?
set -e
checks=$((checks + 1))
if [ "$rc" -eq 0 ] && [ ! -e "$TMP/clean/work" ]; then
    pass "cleanup removes the work directory even when the start directory is gone"
else
    fail "cleanup removes the work directory even when the start directory is gone (rc $rc)" "$TMP/err"
fi

# On a Mac, cleanup must detach a dmg that is still mounted (an interrupted
# verification) before it deletes the work directory.
if [ "$(uname -s)" = Darwin ]; then
    mkdir -p "$TMP/mnt/work/src" "$TMP/mnt/payload"
    echo x > "$TMP/mnt/payload/file"
    hdiutil create -quiet -srcfolder "$TMP/mnt/payload" -volname KyomiTest "$TMP/mnt/test.dmg"
    physical="$(cd "$TMP/mnt/work" && pwd -P)"
    mkdir -p "$physical/dmg-mount"
    hdiutil attach -readonly -nobrowse -mountpoint "$physical/dmg-mount" "$TMP/mnt/test.dmg" > /dev/null
    set +e
    (
        set -e
        source "$SCRIPT"
        START_DIR="$TMP" REPO="$TMP" WORK="$physical" SRC="$physical/src"
        cleanup
    ) > /dev/null 2> "$TMP/err"
    rc=$?
    set -e
    checks=$((checks + 1))
    if [ "$rc" -eq 0 ] && ! mount | grep -qF " on $physical/dmg-mount " && [ ! -e "$physical" ]; then
        pass "cleanup detaches a still-mounted dmg and removes the work directory"
    else
        hdiutil detach -force "$physical/dmg-mount" > /dev/null 2>&1 || true
        fail "cleanup detaches a still-mounted dmg and removes the work directory (rc $rc)" "$TMP/err"
    fi
fi

# A TERM sent to the script while a long step runs must stop the step's whole
# process group promptly, not after the step ends.
# The step starts a grandchild and records its pid, so the test can check that
# the signal reached the whole process group and not only the step's shell.
printf '#!/bin/sh\nsleep 300 &\necho $! > "%s/grandchild.pid"\nwait\n' "$TMP" > "$TMP/longstep"
chmod +x "$TMP/longstep"
(
    source "$SCRIPT"
    trap 'terminate 143' TERM
    run_in "$TMP" "$TMP/longstep"
) > /dev/null 2>&1 &
runner=$!
sleep 1
kill -TERM "$runner"
waited=0
while kill -0 "$runner" 2> /dev/null && [ "$waited" -lt 10 ]; do
    sleep 1
    waited=$((waited + 1))
done
checks=$((checks + 1))
if kill -0 "$runner" 2> /dev/null; then
    kill -KILL "$runner" 2> /dev/null || true
    pkill -f "$TMP/longstep" 2> /dev/null || true
    fail "TERM stops the running step promptly (still running after 10s)"
elif kill -0 "$(cat "$TMP/grandchild.pid")" 2> /dev/null; then
    kill -KILL "$(cat "$TMP/grandchild.pid")" 2> /dev/null || true
    fail "TERM stops the running step's process group (the step's child survived)"
else
    pass "TERM stops the running step and its process group promptly (${waited}s)"
fi

# --- the config strip CI did (BSD sed -i '', so only where that sed exists) --------
if [ "$(uname -s)" = Darwin ]; then
    mkdir -p "$TMP/strip/.cargo" "$TMP/strip/crates/kyomi-ui"
    printf 'keep1\n# BEGIN_LOCAL_DEV_LINKER\nlinker\n# END_LOCAL_DEV_LINKER\nkeep2\n# BEGIN_LOCAL_DEV_DEBUGINFO\ndbg\n# END_LOCAL_DEV_DEBUGINFO\nkeep3\n' > "$TMP/strip/.cargo/config.toml"
    printf 'a = 1\ncargo_profile = "wasm-dev"\nb = 2\n' > "$TMP/strip/crates/kyomi-ui/Trunk.toml"
    in_script strip_local_dev_config "$TMP/strip"
    assert_value "the local-dev linker and debuginfo blocks are stripped" "keep1 keep2 keep3" \
        "$(tr '\n' ' ' < "$TMP/strip/.cargo/config.toml" | sed 's/ $//')"
    assert_value "the trunk cargo_profile line is stripped" "a = 1 b = 2" \
        "$(tr '\n' ' ' < "$TMP/strip/crates/kyomi-ui/Trunk.toml" | sed 's/ $//')"
fi

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
