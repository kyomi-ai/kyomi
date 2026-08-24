#!/usr/bin/env bash
# ------------------------------------------------------------------------------
# scripts/mark-worktree-stranded-test.sh — self-test for
# mark-worktree-stranded.sh (KYO-529)
#
# Follows the same pattern as check-ticket-in-flight-test.sh: real throwaway
# git repos and real `git worktree add` linked worktrees under a fresh
# mktemp -d, removed on exit. No stub `gh` needed — this script never calls
# `gh`. Also proves the marker it writes is actually honoured by
# check-ticket-in-flight.sh, since that interoperability is the entire
# point of factoring the format into one script (KYO-422 principle).
#
# Exit 0 = all pass, exit 1 = any failure.
# ------------------------------------------------------------------------------

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
MARK="$SCRIPT_DIR/mark-worktree-stranded.sh"
CHECK="$SCRIPT_DIR/check-ticket-in-flight.sh"
PASS=0
FAIL=0

tmpdir="$(mktemp -d)"
trap 'rm -rf "$tmpdir"' EXIT

export GIT_AUTHOR_NAME="KYO-529 Test" GIT_AUTHOR_EMAIL="kyo529-test@kyomi.invalid"
export GIT_COMMITTER_NAME="KYO-529 Test" GIT_COMMITTER_EMAIL="kyo529-test@kyomi.invalid"

# ─── stub `gh`, so a stray run_check call in this file never touches the ────
# real network — mirrors check-ticket-in-flight-test.sh's approach, even
# though only the interop test below actually invokes $CHECK.
STUB_BIN="$tmpdir/bin"
mkdir -p "$STUB_BIN"
cat >"$STUB_BIN/gh" <<'STUB'
#!/usr/bin/env bash
exit 0
STUB
chmod +x "$STUB_BIN/gh"
export PATH="$STUB_BIN:$PATH"

pass() {
    printf "  \xe2\x9c\x93 %s\n" "$1"
    PASS=$((PASS + 1))
}

fail() {
    printf "  \xe2\x9c\x97 %s\n" "$1"
    printf '    %s\n' "$2" | sed 's/^/    | /'
    FAIL=$((FAIL + 1))
}

# ─── invocation helper, capturing exit code + combined output ───────────────
RUN_STATUS=""
RUN_OUTPUT=""
run_mark() {
    # run_mark <arg>...  — invokes mark-worktree-stranded.sh directly (it
    # takes an explicit --worktree, so unlike check-ticket-in-flight.sh's
    # run_check it does not need a cwd to be meaningful).
    local out
    if out="$("$MARK" "$@" 2>&1)"; then
        RUN_STATUS=0
    else
        RUN_STATUS=$?
    fi
    RUN_OUTPUT="$out"
}

assert_exit() {
    local name="$1" expected="$2"
    if [ "$RUN_STATUS" -eq "$expected" ]; then
        pass "$name (exit $RUN_STATUS)"
    else
        fail "$name — expected exit $expected, got $RUN_STATUS" "$RUN_OUTPUT"
    fi
}

assert_contains() {
    local name="$1" needle="$2"
    if printf '%s' "$RUN_OUTPUT" | grep -qF -- "$needle"; then
        pass "$name"
    else
        fail "$name — expected output to contain: $needle" "$RUN_OUTPUT"
    fi
}

# ─── real git repo helpers (mirrors check-ticket-in-flight-test.sh) ─────────
new_repo_with_commit() {
    # new_repo_with_commit <dir> -> a real repo on branch main with one commit.
    local dir="$1"
    git init -q -b main "$dir"
    echo "seed" >"$dir/README.md"
    git -C "$dir" add README.md
    git -C "$dir" commit -q -m init
}

new_repo_with_remote() {
    # new_repo_with_remote <bare_dir> <clone_dir> -> a real bare remote plus
    # a clone with `origin` configured and one commit already pushed to
    # main. Needed only for the interop test, where the repo under test has
    # to satisfy check-ticket-in-flight.sh's own checks 1/2 (which require a
    # working `origin`) as well as this script's checks.
    local bare="$1" clone="$2" seed
    git init --bare -q "$bare"
    git -C "$bare" symbolic-ref HEAD refs/heads/main
    seed="$(mktemp -d)"
    git init -q -b main "$seed"
    echo "seed" >"$seed/README.md"
    git -C "$seed" add README.md
    git -C "$seed" commit -q -m init
    git -C "$seed" remote add origin "$bare"
    git -C "$seed" push -q origin main
    rm -rf "$seed"
    git clone -q "$bare" "$clone"
}

echo "Running mark-worktree-stranded self-tests..."
echo

# ─── Test 1: happy path — writes a marker check-ticket-in-flight.sh honours ─
echo "-- Test 1: happy path + interop with check-ticket-in-flight.sh"
t1="$tmpdir/t1"
mkdir -p "$t1"
new_repo_with_remote "$t1/remote.git" "$t1/primary"
git -C "$t1/primary" worktree add -q -b jason/kyo-529-dead "$t1/dead"
run_mark 529 --worktree "$t1/dead" --note "died mid-run, salvage the WIP diff"
assert_exit "writes the marker" 0
assert_contains "confirms the write" "Wrote tombstone for KYO-529"
if [ -f "$t1/dead/STRANDED.md" ]; then
    pass "STRANDED.md exists at the worktree root"
else
    fail "STRANDED.md exists at the worktree root" "not found at $t1/dead/STRANDED.md"
fi
content="$(cat "$t1/dead/STRANDED.md" 2>/dev/null || echo MISSING)"
case "$content" in
    *"KYO-529"*) pass "marker names the ticket" ;;
    *) fail "marker names the ticket" "$content" ;;
esac
case "$content" in
    *"$t1/dead"*) pass "marker records the worktree path" ;;
    *) fail "marker records the worktree path" "$content" ;;
esac
case "$content" in
    *"jason/kyo-529-dead"*) pass "marker records the branch" ;;
    *) fail "marker records the branch" "$content" ;;
esac
case "$content" in
    *"died mid-run, salvage the WIP diff"*) pass "marker includes the --note text" ;;
    *) fail "marker includes the --note text" "$content" ;;
esac
case "$content" in
    *"delete this file"*) pass "marker tells a human adopter to delete it first" ;;
    *) fail "marker tells a human adopter to delete it first" "$content" ;;
esac
# Prove interoperability: the script that actually reads STRANDED.md must
# treat this worktree as tombstoned, not as a hit.
if out="$(cd "$t1/primary" && "$CHECK" 529 2>&1)"; then
    check_status=0
else
    check_status=$?
fi
if [ "$check_status" -eq 0 ] && printf '%s' "$out" | grep -qF "PRESERVED STRANDED WORKTREES"; then
    pass "check-ticket-in-flight.sh honours the marker this script wrote"
else
    fail "check-ticket-in-flight.sh honours the marker this script wrote" "exit=$check_status; $out"
fi
echo

# ─── Test 2: refuses the primary worktree ────────────────────────────────────
echo "-- Test 2: refuses the primary worktree"
t2="$tmpdir/t2"
mkdir -p "$t2"
new_repo_with_commit "$t2/primary"
run_mark 529 --worktree "$t2/primary"
assert_exit "refuses to tombstone the primary worktree" 1
assert_contains "explains why" "refusing to tombstone the primary worktree"
if [ -f "$t2/primary/STRANDED.md" ]; then
    fail "no marker written to the primary worktree" "STRANDED.md exists at $t2/primary"
else
    pass "no marker written to the primary worktree"
fi
echo

# ─── Test 3: refuses branch main ─────────────────────────────────────────────
echo "-- Test 3: refuses branch main"
t3="$tmpdir/t3"
mkdir -p "$t3"
git init -q -b trunk "$t3/primary"
echo "seed" >"$t3/primary/README.md"
git -C "$t3/primary" add README.md
git -C "$t3/primary" commit -q -m init
git -C "$t3/primary" branch main
git -C "$t3/primary" worktree add -q "$t3/wt-main" main
run_mark 529 --worktree "$t3/wt-main"
assert_exit "refuses a worktree checked out on main" 1
assert_contains "explains why" "refusing to tombstone a worktree on branch 'main'"
echo

# ─── Test 4: refuses detached HEAD ───────────────────────────────────────────
echo "-- Test 4: refuses detached HEAD"
t4="$tmpdir/t4"
mkdir -p "$t4"
new_repo_with_commit "$t4/primary"
git -C "$t4/primary" worktree add -q --detach "$t4/wt-detached"
run_mark 529 --worktree "$t4/wt-detached"
assert_exit "refuses a detached-HEAD worktree" 1
assert_contains "explains why" "detached HEAD state"
echo

# ─── Test 5: usage errors ────────────────────────────────────────────────────
echo "-- Test 5: usage errors"
run_mark
assert_exit "no argument" 2
run_mark "not-a-ticket"
assert_exit "unparseable ticket" 2
t5="$tmpdir/t5"
mkdir -p "$t5"
new_repo_with_commit "$t5/primary"
git -C "$t5/primary" worktree add -q -b jason/kyo-529-usage "$t5/wt"
run_mark 529 --worktree
assert_exit "--worktree missing its value" 2
run_mark 529 --worktree "$t5/wt" --note
assert_exit "--note missing its value" 2
run_mark 529 --bogus-flag
assert_exit "unknown flag" 2
run_mark 529 --worktree "$t5/does-not-exist"
assert_exit "--worktree path does not exist" 1
echo

# ─── Test 6: input forms — KYO-529 / kyo-529 / 529 all equivalent ──────────
echo "-- Test 6: input forms"
t6="$tmpdir/t6"
mkdir -p "$t6"
new_repo_with_commit "$t6/primary"
i=0
for form in KYO-529 kyo-529 529; do
    i=$((i + 1))
    git -C "$t6/primary" worktree add -q -b "jason/kyo-529-form-$i" "$t6/wt-$i"
    run_mark "$form" --worktree "$t6/wt-$i"
    assert_exit "input form '$form' writes successfully" 0
    content="$(cat "$t6/wt-$i/STRANDED.md" 2>/dev/null || echo MISSING)"
    case "$content" in
        *"KYO-529"*) pass "input form '$form' normalizes to KYO-529 in the marker" ;;
        *) fail "input form '$form' normalizes to KYO-529 in the marker" "$content" ;;
    esac
done
echo

# ─── Test 7: overwriting an existing marker is idempotent, not an error ────
echo "-- Test 7: overwrite is idempotent"
t7="$tmpdir/t7"
mkdir -p "$t7"
new_repo_with_commit "$t7/primary"
git -C "$t7/primary" worktree add -q -b jason/kyo-529-overwrite "$t7/wt"
run_mark 529 --worktree "$t7/wt" --note "first release"
assert_exit "first write succeeds" 0
run_mark 529 --worktree "$t7/wt" --note "second release"
assert_exit "second write over an existing marker still succeeds" 0
assert_contains "warns on stderr that it overwrote" "overwriting existing marker"
content="$(cat "$t7/wt/STRANDED.md" 2>/dev/null || echo MISSING)"
case "$content" in
    *"second release"*) pass "the marker reflects the latest write" ;;
    *) fail "the marker reflects the latest write" "$content" ;;
esac
echo

echo "Results: $PASS passed, $FAIL failed"
[ "$FAIL" -eq 0 ]
