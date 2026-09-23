#!/usr/bin/env bash
# ------------------------------------------------------------------------------
# scripts/retire-worktree-test.sh — self-test for retire-worktree.sh (KYO-733)
#
# Follows the same pattern as check-ticket-in-flight-test.sh and
# mark-worktree-stranded-test.sh: real throwaway git repos, a real bare
# `origin`, and real `git worktree add` linked worktrees, all built under a
# fresh mktemp -d and removed on exit via `trap ... EXIT`. No `gh` is
# involved (retire-worktree.sh never calls it), so no stub is needed.
# Fixture branch names are pinned explicitly (`git init -q -b main`) rather
# than relying on the runner's `init.defaultBranch`, which is `master` on
# ubuntu-latest.
#
# A NOTE ON "AGED" FIXTURES. `git worktree add` checks out every tracked
# file with an mtime of "now", so a fixture built and immediately tested
# would trip the recent-write check on its own .gitignore/README.md, not on
# whatever the test actually wants to exercise. Every fixture below is aged
# with `touch -d '2 hours ago'` (well outside LIVENESS_WINDOW_MINUTES)
# immediately after `git worktree add`, before the test makes its own,
# deliberately fresh, edit. The exclusion pattern is
# `\( -path "$1/.git" -o -path "$1/.git/*" \) -prune`, NOT a
# `-not -path "*/.git*"` glob — the latter also matches `.gitignore`
# (`/.git` + `ignore` satisfies `*/.git*`), which was caught live while
# writing this suite: an early draft aged every file except `.gitignore`,
# which then perpetually read as "just modified" and made the
# target/-writes-do-not-count case impossible to construct.
#
# MUTATION TESTING (see "Mutation check A" and "Mutation check B" below).
# Two of retire-worktree.sh's assertions are the ones a regression would be
# most dangerous to lose silently — the find-failure fail-closed behaviour
# (the exact KYO-511 shape: an unmutated cutoff-window check that discards
# find's exit status reads a failure as "no recent files") and the
# live-tree refusal (the entire KYO-733 incident this script exists to
# prevent). Both are mutated with `sed -i`, but NEVER on the tracked
# scripts/retire-worktree.sh itself — each mutation lands on a throwaway
# `cp` living under $tmpdir (see MUTATION_SCRATCH_DIR below), so the real,
# on-disk script is never touched. A `diff` against the pristine script
# proves each `sed` actually changed something before the mutated copy is
# invoked; a `cmp` of the real script against a snapshot taken once at the
# very top of this file, and checked once more at the very end (see
# "Suite-level guard" below), is this suite's own proof that it stayed
# untouched throughout. An earlier version of this suite instead mutated
# scripts/retire-worktree.sh's own bytes in place and restored them from a
# `cp` backup via an EXIT trap — the trap never ran on SIGKILL, CI
# cancellation, or Ctrl-C inside that window, which left the real gate on
# disk with its live-tree refusal disabled: a live reproduction of the
# KYO-733 incident this script exists to prevent, for however long the
# window lasted, and for any concurrent agent that ran the real script
# during it. Mutating only a scratch copy removes the window entirely —
# there is nothing on the real file to restore. (`cp`/`diff`/`cmp` here,
# never `git stash` or `git checkout --`, which can lose more than the
# mutation — see docs/standards/testing/no-git-stash-copy-file-instead.md.)
#
# Exit 0 = all pass, exit 1 = any failure.
# ------------------------------------------------------------------------------

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
SCRIPT="$SCRIPT_DIR/retire-worktree.sh"
PASS=0
FAIL=0

tmpdir="$(mktemp -d)"
declare -a BG_PIDS=()
cleanup() {
    local pid
    for pid in "${BG_PIDS[@]:-}"; do
        [ -n "$pid" ] && kill "$pid" >/dev/null 2>&1 || true
    done
    for pid in "${BG_PIDS[@]:-}"; do
        [ -n "$pid" ] && wait "$pid" 2>/dev/null || true
    done
    rm -rf "$tmpdir"
}
trap cleanup EXIT

# production-snapshot guard — taken here, at the very start, and checked
# once more at the very end (search "Suite-level guard" below). Nothing in
# this suite is meant to touch scripts/retire-worktree.sh's own on-disk
# bytes any more (see MUTATION TESTING above); this is the suite's own
# proof that held, not just a comment asserting it.
PRODUCTION_SNAPSHOT="$tmpdir/retire-worktree.sh.production-snapshot"
cp "$SCRIPT" "$PRODUCTION_SNAPSHOT"

export GIT_AUTHOR_NAME="KYO-733 Test" GIT_AUTHOR_EMAIL="kyo733-test@kyomi.invalid"
export GIT_COMMITTER_NAME="KYO-733 Test" GIT_COMMITTER_EMAIL="kyo733-test@kyomi.invalid"

pass() {
    printf "  \xe2\x9c\x93 %s\n" "$1"
    PASS=$((PASS + 1))
}

fail() {
    printf "  \xe2\x9c\x97 %s\n" "$1"
    printf '    %s\n' "$2" | sed 's/^/    | /'
    FAIL=$((FAIL + 1))
}

# ─── fixture helpers ─────────────────────────────────────────────────────────

# new_repo_with_origin <case_dir> -> populates <case_dir>/origin.git (bare)
# and <case_dir>/primary (a real clone on branch main, one commit, a
# committed .gitignore ignoring `target/`, already pushed to origin).
new_repo_with_origin() {
    local dir="$1"
    mkdir -p "$dir"
    git init -q --bare "$dir/origin.git"
    git -C "$dir/origin.git" symbolic-ref HEAD refs/heads/main
    git init -q -b main "$dir/primary"
    echo "target/" >"$dir/primary/.gitignore"
    git -C "$dir/primary" add .gitignore
    git -C "$dir/primary" commit -q -m init
    git -C "$dir/primary" remote add origin "$dir/origin.git"
    git -C "$dir/primary" push -q origin main
}

# age_tree <path> — set every tracked/working file's mtime well outside the
# liveness window, EXCLUDING .git itself (a FILE, not a directory, in a
# linked worktree — see header for why a naive `-not -path "*/.git*"` is
# wrong here).
age_tree() {
    find "$1" \( -path "$1/.git" -o -path "$1/.git/*" \) -prune -o -exec touch -d '2 hours ago' {} + 2>/dev/null || true
}

# add_worktree <case_dir> <branch> <name> -> creates <case_dir>/<name> as a
# linked worktree of <case_dir>/primary on a new branch, ages it, and pushes
# the branch to origin so it starts life "clean and fully pushed". Callers
# add their own deviation (a dirty file, an unpushed commit, ...) after.
add_worktree() {
    local case_dir="$1" branch="$2" name="$3"
    git -C "$case_dir/primary" worktree add -q -b "$branch" "$case_dir/$name" main
    age_tree "$case_dir/$name"
    git -C "$case_dir/$name" push -q origin "$branch"
}

# ─── invocation helper ───────────────────────────────────────────────────────
RUN_STATUS=""
RUN_OUTPUT=""

# run_script <script-path> [args...] — shared by run_retire (below, always
# the real $SCRIPT) and the mutation checks near the bottom (always a
# throwaway scratch copy), so both paths capture status/output identically.
run_script() {
    local script="$1"
    shift
    local out
    if out="$("$script" "$@" 2>&1)"; then
        RUN_STATUS=0
    else
        RUN_STATUS=$?
    fi
    RUN_OUTPUT="$out"
}

run_retire() {
    run_script "$SCRIPT" "$@"
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

echo "Running retire-worktree.sh self-tests..."
echo

# ─── Test 1: a live tree (recent write) is refused and stays present ────────
echo "-- Test 1: live tree (recent write) refused, stays present"
t1="$tmpdir/t1"
new_repo_with_origin "$t1"
add_worktree "$t1" "jason/kyo-733-live" "live-wt"
echo "wip" >"$t1/live-wt/newfile.txt"
run_retire "$t1/live-wt"
assert_exit "refused" 1
assert_contains "names the recent-write reason" "recent write(s):"
if [ -d "$t1/live-wt" ]; then
    pass "the tree was not removed"
else
    fail "the tree was not removed" "directory is gone: $t1/live-wt"
fi
echo

# ─── Test 1b: recent write in ISOLATION (mtime bump only, no content change,
# so git status stays clean) — used below by mutation check B so that check
# is not confounded by the untracked-file's OWN uncommitted-work hit that
# Test 1's more realistic fixture also (correctly) reports. ─────────────────
echo "-- Test 1b: recent write in isolation (tracked file, mtime-only touch)"
t1b="$tmpdir/t1b"
new_repo_with_origin "$t1b"
add_worktree "$t1b" "jason/kyo-733-recentonly" "recentonly-wt"
touch "$t1b/recentonly-wt/.gitignore"
run_retire "$t1b/recentonly-wt"
assert_exit "refused" 1
assert_contains "names the recent-write reason" "recent write(s):"
if printf '%s' "$RUN_OUTPUT" | grep -qF "uncommitted work:"; then
    fail "no confounding uncommitted-work reason" "$RUN_OUTPUT"
else
    pass "no confounding uncommitted-work reason (isolates the recent-write signal)"
fi
if [ -d "$t1b/recentonly-wt" ]; then
    pass "the tree was not removed"
else
    fail "the tree was not removed" "directory is gone: $t1b/recentonly-wt"
fi
echo

# ─── Test 2: a quiet, clean, fully-pushed tree is removed ───────────────────
echo "-- Test 2: quiet clean fully-pushed tree removed"
t2="$tmpdir/t2"
new_repo_with_origin "$t2"
add_worktree "$t2" "jason/kyo-733-clean" "clean-wt"
run_retire "$t2/clean-wt"
assert_exit "removed" 0
assert_contains "confirms removal" "Removed worktree:"
if [ -d "$t2/clean-wt" ]; then
    fail "the tree is actually gone" "directory still present: $t2/clean-wt"
else
    pass "the tree is actually gone"
fi
echo

# ─── Test 3: unpushed commits refused, naming the commit ────────────────────
echo "-- Test 3: unpushed-commit tree refused, names the commit"
t3="$tmpdir/t3"
new_repo_with_origin "$t3"
add_worktree "$t3" "jason/kyo-733-unpushed" "unpushed-wt"
echo "extra" >"$t3/unpushed-wt/extra.txt"
git -C "$t3/unpushed-wt" add extra.txt
git -C "$t3/unpushed-wt" commit -q -m "unpushed local change"
age_tree "$t3/unpushed-wt"
run_retire "$t3/unpushed-wt"
assert_exit "refused" 1
assert_contains "names the unpushed reason" "unpushed commit(s) on HEAD"
assert_contains "names the commit subject" "unpushed local change"
if [ -d "$t3/unpushed-wt" ]; then
    pass "the tree was not removed"
else
    fail "the tree was not removed" "directory is gone: $t3/unpushed-wt"
fi
echo

# ─── Test 4: uncommitted changes refused ─────────────────────────────────────
echo "-- Test 4: uncommitted-changes tree refused"
t4="$tmpdir/t4"
new_repo_with_origin "$t4"
add_worktree "$t4" "jason/kyo-733-dirty" "dirty-wt"
echo "dirty" >"$t4/dirty-wt/README.md"
age_tree "$t4/dirty-wt"
run_retire "$t4/dirty-wt"
assert_exit "refused" 1
assert_contains "names the uncommitted-work reason" "uncommitted work:"
if [ -d "$t4/dirty-wt" ]; then
    pass "the tree was not removed"
else
    fail "the tree was not removed" "directory is gone: $t4/dirty-wt"
fi
echo

# ─── Test 5: STRANDED.md tombstone refused ───────────────────────────────────
echo "-- Test 5: STRANDED.md tree refused"
t5="$tmpdir/t5"
new_repo_with_origin "$t5"
add_worktree "$t5" "jason/kyo-733-stranded" "stranded-wt"
echo "# STRANDED" >"$t5/stranded-wt/STRANDED.md"
git -C "$t5/stranded-wt" add STRANDED.md
git -C "$t5/stranded-wt" commit -q -m "tombstone"
git -C "$t5/stranded-wt" push -q origin jason/kyo-733-stranded
age_tree "$t5/stranded-wt"
run_retire "$t5/stranded-wt"
assert_exit "refused" 1
assert_contains "names the STRANDED.md reason" "STRANDED.md tombstone present"
if [ -d "$t5/stranded-wt" ]; then
    pass "the tree was not removed"
else
    fail "the tree was not removed" "directory is gone: $t5/stranded-wt"
fi
echo

# ─── Test 6: `find` failure refused with exit 3, not read as quiet ──────────
echo "-- Test 6: find failure refused with exit 3"
t6="$tmpdir/t6"
new_repo_with_origin "$t6"
add_worktree "$t6" "jason/kyo-733-findfail" "findfail-wt"
stub_find_bin="$t6/stub-find-bin"
mkdir -p "$stub_find_bin"
cat >"$stub_find_bin/find" <<'STUB'
#!/usr/bin/env bash
# Reproduces this machine's real `find` (bfs) rejecting GNU's relative
# -newermt syntax: exits 1, prints NOTHING. If retire-worktree.sh ever
# pipes find's output into something that discards this exit status, this
# stub proves it by making the suite fail instead of silently reading an
# empty result as "no recent files" (the KYO-511 shape).
exit 1
STUB
chmod +x "$stub_find_bin/find"
PATH="$stub_find_bin:$PATH" run_retire "$t6/findfail-wt"
assert_exit "refused, could not complete" 3
assert_contains "names the find failure" "'find' failed"
assert_contains "says nothing was removed" "Nothing was removed"
if [ -d "$t6/findfail-wt" ]; then
    pass "the tree was not removed"
else
    fail "the tree was not removed" "directory is gone: $t6/findfail-wt"
fi
echo

# ─── Test 7: fetch failure refused with exit 3 ───────────────────────────────
echo "-- Test 7: fetch failure refused with exit 3"
t7="$tmpdir/t7"
new_repo_with_origin "$t7"
add_worktree "$t7" "jason/kyo-733-fetchfail" "fetchfail-wt"
git -C "$t7/fetchfail-wt" remote set-url origin "$t7/this-origin-does-not-exist.git"
age_tree "$t7/fetchfail-wt"
run_retire "$t7/fetchfail-wt"
assert_exit "refused, could not complete" 3
assert_contains "names the fetch failure" "git fetch --prune origin' failed"
if [ -d "$t7/fetchfail-wt" ]; then
    pass "the tree was not removed"
else
    fail "the tree was not removed" "directory is gone: $t7/fetchfail-wt"
fi
echo

# ─── Test 8: a process whose cwd is inside the tree is refused ──────────────
echo "-- Test 8: a live process cwd inside the tree is refused"
t8="$tmpdir/t8"
new_repo_with_origin "$t8"
add_worktree "$t8" "jason/kyo-733-procwd" "procwd-wt"
(
    cd "$t8/procwd-wt"
    exec sleep 30
) &
bgpid=$!
BG_PIDS+=("$bgpid")
# Give the subshell time to actually chdir and exec before we scan /proc.
for _ in 1 2 3 4 5 6 7 8 9 10; do
    if [ -d "/proc/$bgpid" ] && [ "$(readlink "/proc/$bgpid/cwd" 2>/dev/null)" = "$(realpath "$t8/procwd-wt")" ]; then
        break
    fi
    sleep 0.2
done
run_retire "$t8/procwd-wt"
kill "$bgpid" >/dev/null 2>&1 || true
wait "$bgpid" 2>/dev/null || true
assert_exit "refused" 1
assert_contains "names the live-process reason" "live process cwd inside the tree"
if [ -d "$t8/procwd-wt" ]; then
    pass "the tree was not removed"
else
    fail "the tree was not removed" "directory is gone: $t8/procwd-wt"
fi
echo

# ─── Test 9: the primary worktree is refused (usage error, exit 2) ──────────
echo "-- Test 9: primary worktree -> exit 2"
t9="$tmpdir/t9"
new_repo_with_origin "$t9"
run_retire "$t9/primary"
assert_exit "usage error" 2
assert_contains "explains why" "primary worktree / canonical clone"
echo

# ─── Test 10: a non-worktree path is refused (usage error, exit 2) ──────────
echo "-- Test 10: non-worktree path -> exit 2"
t10="$tmpdir/t10"
mkdir -p "$t10/plain-dir"
run_retire "$t10/plain-dir"
assert_exit "usage error" 2
assert_contains "explains why" "not a git working tree"
echo

# ─── Test 11: --force with a mismatched second path -> exit 2 ───────────────
echo "-- Test 11: --force mismatched second path -> exit 2"
t11="$tmpdir/t11"
new_repo_with_origin "$t11"
add_worktree "$t11" "jason/kyo-733-mismatch" "mismatch-wt"
run_retire --force "$t11/mismatch-wt" "$t11/primary"
assert_exit "usage error" 2
assert_contains "explains why" "identical path twice"
if [ -d "$t11/mismatch-wt" ]; then
    pass "the tree was not removed"
else
    fail "the tree was not removed" "directory is gone: $t11/mismatch-wt"
fi
echo

# ─── Test 12: --force with a matching path removes a refused tree ───────────
echo "-- Test 12: --force removes a refused tree, prints overridden reasons"
t12="$tmpdir/t12"
new_repo_with_origin "$t12"
add_worktree "$t12" "jason/kyo-733-force" "force-wt"
echo "dirty" >"$t12/force-wt/dirty.txt"
run_retire --force "$t12/force-wt" "$t12/force-wt"
assert_exit "removed under --force" 0
assert_contains "prints the OVERRIDING header" "OVERRIDING (--force)"
assert_contains "prints the overridden reason" "recent write(s):"
assert_contains "confirms removal" "Removed worktree:"
if [ -d "$t12/force-wt" ]; then
    fail "the tree is actually gone" "directory still present: $t12/force-wt"
else
    pass "the tree is actually gone"
fi
echo

# ─── Test 13: target/ writes alone do NOT count as live ─────────────────────
echo "-- Test 13: target/ writes alone do not count as live"
t13="$tmpdir/t13"
new_repo_with_origin "$t13"
add_worktree "$t13" "jason/kyo-733-target-only" "target-wt"
mkdir -p "$t13/target-wt/target/deep"
echo "build artifact" >"$t13/target-wt/target/deep/out.bin"
run_retire "$t13/target-wt"
assert_exit "removed (target/ writes are not a live signal)" 0
if [ -d "$t13/target-wt" ]; then
    fail "the tree is actually gone" "directory still present: $t13/target-wt"
else
    pass "the tree is actually gone"
fi
echo

# ─── Test 14: usage errors — argument shape ──────────────────────────────────
echo "-- Test 14: argument-shape usage errors"
run_retire
assert_exit "no arguments" 2
t14="$tmpdir/t14"
mkdir -p "$t14/does-not-exist-parent"
run_retire "$t14/does-not-exist-parent/nope"
assert_exit "missing path" 2
run_retire --force "$t14"
assert_exit "--force with only one path" 2
echo

echo "Fixture suite: $PASS passed, $FAIL failed so far"
echo

# ══════════════════════════════════════════════════════════════════════════
# MUTATION CHECKS — prove the two most safety-critical assertions above
# actually exercise retire-worktree.sh's own bytes, not a fixture artefact.
# Each check: `cp` the script to a scratch copy under $tmpdir, `sed -i` a
# targeted regression into the COPY, `diff` it against the real script to
# prove the mutation took, then run the one fixture that assertion depends
# on against the copy and confirm it now slips through. The tracked script
# is never modified, so there is nothing to restore — see the header's
# MUTATION TESTING block and the byte-identical guard at the end of the file.
# ══════════════════════════════════════════════════════════════════════════

MUTATION_PASS=0
MUTATION_FAIL=0

mutation_pass() {
    printf "  \xe2\x9c\x93 %s\n" "$1"
    MUTATION_PASS=$((MUTATION_PASS + 1))
}

mutation_fail() {
    printf "  \xe2\x9c\x97 %s\n" "$1"
    printf '    %s\n' "$2" | sed 's/^/    | /'
    MUTATION_FAIL=$((MUTATION_FAIL + 1))
}

# The scratch copy lives in its own scripts/ + scripts/lib/ directory, not
# a bare file under $tmpdir, because retire-worktree.sh resolves its own
# SCRIPT_DIR from ${BASH_SOURCE[0]} and unconditionally `source`s
# lib/stale-tooling-guard.sh relative to it under `set -e` — a bare copy
# with no sibling lib/ would abort on that `source` before the mutation
# under test ever ran. (The guard itself degrades to a harmless stderr
# warning here, since $tmpdir is not inside any git repo and the guard's
# own "cannot determine" path never changes a caller's exit code — see
# scripts/lib/stale-tooling-guard.sh's own header. This is not testing the
# guard; the scratch copy just needs to not explode on `source`.)
MUTATION_SCRATCH_DIR="$tmpdir/mutation-scratch/scripts"
mkdir -p "$MUTATION_SCRATCH_DIR/lib"
cp "$SCRIPT_DIR/lib/stale-tooling-guard.sh" "$MUTATION_SCRATCH_DIR/lib/stale-tooling-guard.sh"
MUTATED_SCRIPT="$MUTATION_SCRATCH_DIR/retire-worktree.sh"

echo "-- Mutation check A: find-failure fail-closed behaviour (Test 6)"
# The exact KYO-511 shape: make a failed `find` read as "no recent files"
# by short-circuiting its exit status with `|| true`, so the else/INCOMPLETE
# branch this script's fail-closed behaviour depends on can never run.
# Mutated on a fresh COPY, never on $SCRIPT itself (see MUTATION TESTING
# above).
cp "$SCRIPT" "$MUTATED_SCRIPT"
chmod +x "$MUTATED_SCRIPT"
sed -i 's/-newermt "\$CUTOFF" -print 2>"\$find_stderr_file")"; then/-newermt "$CUTOFF" -print 2>"$find_stderr_file")" || true; then/' "$MUTATED_SCRIPT"
if diff -q "$SCRIPT" "$MUTATED_SCRIPT" >/dev/null 2>&1; then
    mutation_fail "mutation A actually changed the script" "sed made no change — the anchor pattern did not match current source, this mutation proves nothing"
else
    mutation_pass "mutation A actually changed the script (diff confirms)"
    PATH="$stub_find_bin:$PATH" run_script "$MUTATED_SCRIPT" "$t6/findfail-wt"
    if [ "$RUN_STATUS" -eq 3 ]; then
        mutation_fail "mutated script still refuses on find failure (expected the fail-closed check to be broken)" "$RUN_OUTPUT"
    else
        mutation_pass "mutated script no longer refuses on find failure — Test 6 would have failed to catch this regression, confirming it is real coverage (got exit $RUN_STATUS instead of 3)"
    fi
fi
echo

echo "-- Mutation check B: live-tree (recent write) refusal (Test 1b)"
# Disable the recent-write reason outright — the exact class of defect
# Test 1/1b exist to catch (a live agent's tree silently reads as
# removable). Uses Test 1b's ISOLATED fixture (tracked file, mtime-only
# touch, git status stays clean) rather than Test 1's, specifically so this
# check is not confounded by Test 1's fixture ALSO being independently
# flagged for uncommitted work — this mutation must be shown to flip the
# verdict on its own, not ride alongside an unrelated passing check.
# Mutated on a fresh COPY, never on $SCRIPT itself (see MUTATION TESTING
# above).
cp "$SCRIPT" "$MUTATED_SCRIPT"
chmod +x "$MUTATED_SCRIPT"
sed -i 's/if \[ -n "\$RECENT_FILES" \]; then/if false \&\& [ -n "$RECENT_FILES" ]; then/' "$MUTATED_SCRIPT"
if diff -q "$SCRIPT" "$MUTATED_SCRIPT" >/dev/null 2>&1; then
    mutation_fail "mutation B actually changed the script" "sed made no change — the anchor pattern did not match current source, this mutation proves nothing"
else
    mutation_pass "mutation B actually changed the script (diff confirms)"
    run_script "$MUTATED_SCRIPT" "$t1b/recentonly-wt"
    if [ "$RUN_STATUS" -eq 1 ] && printf '%s' "$RUN_OUTPUT" | grep -qF "recent write(s):"; then
        mutation_fail "mutated script still refuses the isolated recent-write tree (expected this check to be broken)" "$RUN_OUTPUT"
    else
        mutation_pass "mutated script now silently accepts the isolated recent-write tree — Test 1b would have failed to catch this regression, confirming it is real coverage (exit $RUN_STATUS)"
    fi
fi
echo

echo "Mutation checks: $MUTATION_PASS passed, $MUTATION_FAIL failed"
PASS=$((PASS + MUTATION_PASS))
FAIL=$((FAIL + MUTATION_FAIL))
echo

# ─── Suite-level guard: the real script was never modified on disk ──────────
# Every mutation above operated on $MUTATED_SCRIPT, a throwaway copy under
# $tmpdir — this is the suite's own proof that scripts/retire-worktree.sh's
# on-disk bytes never moved from the snapshot taken at the very top of this
# file. If a future change to this suite (or anything it calls) ever
# mutates the real, tracked file again, this comparison is what catches it.
echo "-- Suite-level guard: scripts/retire-worktree.sh was never modified on disk"
if cmp -s "$PRODUCTION_SNAPSHOT" "$SCRIPT"; then
    pass "scripts/retire-worktree.sh is byte-identical to its pre-suite snapshot"
else
    fail "scripts/retire-worktree.sh is byte-identical to its pre-suite snapshot" \
        "cmp reports a difference — this suite (or something it called) modified the real, tracked script; that must never happen"
fi
echo

echo "Results: $PASS passed, $FAIL failed"
[ "$FAIL" -eq 0 ]
