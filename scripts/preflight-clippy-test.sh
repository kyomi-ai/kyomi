#!/usr/bin/env bash
# ------------------------------------------------------------------------------
# scripts/preflight-clippy-test.sh — self-test for preflight-clippy.sh
# (KYO-629)
#
# Needs no Rust toolchain at all: `cargo` and `rustup` are stubbed as
# executables placed first on PATH, so the suite never compiles anything.
# The defect this ticket fixes is a *flag string* silently drifting between
# .github/workflows/ci.yml and the documented pre-PR gate, so pinning the
# exact argv preflight-clippy.sh hands to `cargo` is precisely the test that
# matters — an end-to-end clippy run would prove the flags work, but only
# argv inspection proves they are the SAME flags CI runs. Everything happens
# under a fresh mktemp -d fixture git repo, removed on exit; the real kyomi
# repo, the real cargo, and the real rustup are never touched.
#
# Test 10 is the load-bearing one: it parses the three `run: cargo clippy`
# lines back out of THIS repo's real .github/workflows/ci.yml (located by
# grepping for that content, never by a hardcoded line number — see
# docs/standards/comments-documentation/anchor-a-citation-to-a-symbol-not-a-line-number.md)
# and diffs them against what preflight-clippy.sh actually invokes. That is
# what makes the drift this ticket exists to fix structurally impossible to
# reintroduce silently, rather than merely documented against.
#
# Exit 0 = all pass, exit 1 = any failure.
# ------------------------------------------------------------------------------

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
SCRIPT="$SCRIPT_DIR/preflight-clippy.sh"
CI_YML="$SCRIPT_DIR/../.github/workflows/ci.yml"
PASS=0
FAIL=0

tmpdir="$(mktemp -d)"
trap 'rm -rf "$tmpdir"' EXIT

# Give the throwaway commit a fixed identity so the suite doesn't depend on
# (or pollute) the real machine's git config, and never touches $HOME.
export GIT_AUTHOR_NAME="KYO-629 Test" GIT_AUTHOR_EMAIL="kyo629-test@kyomi.invalid"
export GIT_COMMITTER_NAME="KYO-629 Test" GIT_COMMITTER_EMAIL="kyo629-test@kyomi.invalid"

# ─── a real, hermetic fixture git repo ───────────────────────────────────────
# preflight-clippy.sh resolves its own working directory via
# `git rev-parse --show-toplevel`, so it needs a real repo to run inside —
# not the actual kyomi checkout, a throwaway one. Branch name is pinned
# explicitly (`checkout -q -b main`) rather than left to `init.defaultBranch`,
# which is `master` on the runner this suite executes on in CI, not `main`
# (the script itself never inspects the branch name, but pinning it keeps
# this fixture consistent with every other suite in this job).
FIXTURE_REPO="$tmpdir/fixture-repo"
mkdir -p "$FIXTURE_REPO"
git init -q "$FIXTURE_REPO"
git -C "$FIXTURE_REPO" checkout -q -b main
echo "fixture" >"$FIXTURE_REPO/README.md"
git -C "$FIXTURE_REPO" add README.md
git -C "$FIXTURE_REPO" commit -q -m init

# ─── stub `cargo`, records argv, replays a configurable exit status ─────────
STUB_BIN="$tmpdir/bin"
mkdir -p "$STUB_BIN"
export CARGO_LOG="$tmpdir/cargo.log"
export RUSTUP_LOG="$tmpdir/rustup.log"
export RUSTUP_TARGETS_FILE="$tmpdir/rustup_targets"

cat >"$STUB_BIN/cargo" <<'STUB'
#!/usr/bin/env bash
# Test-only stand-in for `cargo`. Ignores what it's asked to do and instead
# records the full command line (with "cargo" restored as argv[0], so a
# logged line reads exactly like the `run:` string it's meant to match) to
# CARGO_LOG, one call per line. Exits 0 unless the recorded line is an exact
# match for CARGO_FAIL_LINE, in which case it exits CARGO_FAIL_EXIT (default
# 1) — lets a test make exactly one of several passes "fail" by naming its
# full expected command line. Ships only inside this test's own $tmpdir/bin,
# first on PATH for the duration of the run — never touches the real cargo.
line="cargo $*"
printf '%s\n' "$line" >>"$CARGO_LOG"
if [ -n "${CARGO_FAIL_LINE:-}" ] && [ "$line" = "$CARGO_FAIL_LINE" ]; then
    exit "${CARGO_FAIL_EXIT:-1}"
fi
exit 0
STUB
chmod +x "$STUB_BIN/cargo"

# ─── stub `rustup`, records argv, replays a configurable target list ───────
cat >"$STUB_BIN/rustup" <<'STUB'
#!/usr/bin/env bash
# Test-only stand-in for `rustup`. Records its argv to RUSTUP_LOG, and for
# `target list --installed` (the only subcommand preflight-clippy.sh ever
# calls) prints whatever the test staged in RUSTUP_TARGETS_FILE. Ships only
# inside this test's own $tmpdir/bin — never touches the real rustup or the
# real toolchain installation.
printf 'rustup %s\n' "$*" >>"$RUSTUP_LOG"
if [ "${1:-}" = "target" ] && [ "${2:-}" = "list" ]; then
    cat "$RUSTUP_TARGETS_FILE"
fi
exit 0
STUB
chmod +x "$STUB_BIN/rustup"

export PATH="$STUB_BIN:$PATH"

# ─── per-test reset ──────────────────────────────────────────────────────────
reset_stubs() {
    : >"$CARGO_LOG"
    : >"$RUSTUP_LOG"
    unset CARGO_FAIL_LINE CARGO_FAIL_EXIT || true
    # Default: wasm32-unknown-unknown IS installed, alongside two other
    # targets, so most tests don't have to think about the fail-closed path.
    printf 'wasm32-unknown-emscripten\nwasm32-unknown-unknown\nx86_64-unknown-linux-gnu\n' \
        >"$RUSTUP_TARGETS_FILE"
}
reset_stubs

# ─── invoke the script under test, capturing exit code + combined output ───
RUN_STATUS=""
RUN_OUTPUT=""
run_preflight() {
    # run_preflight [args to preflight-clippy.sh...] — always runs with cwd
    # inside the fixture repo, never the real kyomi checkout.
    local out
    if out="$(cd "$FIXTURE_REPO" && "$SCRIPT" "$@" 2>&1)"; then
        RUN_STATUS=0
    else
        RUN_STATUS=$?
    fi
    RUN_OUTPUT="$out"
}

# ─── assertion helpers (shape matches check-ticket-in-flight-test.sh) ──────
assert_exit() {
    local name="$1" expected="$2"
    if [ "$RUN_STATUS" -eq "$expected" ]; then
        printf "  \xe2\x9c\x93 %s (exit %d)\n" "$name" "$RUN_STATUS"
        PASS=$((PASS + 1))
    else
        printf "  \xe2\x9c\x97 %s \xe2\x80\x94 expected exit %d, got %d\n" "$name" "$expected" "$RUN_STATUS"
        echo "    output:"
        echo "$RUN_OUTPUT" | sed 's/^/    | /'
        FAIL=$((FAIL + 1))
    fi
}

assert_contains() {
    local name="$1" needle="$2"
    if printf '%s' "$RUN_OUTPUT" | grep -qF -- "$needle"; then
        printf "  \xe2\x9c\x93 %s\n" "$name"
        PASS=$((PASS + 1))
    else
        printf "  \xe2\x9c\x97 %s \xe2\x80\x94 expected output to contain: %s\n" "$name" "$needle"
        echo "    output:"
        echo "$RUN_OUTPUT" | sed 's/^/    | /'
        FAIL=$((FAIL + 1))
    fi
}

assert_eq() {
    local name="$1" expected="$2" actual="$3"
    if [ "$expected" = "$actual" ]; then
        printf "  \xe2\x9c\x93 %s\n" "$name"
        PASS=$((PASS + 1))
    else
        printf "  \xe2\x9c\x97 %s\n" "$name"
        echo "    expected: $expected"
        echo "    actual:   $actual"
        FAIL=$((FAIL + 1))
    fi
}

assert_line_count() {
    local name="$1" file="$2" expected="$3" actual
    actual="$(wc -l <"$file" | tr -d ' ')"
    if [ "$actual" -eq "$expected" ]; then
        printf "  \xe2\x9c\x93 %s (%d lines)\n" "$name" "$actual"
        PASS=$((PASS + 1))
    else
        printf "  \xe2\x9c\x97 %s \xe2\x80\x94 expected %d lines, got %d\n" "$name" "$expected" "$actual"
        echo "    file contents:"
        sed 's/^/    | /' "$file"
        FAIL=$((FAIL + 1))
    fi
}

assert_line_contains() {
    local name="$1" file="$2" line_num="$3" needle="$4" line
    line="$(sed -n "${line_num}p" "$file")"
    if printf '%s' "$line" | grep -qF -- "$needle"; then
        printf "  \xe2\x9c\x93 %s\n" "$name"
        PASS=$((PASS + 1))
    else
        printf "  \xe2\x9c\x97 %s \xe2\x80\x94 line %d does not contain: %s\n" "$name" "$line_num" "$needle"
        echo "    line ${line_num}: $line"
        FAIL=$((FAIL + 1))
    fi
}

assert_line_not_contains() {
    local name="$1" file="$2" line_num="$3" needle="$4" line
    line="$(sed -n "${line_num}p" "$file")"
    if printf '%s' "$line" | grep -qF -- "$needle"; then
        printf "  \xe2\x9c\x97 %s \xe2\x80\x94 line %d unexpectedly contains: %s\n" "$name" "$line_num" "$needle"
        echo "    line ${line_num}: $line"
        FAIL=$((FAIL + 1))
    else
        printf "  \xe2\x9c\x93 %s\n" "$name"
        PASS=$((PASS + 1))
    fi
}

echo "Running preflight-clippy self-tests..."
echo

# The known-good literal argv this suite pins passes 1-9 against. Test 10
# separately re-derives these three strings from ci.yml itself and diffs
# them against the same CARGO_LOG shape, so a hand-edited mistake here and a
# real drift in ci.yml are two independently-caught failure modes.
PASS1_UNNARROWED="cargo clippy --locked --workspace --exclude kyomi-desktop -- -D warnings"
PASS2_UNNARROWED="cargo clippy --locked --workspace --exclude kyomi-desktop --all-targets -- -D warnings -A clippy::unwrap_used"
PASS3_LINE="cargo clippy --locked -p kyomi-ui --target wasm32-unknown-unknown --features hydrate -- -D warnings"

# ─── Test 1: unnarrowed run emits exactly three cargo clippy calls, in ──────
# CI's order.
echo "-- Test 1: unnarrowed run, three invocations in CI's order"
reset_stubs
run_preflight
assert_exit "clean run against a fully-stubbed toolchain" 0
assert_line_count "exactly three cargo invocations" "$CARGO_LOG" 3
assert_eq "pass 1 argv" "$PASS1_UNNARROWED" "$(sed -n '1p' "$CARGO_LOG")"
assert_eq "pass 2 argv" "$PASS2_UNNARROWED" "$(sed -n '2p' "$CARGO_LOG")"
assert_eq "pass 3 argv" "$PASS3_LINE" "$(sed -n '3p' "$CARGO_LOG")"
echo

# ─── Test 2: pass 1 has neither --all-targets nor the unwrap_used allow ────
echo "-- Test 2: pass 1 carries no --all-targets and no unwrap_used allow"
reset_stubs
run_preflight
assert_line_not_contains "pass 1 has no --all-targets" "$CARGO_LOG" 1 "--all-targets"
assert_line_not_contains "pass 1 has no -A clippy::unwrap_used" "$CARGO_LOG" 1 "-A clippy::unwrap_used"
echo

# ─── Test 3: pass 2 has both --all-targets and the unwrap_used allow ───────
echo "-- Test 3: pass 2 carries both --all-targets and the unwrap_used allow"
reset_stubs
run_preflight
assert_line_contains "pass 2 has --all-targets" "$CARGO_LOG" 2 "--all-targets"
assert_line_contains "pass 2 has -A clippy::unwrap_used" "$CARGO_LOG" 2 "-A clippy::unwrap_used"
echo

# ─── Test 4: pass 3 carries the wasm32 target and hydrate feature ──────────
echo "-- Test 4: pass 3 carries --target wasm32-unknown-unknown and --features hydrate"
reset_stubs
run_preflight
assert_line_contains "pass 3 has --target wasm32-unknown-unknown" "$CARGO_LOG" 3 "--target wasm32-unknown-unknown"
assert_line_contains "pass 3 has --features hydrate" "$CARGO_LOG" 3 "--features hydrate"
echo

# ─── Test 5: --locked is present on all three ───────────────────────────────
echo "-- Test 5: --locked present on all three passes"
reset_stubs
run_preflight
assert_line_contains "pass 1 has --locked" "$CARGO_LOG" 1 "--locked"
assert_line_contains "pass 2 has --locked" "$CARGO_LOG" 2 "--locked"
assert_line_contains "pass 3 has --locked" "$CARGO_LOG" 3 "--locked"
echo

# ─── Test 6: narrowed run replaces scope, keeps every other flag ───────────
echo "-- Test 6: -p kyomi-auth -p kyomi-agent replaces scope, keeps every other flag"
reset_stubs
run_preflight -p kyomi-auth -p kyomi-agent
assert_exit "narrowed run to two non-kyomi-ui crates" 0
assert_line_count "only two invocations — pass 3 skipped" "$CARGO_LOG" 2
assert_eq "pass 1 argv, narrowed" \
    "cargo clippy --locked -p kyomi-auth -p kyomi-agent -- -D warnings" \
    "$(sed -n '1p' "$CARGO_LOG")"
assert_eq "pass 2 argv, narrowed — keeps --all-targets and the unwrap_used allow" \
    "cargo clippy --locked -p kyomi-auth -p kyomi-agent --all-targets -- -D warnings -A clippy::unwrap_used" \
    "$(sed -n '2p' "$CARGO_LOG")"
assert_contains "summary names pass 3 as explicitly skipped" "SKIPPED"
assert_contains "skip reason names kyomi-ui" "kyomi-ui"
echo

# ─── Test 6b: -p kyomi-ui alone still runs pass 3, unaffected by narrowing ──
# (kyomi-ui being IN the narrowed set is the other half of test 6's logic —
# not one of the ten required tests, but the acceptance criteria explicitly
# describe this branch and it would be silent regression risk otherwise.)
echo "-- Test 6b: -p kyomi-ui keeps pass 3 running, with its own fixed argv"
reset_stubs
run_preflight -p kyomi-ui
assert_exit "narrowed run including kyomi-ui" 0
assert_line_count "all three invocations still run" "$CARGO_LOG" 3
assert_eq "pass 1 argv, narrowed to kyomi-ui" \
    "cargo clippy --locked -p kyomi-ui -- -D warnings" \
    "$(sed -n '1p' "$CARGO_LOG")"
assert_eq "pass 3 argv is unaffected by narrowing (CI hardcodes -p kyomi-ui)" \
    "$PASS3_LINE" "$(sed -n '3p' "$CARGO_LOG")"
echo

# ─── Test 7: a failing pass 1 does not prevent passes 2 and 3 from running ──
echo "-- Test 7: pass 1 failing still lets passes 2 and 3 run; exit is non-zero"
reset_stubs
export CARGO_FAIL_LINE="$PASS1_UNNARROWED"
run_preflight
unset CARGO_FAIL_LINE
assert_exit "overall run reports failure" 1
assert_line_count "all three invocations still happened" "$CARGO_LOG" 3
assert_contains "summary names pass 1 as the failure" "pass 1"
assert_contains "summary says lints were found" "LINTS FOUND"
echo

# ─── Test 8: missing wasm32-unknown-unknown fails closed, exit 3 not 0 ─────
echo "-- Test 8: missing wasm32 target exits 3, not 0"
reset_stubs
printf 'x86_64-unknown-linux-gnu\n' >"$RUSTUP_TARGETS_FILE"
run_preflight
assert_exit "cannot claim success when a pass could not run" 3
assert_line_count "passes 1 and 2 still ran; pass 3 was never invoked" "$CARGO_LOG" 2
assert_contains "names the fix" "rustup target add wasm32-unknown-unknown"
reset_stubs
echo

# ─── Test 9: crates/kyomi-ui/dist is created ───────────────────────────────
echo "-- Test 9: crates/kyomi-ui/dist is created (RustEmbed guard)"
reset_stubs
rm -rf "$FIXTURE_REPO/crates"
run_preflight
if [ -d "$FIXTURE_REPO/crates/kyomi-ui/dist" ]; then
    printf "  \xe2\x9c\x93 %s\n" "crates/kyomi-ui/dist exists after a run"
    PASS=$((PASS + 1))
else
    printf "  \xe2\x9c\x97 %s\n" "crates/kyomi-ui/dist does NOT exist after a run"
    FAIL=$((FAIL + 1))
fi
echo

# ─── Test 10: THE PARITY TEST — ci.yml and preflight-clippy.sh must agree ──
# Located by content (`grep` for the literal "run: cargo clippy" prefix),
# never by a hardcoded line number, per
# docs/standards/comments-documentation/anchor-a-citation-to-a-symbol-not-a-line-number.md
# — ci.yml is edited by many PRs and a line-number anchor here would go
# stale silently.
echo "-- Test 10: parity against the real .github/workflows/ci.yml"
if [ ! -f "$CI_YML" ]; then
    printf "  \xe2\x9c\x97 %s\n" "could not find ci.yml at $CI_YML — cannot run the parity check"
    FAIL=$((FAIL + 1))
else
    mapfile -t CI_CLIPPY_LINES < <(grep -E '^[[:space:]]*run: cargo clippy' "$CI_YML" | sed -E 's/^[[:space:]]*run: //')
    if [ "${#CI_CLIPPY_LINES[@]}" -eq 3 ]; then
        printf "  \xe2\x9c\x93 %s\n" "found exactly three 'run: cargo clippy' lines in ci.yml"
        PASS=$((PASS + 1))
    else
        printf "  \xe2\x9c\x97 %s \xe2\x80\x94 expected 3, found %d\n" "found exactly three 'run: cargo clippy' lines in ci.yml" "${#CI_CLIPPY_LINES[@]}"
        FAIL=$((FAIL + 1))
    fi

    reset_stubs
    run_preflight
    idx=1
    for expected in "${CI_CLIPPY_LINES[@]}"; do
        actual="$(sed -n "${idx}p" "$CARGO_LOG")"
        if [ "$expected" = "$actual" ]; then
            printf "  \xe2\x9c\x93 %s\n" "ci.yml pass ${idx} matches preflight-clippy.sh"
            PASS=$((PASS + 1))
        else
            printf "  \xe2\x9c\x97 %s\n" "ci.yml pass ${idx} DIVERGED from preflight-clippy.sh"
            echo "    ci.yml:              $expected"
            echo "    preflight-clippy.sh: $actual"
            echo "    .github/workflows/ci.yml and scripts/preflight-clippy.sh have drifted apart —"
            echo "    fix scripts/preflight-clippy.sh to match ci.yml (ci.yml is the source of truth;"
            echo "    the script conforms to CI, not the other way round)."
            FAIL=$((FAIL + 1))
        fi
        idx=$((idx + 1))
    done
fi
echo

# ─── Test 11: --help exits 0 and never touches cargo ───────────────────────
echo "-- Test 11: --help exits 0 without invoking cargo"
reset_stubs
run_preflight --help
assert_exit "--help exits 0" 0
assert_line_count "no cargo invocations for --help" "$CARGO_LOG" 0
assert_contains "prints usage" "Usage:"
echo

# ─── Test 12: usage errors exit 2 and never touch cargo ────────────────────
echo "-- Test 12: usage errors exit 2 without invoking cargo"
reset_stubs
run_preflight --bogus-flag
assert_exit "unknown flag exits 2" 2
assert_line_count "no cargo invocations for a usage error" "$CARGO_LOG" 0

reset_stubs
run_preflight -p
assert_exit "-p with no value exits 2" 2
assert_line_count "no cargo invocations when -p has no value" "$CARGO_LOG" 0
echo

echo "Results: $PASS passed, $FAIL failed"
[ "$FAIL" -eq 0 ]
