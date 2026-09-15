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
# KYO-723 dependency note: `jq` is NOT stubbed. preflight-clippy.sh's -p
# narrowing now derives a unified --features set by piping real `cargo
# metadata` output through real `jq` (see derive_unified_features() in
# preflight-clippy.sh) — only `cargo` itself is stubbed, via the
# CARGO_METADATA_FIXTURE JSON below, to answer `cargo metadata` without a
# real Cargo.toml/workspace. That means this suite now depends on `jq`
# being installed on whatever box runs it, same as preflight-clippy.sh
# itself; it ships preinstalled on GitHub's hosted ubuntu runners and was
# already present on the machine this was written and verified on.
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

# ─── fixture `cargo metadata` output (KYO-723) ──────────────────────────────
# What derive_unified_features() in preflight-clippy.sh actually reads:
# .packages[].name/.id to find a crate's package id, .resolve.nodes[].id to
# match it, .resolve.nodes[].features to read its unified feature set. Every
# other field cargo metadata normally emits (workspace_members,
# target_directory, workspace_root, ...) is omitted — the derivation never
# touches them, and keeping the fixture minimal keeps it obviously hermetic.
# Five fixture packages, chosen to cover every derivation case the tests
# below exercise:
#   kyomi-auth, kyomi-agent   — resolve to ["default"] only (no extra
#                               feature), reusing the crate names Test 6
#                               already narrows to, so "narrowing to a
#                               default-only crate adds no --features" is
#                               proven on real per-crate identities.
#   kyomi-ui                  — resolves to ["default","slack","ssr"], the
#                               exact KYO-723 bug scenario: apps/server's
#                               feature unification turning on kyomi-ui's
#                               ssr (and slack) features that a bare `-p
#                               kyomi-ui` never would.
#   fixture-multi-a/-b        — each carry one distinct extra feature, used
#                               only to prove multiple -p crates combine and
#                               sort correctly across crates.
export CARGO_METADATA_FIXTURE="$tmpdir/cargo-metadata-fixture.json"
cat >"$CARGO_METADATA_FIXTURE" <<'JSON'
{
  "packages": [
    {"name": "kyomi-auth", "id": "path+file:///fixture/kyomi-auth#0.1.0"},
    {"name": "kyomi-agent", "id": "path+file:///fixture/kyomi-agent#0.1.0"},
    {"name": "kyomi-ui", "id": "path+file:///fixture/kyomi-ui#0.1.0"},
    {"name": "fixture-multi-a", "id": "path+file:///fixture/fixture-multi-a#0.1.0"},
    {"name": "fixture-multi-b", "id": "path+file:///fixture/fixture-multi-b#0.1.0"}
  ],
  "resolve": {
    "nodes": [
      {"id": "path+file:///fixture/kyomi-auth#0.1.0", "features": ["default"]},
      {"id": "path+file:///fixture/kyomi-agent#0.1.0", "features": ["default"]},
      {"id": "path+file:///fixture/kyomi-ui#0.1.0", "features": ["default", "slack", "ssr"]},
      {"id": "path+file:///fixture/fixture-multi-a#0.1.0", "features": ["default", "alpha"]},
      {"id": "path+file:///fixture/fixture-multi-b#0.1.0", "features": ["default", "beta"]}
    ]
  }
}
JSON

cat >"$STUB_BIN/cargo" <<'STUB'
#!/usr/bin/env bash
# Test-only stand-in for `cargo`. Ignores what it's asked to do and instead
# records the full command line (with "cargo" restored as argv[0], so a
# logged line reads exactly like the `run:` string it's meant to match) to
# CARGO_LOG, one call per line. Exits 0 unless the recorded line is an exact
# match for CARGO_FAIL_LINE, in which case it exits CARGO_FAIL_EXIT (default
# 1) — lets a test make exactly one of several passes "fail" by naming its
# full expected command line. For a `cargo metadata ...` call that is not
# the configured failure line, prints the fixture JSON at
# CARGO_METADATA_FIXTURE to stdout (KYO-723) before exiting 0 — that JSON is
# then parsed by the REAL jq binary inside preflight-clippy.sh's
# derive_unified_features(), exactly as a real `cargo metadata` invocation's
# output would be. Ships only inside this test's own $tmpdir/bin, first on
# PATH for the duration of the run — never touches the real cargo.
line="cargo $*"
printf '%s\n' "$line" >>"$CARGO_LOG"
if [ -n "${CARGO_FAIL_LINE:-}" ] && [ "$line" = "$CARGO_FAIL_LINE" ]; then
    exit "${CARGO_FAIL_EXIT:-1}"
fi
if [ "${1:-}" = "metadata" ]; then
    cat "$CARGO_METADATA_FIXTURE"
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
# (KYO-723: narrowing now also derives a --features set via `cargo
# metadata`, so a narrowed run logs a leading `cargo metadata` call ahead of
# the two clippy passes. kyomi-auth and kyomi-agent are fixtured to resolve
# to ["default"] only — see CARGO_METADATA_FIXTURE above — so this test also
# covers "a crate whose resolved features are only default produces no
# --features flag" on two real per-crate identities.)
echo "-- Test 6: -p kyomi-auth -p kyomi-agent replaces scope, keeps every other flag"
reset_stubs
run_preflight -p kyomi-auth -p kyomi-agent
assert_exit "narrowed run to two non-kyomi-ui crates" 0
assert_line_count "cargo metadata lookup + two clippy invocations — pass 3 skipped" "$CARGO_LOG" 3
assert_eq "line 1 is the KYO-723 cargo metadata derivation lookup" \
    "cargo metadata --locked --format-version 1" \
    "$(sed -n '1p' "$CARGO_LOG")"
assert_eq "pass 1 argv, narrowed — both fixture crates resolve only to 'default', so no --features flag" \
    "cargo clippy --locked -p kyomi-auth -p kyomi-agent -- -D warnings" \
    "$(sed -n '2p' "$CARGO_LOG")"
assert_eq "pass 2 argv, narrowed — keeps --all-targets and the unwrap_used allow, still no --features" \
    "cargo clippy --locked -p kyomi-auth -p kyomi-agent --all-targets -- -D warnings -A clippy::unwrap_used" \
    "$(sed -n '3p' "$CARGO_LOG")"
assert_contains "summary names pass 3 as explicitly skipped" "SKIPPED"
assert_contains "skip reason names kyomi-ui" "kyomi-ui"
echo

# ─── Test 6b: -p kyomi-ui alone still runs pass 3, unaffected by narrowing ──
# (kyomi-ui being IN the narrowed set is the other half of test 6's logic —
# not one of the ten required tests, but the acceptance criteria explicitly
# describe this branch and it would be silent regression risk otherwise.)
# KYO-723: this is the headline bug scenario. The fixture resolves kyomi-ui
# to ["default","slack","ssr"] (mirroring apps/server's real feature
# unification), so passes 1 and 2 must now carry
# --features kyomi-ui/slack,kyomi-ui/ssr — without it, pass 2 is exactly the
# narrowed run that used to misreport credential_status_indicates_connected
# as dead_code. Pass 3's argv stays the CI-hardcoded literal, unaffected by
# any derivation, exactly as before this ticket.
echo "-- Test 6b: -p kyomi-ui keeps pass 3 running, with derived --features on passes 1-2"
reset_stubs
run_preflight -p kyomi-ui
assert_exit "narrowed run including kyomi-ui" 0
assert_line_count "cargo metadata lookup + all three clippy invocations still run" "$CARGO_LOG" 4
assert_eq "line 1 is the KYO-723 cargo metadata derivation lookup" \
    "cargo metadata --locked --format-version 1" \
    "$(sed -n '1p' "$CARGO_LOG")"
assert_eq "pass 1 argv, narrowed to kyomi-ui — carries the derived --features (the KYO-723 fix)" \
    "cargo clippy --locked -p kyomi-ui --features kyomi-ui/slack,kyomi-ui/ssr -- -D warnings" \
    "$(sed -n '2p' "$CARGO_LOG")"
assert_eq "pass 2 argv, narrowed to kyomi-ui — same derived --features, plus --all-targets and the unwrap_used allow" \
    "cargo clippy --locked -p kyomi-ui --features kyomi-ui/slack,kyomi-ui/ssr --all-targets -- -D warnings -A clippy::unwrap_used" \
    "$(sed -n '3p' "$CARGO_LOG")"
assert_eq "pass 3 argv is unaffected by narrowing or derivation (CI hardcodes -p kyomi-ui)" \
    "$PASS3_LINE" "$(sed -n '4p' "$CARGO_LOG")"
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

# ─── Test 13: multiple -p crates combine and sort features ACROSS crates ───
# (KYO-723) fixture-multi-a resolves to ["default","alpha"], fixture-multi-b
# to ["default","beta"]. Given in reverse (-b before -a) to prove the
# combined --features value is sorted independently of -p order — SCOPE_ARGS
# keeps the crates in the order given (CI parity for -p itself), but the
# derived --features list does not inherit that order.
echo "-- Test 13: multiple -p crates each contribute correctly package-qualified, sorted features"
reset_stubs
run_preflight -p fixture-multi-b -p fixture-multi-a
assert_exit "narrowed run across two feature-bearing fixture crates" 0
assert_line_count "cargo metadata lookup + two clippy invocations — pass 3 skipped" "$CARGO_LOG" 3
assert_eq "pass 1 argv carries both crates' features, sorted across crates regardless of -p order" \
    "cargo clippy --locked -p fixture-multi-b -p fixture-multi-a --features fixture-multi-a/alpha,fixture-multi-b/beta -- -D warnings" \
    "$(sed -n '2p' "$CARGO_LOG")"
assert_eq "pass 2 argv carries the same derived --features plus --all-targets and the unwrap_used allow" \
    "cargo clippy --locked -p fixture-multi-b -p fixture-multi-a --features fixture-multi-a/alpha,fixture-multi-b/beta --all-targets -- -D warnings -A clippy::unwrap_used" \
    "$(sed -n '3p' "$CARGO_LOG")"
echo

# ─── Test 14: a single default-only crate emits no --features flag ─────────
# (KYO-723) The single-crate mirror of Test 6's two-crate case, so "resolves
# only to default -> no --features" is proven on a single -p too, not only
# in combination.
echo "-- Test 14: a single narrowed crate resolving only to 'default' emits no --features flag"
reset_stubs
run_preflight -p kyomi-auth
assert_exit "narrowed run to a single default-only fixture crate" 0
assert_line_count "cargo metadata lookup + two clippy invocations — pass 3 skipped" "$CARGO_LOG" 3
assert_eq "pass 1 argv has no --features flag" \
    "cargo clippy --locked -p kyomi-auth -- -D warnings" \
    "$(sed -n '2p' "$CARGO_LOG")"
echo

# ─── Test 15: cargo metadata failing exits 3, never 0 and never 1 ──────────
# (KYO-723 FAIL CLOSED) A derivation that could not run must never look like
# a narrowed run with no extra features needed — that would silently
# recreate the exact false dead_code this ticket exists to eliminate. No
# clippy pass may run once the metadata lookup itself has failed.
echo "-- Test 15: cargo metadata failing makes the script exit 3, not 0 or 1"
reset_stubs
export CARGO_FAIL_LINE="cargo metadata --locked --format-version 1"
export CARGO_FAIL_EXIT=1
run_preflight -p kyomi-ui
unset CARGO_FAIL_LINE CARGO_FAIL_EXIT
assert_exit "a derivation that could not run must fail closed" 3
assert_line_count "only the failed metadata call happened — no clippy invocation was ever attempted" "$CARGO_LOG" 1
assert_contains "names cargo metadata as the failure" "cargo metadata"
reset_stubs
echo

# ─── Test 16: -p naming a crate cargo metadata cannot find exits 3 ─────────
# (KYO-723 FAIL CLOSED) A typo'd or nonexistent crate name must not silently
# degrade to "no extra features" — see
# docs/standards/error-handling/empty-on-failure-must-not-look-like-a-real-result.md.
echo "-- Test 16: -p naming a crate absent from cargo metadata output exits 3"
reset_stubs
run_preflight -p this-crate-does-not-exist
assert_exit "an unresolvable crate name must fail closed" 3
assert_line_count "only the metadata call happened — no clippy invocation was ever attempted" "$CARGO_LOG" 1
assert_contains "names the crate that could not be found" "this-crate-does-not-exist"
echo

echo "Results: $PASS passed, $FAIL failed"
[ "$FAIL" -eq 0 ]
