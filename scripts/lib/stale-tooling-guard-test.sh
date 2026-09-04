#!/usr/bin/env bash
# ------------------------------------------------------------------------------
# scripts/lib/stale-tooling-guard-test.sh — self-test for
# stale-tooling-guard.sh (KYO-632)
#
# Follows the same pattern as scripts/check-ticket-in-flight-test.sh: real
# throwaway git repos, real bare remotes, real `git clone`, all under a
# fresh mktemp -d that is removed on exit. Never touches the real kyomi
# repo, its real origin, or any network.
#
# Every fixture repo copies the REAL scripts/lib/stale-tooling-guard.sh
# under test into place (see $GUARD_SRC / write_guard_and_fixture below) —
# never a re-implementation of its logic — so a change to the guard is
# exercised by the exact same file this suite asserts against
# (docs/standards/testing/mutate-by-relocating-real-code.md).
#
# Each fixture repo also gets a tiny stand-in "guarded script"
# (scripts/fixture-guarded.sh) instead of a copy of one of the seven real
# guarded scripts: it sources the guard, calls it on its own
# `${BASH_SOURCE[0]}`, prints a marker ("REAL WORK RAN") and exits with a
# caller-chosen code. That marker plus that exit code are what let a test
# tell "the guard ran and warned" apart from "the guard's STRICT escalation
# stopped the real work from running at all" — the single property this
# whole ticket is about (a guard must never be the reason the caller's real
# work does not run, except via the one deliberate, documented STRICT path).
#
# KNOWN DUPLICATION, DEFERRED RATHER THAN FIXED HERE: the git-repo bootstrap
# helpers below (new_bare_remote / init_and_push / clone_repo) are a fourth
# near-identical copy of the same shape already in
# check-ticket-in-flight-test.sh, mark-worktree-stranded-test.sh, and
# mark-branch-stranded-test.sh —
# docs/standards/code-organization/third-copy-of-test-helper-is-extraction-trigger.md
# says a third copy is the trigger to extract, and this is a fourth.
# Extraction was NOT done in this change: KYO-632's own scope is limited to
# scripts/lib/, the seven guarded scripts, and .github/workflows/ci.yml (see
# the ticket), and a real extraction has to edit those three existing test
# files too — out of bounds here, and not a regression this diff
# introduces (the duplication predates it). Filed as its own ticket instead
# of silently repeating the same "it's not worth it" reasoning inline (see
# docs/standards/version-control-working-tree/a-deferral-ticket-is-not-always-enough.md
# on why a deferral still needs an honest cost/provenance case): KYO-639.
#
# Exit 0 = all pass, exit 1 = any failure.
# ------------------------------------------------------------------------------

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
GUARD_SRC="$SCRIPT_DIR/stale-tooling-guard.sh"
PASS=0
FAIL=0

tmpdir="$(mktemp -d)"
trap 'rm -rf "$tmpdir"' EXIT

export GIT_AUTHOR_NAME="KYO-632 Test" GIT_AUTHOR_EMAIL="kyo632-test@kyomi.invalid"
export GIT_COMMITTER_NAME="KYO-632 Test" GIT_COMMITTER_EMAIL="kyo632-test@kyomi.invalid"

# ─── assertion helpers ───────────────────────────────────────────────────────
pass() {
    printf "  \xe2\x9c\x93 %s\n" "$1"
    PASS=$((PASS + 1))
}

fail() {
    printf "  \xe2\x9c\x97 %s\n" "$1"
    if [ "$#" -ge 2 ]; then
        printf '%s\n' "$2" | sed 's/^/    | /'
    fi
    FAIL=$((FAIL + 1))
}

assert_exit() {
    local name="$1" expected="$2" actual="$3" output="$4"
    if [ "$actual" -eq "$expected" ]; then
        pass "$name (exit $actual)"
    else
        fail "$name — expected exit $expected, got $actual" "$output"
    fi
}

assert_contains() {
    local name="$1" needle="$2" haystack="$3"
    if printf '%s' "$haystack" | grep -qF -- "$needle"; then
        pass "$name"
    else
        fail "$name — expected output to contain: $needle" "$haystack"
    fi
}

assert_not_contains() {
    local name="$1" needle="$2" haystack="$3"
    if printf '%s' "$haystack" | grep -qF -- "$needle"; then
        fail "$name — expected output NOT to contain: $needle" "$haystack"
    else
        pass "$name"
    fi
}

# ─── real git-repo helpers (see KNOWN DUPLICATION note above) ───────────────
new_bare_remote() {
    # new_bare_remote <dir> -> echoes <dir>. Pre-points HEAD at
    # refs/heads/main so a later clone checks out "main" the moment it
    # exists, without depending on this machine's init.defaultBranch
    # (ubuntu-latest's is "master" — pinned explicitly per KYO-632's own
    # test-writing instructions).
    local dir="$1"
    git init --bare -q "$dir"
    git -C "$dir" symbolic-ref HEAD refs/heads/main
    echo "$dir"
}

init_and_push() {
    # init_and_push <working-dir> <bare-remote> — commits everything
    # currently in <working-dir> to a fresh "main" branch and pushes it, so
    # that once cloned, origin/main is exactly this content.
    local dir="$1" bare="$2"
    git -C "$dir" init -q
    git -C "$dir" checkout -q -b main
    git -C "$dir" add -A
    git -C "$dir" commit -q -m fixture
    git -C "$dir" remote add origin "$bare"
    git -C "$dir" push -q origin main
}

clone_repo() { git clone -q "$1" "$2"; } # clone_repo <bare> <dest>

# write_guard_and_fixture <dir> <exit-code> — populates <dir>/scripts/lib/
# with the REAL guard under test and <dir>/scripts/fixture-guarded.sh, a
# minimal stand-in for one of the seven real guarded scripts.
write_guard_and_fixture() {
    local dir="$1" exit_code="$2"
    mkdir -p "$dir/scripts/lib"
    cp "$GUARD_SRC" "$dir/scripts/lib/stale-tooling-guard.sh"
    cat >"$dir/scripts/fixture-guarded.sh" <<FIXTURE
#!/usr/bin/env bash
set -euo pipefail
SCRIPT_DIR="\$(cd "\$(dirname "\${BASH_SOURCE[0]}")" && pwd)"
source "\${SCRIPT_DIR}/lib/stale-tooling-guard.sh"
stale_tooling_guard "\${BASH_SOURCE[0]}"
echo "REAL WORK RAN"
exit ${exit_code}
FIXTURE
    chmod +x "$dir/scripts/fixture-guarded.sh"
}

# ─── invocation helper, capturing exit code + combined output ───────────────
RUN_STATUS=""
RUN_OUTPUT=""
run_fixture() {
    # run_fixture <dir> [env assignment]...
    local dir="$1" out
    shift
    if out="$(cd "$dir" && env "$@" bash scripts/fixture-guarded.sh 2>&1)"; then
        RUN_STATUS=0
    else
        RUN_STATUS=$?
    fi
    RUN_OUTPUT="$out"
}

echo "Running stale-tooling-guard self-tests..."
echo

# ─── Test 1: content matches origin/main → silent, real work runs ──────────
echo "-- Test 1: matches origin/main"
t1="$tmpdir/t1"
mkdir -p "$t1/seed"
write_guard_and_fixture "$t1/seed" 7
bare1="$(new_bare_remote "$t1/remote.git")"
init_and_push "$t1/seed" "$bare1"
clone_repo "$bare1" "$t1/worker"
run_fixture "$t1/worker"
assert_exit "clean checkout: host exit code unaffected" 7 "$RUN_STATUS" "$RUN_OUTPUT"
assert_contains "clean checkout: real work still runs" "REAL WORK RAN" "$RUN_OUTPUT"
assert_not_contains "clean checkout: no staleness output at all" "STALE TOOLING WARNING" "$RUN_OUTPUT"
assert_not_contains "clean checkout: no cannot-determine output either" "could not determine" "$RUN_OUTPUT"
echo

# ─── Test 2: THE MUST-NOT-REGRESS CASE — content differs, loud but ─────────
# non-fatal by default.
echo "-- Test 2: differs from origin/main (default warn mode)"
t2="$tmpdir/t2"
mkdir -p "$t2/seed"
write_guard_and_fixture "$t2/seed" 7
bare2="$(new_bare_remote "$t2/remote.git")"
init_and_push "$t2/seed" "$bare2"
clone_repo "$bare2" "$t2/worker"
# Diverge the WORKING COPY from what was pushed, without committing —
# origin/main (as this clone knows it) still points at the pushed content.
echo '# local-only edit, never pushed' >>"$t2/worker/scripts/fixture-guarded.sh"
run_fixture "$t2/worker"
assert_exit "stale by default: host's own exit code is unaffected" 7 "$RUN_STATUS" "$RUN_OUTPUT"
assert_contains "stale by default: real work still runs" "REAL WORK RAN" "$RUN_OUTPUT"
assert_contains "stale by default: fires the loud banner" "STALE TOOLING WARNING" "$RUN_OUTPUT"
assert_contains "stale by default: names the file" "scripts/fixture-guarded.sh" "$RUN_OUTPUT"
assert_contains "stale by default: names both possible directions, not just one" "BEHIND origin/main" "$RUN_OUTPUT"
assert_contains "stale by default: names both possible directions, not just one" "UNPUSHED local edits" "$RUN_OUTPUT"
echo

# ─── Test 3: a change to a DIFFERENT file must not false-positive ──────────
# — the check is content-of-this-file, not commit-ancestry.
echo "-- Test 3: local commits ahead, but this file is untouched"
t3="$tmpdir/t3"
mkdir -p "$t3/seed"
write_guard_and_fixture "$t3/seed" 7
echo "seed readme" >"$t3/seed/README.md"
bare3="$(new_bare_remote "$t3/remote.git")"
init_and_push "$t3/seed" "$bare3"
clone_repo "$bare3" "$t3/worker"
echo "local-only change to an unrelated file" >>"$t3/worker/README.md"
git -C "$t3/worker" commit -q -am "local: unrelated change"
run_fixture "$t3/worker"
assert_exit "unrelated local commit: host exit code unaffected" 7 "$RUN_STATUS" "$RUN_OUTPUT"
assert_not_contains "unrelated local commit: no false-positive staleness warning" "STALE TOOLING WARNING" "$RUN_OUTPUT"
echo

# ─── Test 4: cannot determine — not inside a git repository at all ─────────
echo "-- Test 4: cannot determine (no git repo)"
t4="$tmpdir/t4/plain"
write_guard_and_fixture "$t4" 7
run_fixture "$t4"
assert_exit "no git repo: host exit code unaffected" 7 "$RUN_STATUS" "$RUN_OUTPUT"
assert_contains "no git repo: real work still runs" "REAL WORK RAN" "$RUN_OUTPUT"
assert_contains "no git repo: loud, not silent" "could not determine" "$RUN_OUTPUT"
assert_not_contains "no git repo: never claims a confirmed mismatch it can't back up" "STALE TOOLING WARNING" "$RUN_OUTPUT"
echo

# ─── Test 5: cannot determine — no local origin/main ref ───────────────────
echo "-- Test 5: cannot determine (no origin/main ref — never fetched)"
t5="$tmpdir/t5"
mkdir -p "$t5"
write_guard_and_fixture "$t5" 7
git -C "$t5" init -q
git -C "$t5" checkout -q -b main
git -C "$t5" add -A
git -C "$t5" commit -q -m fixture
run_fixture "$t5"
assert_exit "no origin/main ref: host exit code unaffected" 7 "$RUN_STATUS" "$RUN_OUTPUT"
assert_contains "no origin/main ref: real work still runs" "REAL WORK RAN" "$RUN_OUTPUT"
assert_contains "no origin/main ref: loud, not silent" "could not determine" "$RUN_OUTPUT"
assert_contains "no origin/main ref: tells the operator how to fix it" "git fetch origin main" "$RUN_OUTPUT"
echo

# ─── Test 6: cannot determine — path does not exist at origin/main ─────────
echo "-- Test 6: cannot determine (path missing at origin/main)"
t6="$tmpdir/t6"
mkdir -p "$t6/seed/scripts/lib"
cp "$GUARD_SRC" "$t6/seed/scripts/lib/stale-tooling-guard.sh"
echo "no fixture-guarded.sh here yet" >"$t6/seed/README.md"
bare6="$(new_bare_remote "$t6/remote.git")"
init_and_push "$t6/seed" "$bare6"
clone_repo "$bare6" "$t6/worker"
# The fixture script exists only LOCALLY, never pushed.
cat >"$t6/worker/scripts/fixture-guarded.sh" <<'FIXTURE'
#!/usr/bin/env bash
set -euo pipefail
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
source "${SCRIPT_DIR}/lib/stale-tooling-guard.sh"
stale_tooling_guard "${BASH_SOURCE[0]}"
echo "REAL WORK RAN"
exit 7
FIXTURE
chmod +x "$t6/worker/scripts/fixture-guarded.sh"
run_fixture "$t6/worker"
assert_exit "path missing upstream: host exit code unaffected" 7 "$RUN_STATUS" "$RUN_OUTPUT"
assert_contains "path missing upstream: real work still runs" "REAL WORK RAN" "$RUN_OUTPUT"
assert_contains "path missing upstream: loud, not silent" "could not determine" "$RUN_OUTPUT"
echo

# ─── Test 7: KYOMI_STALE_TOOLING_STRICT=1 escalates a CONFIRMED mismatch ───
echo "-- Test 7: STRICT=1 escalates a confirmed mismatch"
t7="$tmpdir/t7"
mkdir -p "$t7/seed"
write_guard_and_fixture "$t7/seed" 7
bare7="$(new_bare_remote "$t7/remote.git")"
init_and_push "$t7/seed" "$bare7"
clone_repo "$bare7" "$t7/worker"
echo '# local-only edit, never pushed' >>"$t7/worker/scripts/fixture-guarded.sh"
run_fixture "$t7/worker" KYOMI_STALE_TOOLING_STRICT=1
assert_exit "STRICT=1 + confirmed mismatch: exits the guard's own code, not the host's" 42 "$RUN_STATUS" "$RUN_OUTPUT"
assert_contains "STRICT=1 + confirmed mismatch: still prints the loud banner" "STALE TOOLING WARNING" "$RUN_OUTPUT"
assert_not_contains "STRICT=1 + confirmed mismatch: real work must NOT run" "REAL WORK RAN" "$RUN_OUTPUT"
echo

# ─── Test 8: STRICT=1 must NOT escalate a "cannot determine" result ────────
# — this is the load-bearing split documented in the guard's own header
# (A SCRIPT MUST NEVER BE BROKEN BY ITS OWN GUARD): only a CONFIRMED
# mismatch is eligible for escalation, never the guard's own inability to
# check.
echo "-- Test 8: STRICT=1 does not escalate a cannot-determine result"
t8="$tmpdir/t8"
mkdir -p "$t8"
write_guard_and_fixture "$t8" 7
git -C "$t8" init -q
git -C "$t8" checkout -q -b main
git -C "$t8" add -A
git -C "$t8" commit -q -m fixture
run_fixture "$t8" KYOMI_STALE_TOOLING_STRICT=1
assert_exit "STRICT=1 + cannot-determine: host exit code still unaffected" 7 "$RUN_STATUS" "$RUN_OUTPUT"
assert_contains "STRICT=1 + cannot-determine: real work still runs" "REAL WORK RAN" "$RUN_OUTPUT"
assert_contains "STRICT=1 + cannot-determine: still loud" "could not determine" "$RUN_OUTPUT"
echo

# ─── Test 9: STRICT=1 on a clean match stays completely silent ─────────────
echo "-- Test 9: STRICT=1 with a clean match changes nothing"
t9="$tmpdir/t9"
mkdir -p "$t9/seed"
write_guard_and_fixture "$t9/seed" 7
bare9="$(new_bare_remote "$t9/remote.git")"
init_and_push "$t9/seed" "$bare9"
clone_repo "$bare9" "$t9/worker"
run_fixture "$t9/worker" KYOMI_STALE_TOOLING_STRICT=1
assert_exit "STRICT=1 + clean match: host exit code unaffected" 7 "$RUN_STATUS" "$RUN_OUTPUT"
assert_not_contains "STRICT=1 + clean match: no staleness output" "STALE TOOLING WARNING" "$RUN_OUTPUT"
echo

# ─── Test 10: only the exact value "1" escalates — anything else warns ────
echo "-- Test 10: only KYOMI_STALE_TOOLING_STRICT=1 escalates"
t10="$tmpdir/t10"
mkdir -p "$t10/seed"
write_guard_and_fixture "$t10/seed" 7
bare10="$(new_bare_remote "$t10/remote.git")"
init_and_push "$t10/seed" "$bare10"
clone_repo "$bare10" "$t10/worker"
echo '# local-only edit, never pushed' >>"$t10/worker/scripts/fixture-guarded.sh"
run_fixture "$t10/worker" KYOMI_STALE_TOOLING_STRICT=0
assert_exit "STRICT=0 behaves like unset (warn only)" 7 "$RUN_STATUS" "$RUN_OUTPUT"
assert_contains "STRICT=0 still warns" "STALE TOOLING WARNING" "$RUN_OUTPUT"
run_fixture "$t10/worker" KYOMI_STALE_TOOLING_STRICT=true
assert_exit "STRICT=true (not the literal '1') behaves like unset (warn only)" 7 "$RUN_STATUS" "$RUN_OUTPUT"
assert_contains "STRICT=true still warns" "STALE TOOLING WARNING" "$RUN_OUTPUT"
echo

echo "Results: $PASS passed, $FAIL failed"
[ "$FAIL" -eq 0 ]
