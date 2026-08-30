#!/usr/bin/env bash
# ------------------------------------------------------------------------------
# scripts/mark-branch-stranded-test.sh — self-test for
# mark-branch-stranded.sh (KYO-567)
#
# Follows the same pattern as mark-worktree-stranded-test.sh and
# check-ticket-in-flight-test.sh: real throwaway bare git remotes and real
# `git worktree add` fixtures under a fresh mktemp -d, removed on exit, plus
# a stub `gh` (mirroring check-ticket-in-flight-test.sh's file-replay stub,
# since — unlike mark-worktree-stranded.sh — this script actually calls
# `gh pr list`). Never touches the real kyomi repo, the real origin, or the
# real `gh`.
#
# The most important test here is "failed push leaves the original remote
# ref intact" (Test 9): this script's entire safety model is "verify before
# destroying," and that is only worth anything if a failure genuinely rolls
# back to nothing having moved.
#
# Exit 0 = all pass, exit 1 = any failure.
# ------------------------------------------------------------------------------

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
MARK="$SCRIPT_DIR/mark-branch-stranded.sh"
MARK_WORKTREE="$SCRIPT_DIR/mark-worktree-stranded.sh"
PASS=0
FAIL=0

tmpdir="$(mktemp -d)"
trap 'rm -rf "$tmpdir"' EXIT

export GIT_AUTHOR_NAME="KYO-567 Test" GIT_AUTHOR_EMAIL="kyo567-test@kyomi.invalid"
export GIT_COMMITTER_NAME="KYO-567 Test" GIT_COMMITTER_EMAIL="kyo567-test@kyomi.invalid"

# ─── stub `gh`, controlled by files the harness writes before each call ─────
# Identical mechanism to check-ticket-in-flight-test.sh's stub: replays
# whatever was staged in GH_STDOUT_FILE/GH_STDERR_FILE/GH_EXIT_FILE,
# ignoring its actual arguments. First on PATH for the duration of the run.
STUB_BIN="$tmpdir/bin"
mkdir -p "$STUB_BIN"
export GH_STDOUT_FILE="$tmpdir/gh_stdout"
export GH_STDERR_FILE="$tmpdir/gh_stderr"
export GH_EXIT_FILE="$tmpdir/gh_exit"

cat >"$STUB_BIN/gh" <<'STUB'
#!/usr/bin/env bash
cat "$GH_STDOUT_FILE"
cat "$GH_STDERR_FILE" >&2
exit "$(cat "$GH_EXIT_FILE")"
STUB
chmod +x "$STUB_BIN/gh"
export PATH="$STUB_BIN:$PATH"

gh_ok_empty() { : >"$GH_STDOUT_FILE"; : >"$GH_STDERR_FILE"; echo 0 >"$GH_EXIT_FILE"; }
gh_ok_prs() {
    # gh_ok_prs '<number>\t<state>' [...] — matches the TSV shape
    # mark-branch-stranded.sh asks `gh` to produce via
    # --jq '.[] | [.number, .state] | @tsv'.
    printf '%s\n' "$@" >"$GH_STDOUT_FILE"
    : >"$GH_STDERR_FILE"
    echo 0 >"$GH_EXIT_FILE"
}
gh_fail() {
    : >"$GH_STDOUT_FILE"
    printf '%s\n' "$1" >"$GH_STDERR_FILE"
    echo 1 >"$GH_EXIT_FILE"
}
gh_ok_empty # default: no PRs, until a test says otherwise

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
    # run_mark <dir> <arg>...  — invokes mark-branch-stranded.sh with cwd
    # set to <dir>, since (unlike mark-worktree-stranded.sh's explicit
    # --worktree) this script has no --repo flag: it operates on whatever
    # repo the cwd resolves to, for both the local-branch check and the
    # default `--remote origin`.
    local dir="$1" out
    shift
    if out="$(cd "$dir" && "$MARK" "$@" 2>&1)"; then
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

assert_not_contains() {
    local name="$1" needle="$2"
    if printf '%s' "$RUN_OUTPUT" | grep -qF -- "$needle"; then
        fail "$name — expected output NOT to contain: $needle" "$RUN_OUTPUT"
    else
        pass "$name"
    fi
}

assert_ref_sha() {
    # assert_ref_sha <name> <bare_remote> <ref> <expected_sha|MISSING>
    local name="$1" bare="$2" ref="$3" expected="$4" actual
    if line="$(git ls-remote --exit-code "$bare" "$ref" 2>/dev/null)"; then
        actual="${line%%$'\t'*}"
    else
        actual="MISSING"
    fi
    if [ "$actual" = "$expected" ]; then
        pass "$name"
    else
        fail "$name" "expected $ref = $expected, got $actual"
    fi
}

# ─── real git repo helpers (mirrors the sibling test suites) ───────────────
new_bare_remote() {
    local dir="$1"
    git init --bare -q "$dir"
    git -C "$dir" symbolic-ref HEAD refs/heads/main
}

seed_main() {
    # seed_main <bare_remote_dir>
    local bare="$1" seed
    seed="$(mktemp -d)"
    git init -q -b main "$seed"
    echo "seed" >"$seed/README.md"
    git -C "$seed" add README.md
    git -C "$seed" commit -q -m init
    git -C "$seed" remote add origin "$bare"
    git -C "$seed" push -q origin main
    rm -rf "$seed"
}

clone_repo() { git clone -q "$1" "$2"; } # clone_repo <bare> <dest>

push_new_branch() {
    # push_new_branch <clone_dir> <branch> — checks out a new branch in
    # <clone_dir>, commits one change, and pushes it to origin. Leaves the
    # branch checked out at HEAD in <clone_dir>.
    local dir="$1" branch="$2"
    git -C "$dir" checkout -q -b "$branch"
    echo "work on $branch" >>"$dir/README.md"
    git -C "$dir" commit -q -am "start $branch"
    git -C "$dir" push -q origin "$branch"
}

echo "Running mark-branch-stranded self-tests..."
echo

# ─── Test 1: happy path — sha preserved under stranded/, original gone ────
echo "-- Test 1: happy path, no local branch anywhere"
t1="$tmpdir/t1"
mkdir -p "$t1"
bare1="$t1/remote.git"
new_bare_remote "$bare1"
seed_main "$bare1"
clone_repo "$bare1" "$t1/workerA"
push_new_branch "$t1/workerA" "jason/kyo-567-happy"
sha1="$(git -C "$t1/workerA" rev-parse HEAD)"
clone_repo "$bare1" "$t1/releaser" # no matching local branch here at all
gh_ok_empty
run_mark "$t1/releaser" 567 --branch jason/kyo-567-happy --note "smoke"
assert_exit "tombstones successfully" 0
assert_contains "reports the rename" "TOMBSTONED: jason/kyo-567-happy -> stranded/jason/kyo-567-happy"
assert_contains "includes the note" "smoke"
assert_ref_sha "stranded/ ref preserves the sha" "$bare1" "refs/heads/stranded/jason/kyo-567-happy" "$sha1"
assert_ref_sha "original ref is gone" "$bare1" "refs/heads/jason/kyo-567-happy" "MISSING"
echo

# ─── Test 2: happy path — local branch exists but isn't checked out ───────
echo "-- Test 2: local branch present elsewhere, not checked out, gets renamed"
t2="$tmpdir/t2"
mkdir -p "$t2"
bare2="$t2/remote.git"
new_bare_remote "$bare2"
seed_main "$bare2"
clone_repo "$bare2" "$t2/workerA"
push_new_branch "$t2/workerA" "jason/kyo-567-localrename"
git -C "$t2/workerA" checkout -q main # move off the branch so it's not "checked out" for THIS check —
# note: `checked out` in mark-branch-stranded.sh means checked out in some
# OTHER worktree via `git worktree list`, not "is it the current branch of
# the repo we run from" — this repo (workerA) is where we release FROM, and
# its own local branch of the same name is exactly the "exists, not checked
# out anywhere" case, since checking out main freed it.
gh_ok_empty
run_mark "$t2/workerA" 567 --branch jason/kyo-567-localrename
assert_exit "tombstones and renames the local branch" 0
assert_contains "reports the local rename" "renamed local branch 'jason/kyo-567-localrename' -> 'stranded/jason/kyo-567-localrename'"
if git -C "$t2/workerA" show-ref --verify --quiet "refs/heads/stranded/jason/kyo-567-localrename"; then
    pass "local branch actually renamed"
else
    fail "local branch actually renamed" "$(git -C "$t2/workerA" branch --list)"
fi
if git -C "$t2/workerA" show-ref --verify --quiet "refs/heads/jason/kyo-567-localrename"; then
    fail "old local branch name is gone" "old local branch still present"
else
    pass "old local branch name is gone"
fi
echo

# ─── Test 2b: local branch present elsewhere, but the rename FAILS ────────
# Companion to Test 2: same "exists, not checked out anywhere" setup, but
# here `git branch -m` itself fails — forced by pre-creating a conflicting
# local `stranded/<branch>` ref, so the rename has nowhere to land. This is
# the reviewer-flagged MAJOR gap: LOCAL_RENAMED=0, LOCAL_BRANCH_CHECKED_OUT_AT
# empty, LOCAL_BRANCH_EXISTS=1 — the one reachable combination the pasteable
# summary used to print no "Local:" line for at all, silently losing the
# residual-inconsistency warning that otherwise only ever reached stderr.
echo "-- Test 2b: local rename fails (conflicting local stranded/ ref), summary carries the warning"
t2b="$tmpdir/t2b"
mkdir -p "$t2b"
bare2b="$t2b/remote.git"
new_bare_remote "$bare2b"
seed_main "$bare2b"
clone_repo "$bare2b" "$t2b/workerA"
push_new_branch "$t2b/workerA" "jason/kyo-567-renamefail"
git -C "$t2b/workerA" checkout -q main # free the branch up, same as Test 2
git -C "$t2b/workerA" branch -q stranded/jason/kyo-567-renamefail main # conflicting ref so `git branch -m` has nowhere to land
gh_ok_empty
run_mark "$t2b/workerA" 567 --branch jason/kyo-567-renamefail
assert_exit "still tombstones the remote even though the local rename fails" 0
assert_contains "reports the remote tombstone" "TOMBSTONED: jason/kyo-567-renamefail -> stranded/jason/kyo-567-renamefail"
assert_contains "the pasteable summary carries the NOT-renamed warning" "  - Local:   NOT renamed"
assert_contains "the summary names the stale branch" "stale local branch 'jason/kyo-567-renamefail'"
assert_contains "the summary gives the --ignore-branch remedy" "--ignore-branch 'jason/kyo-567-renamefail'"
if git -C "$t2b/workerA" show-ref --verify --quiet "refs/heads/jason/kyo-567-renamefail"; then
    pass "original local branch name is still present (rename never happened)"
else
    fail "original local branch name is still present (rename never happened)" "$(git -C "$t2b/workerA" branch --list)"
fi
echo

# ─── Test 3: refuses main ───────────────────────────────────────────────────
echo "-- Test 3: refuses main"
t3="$tmpdir/t3"
mkdir -p "$t3"
bare3="$t3/remote.git"
new_bare_remote "$bare3"
seed_main "$bare3"
clone_repo "$bare3" "$t3/releaser"
run_mark "$t3/releaser" 567 --branch main
assert_exit "refuses to tombstone main" 1
assert_contains "explains why" "refusing to tombstone branch 'main'"
echo

# ─── Test 4: refuses when a PR exists (any state) ──────────────────────────
echo "-- Test 4: refuses when the branch has a PR"
t4="$tmpdir/t4"
mkdir -p "$t4"
bare4="$t4/remote.git"
new_bare_remote "$bare4"
seed_main "$bare4"
clone_repo "$bare4" "$t4/workerA"
push_new_branch "$t4/workerA" "jason/kyo-567-haspr"
clone_repo "$bare4" "$t4/releaser"
gh_ok_prs $'510\tOPEN'
run_mark "$t4/releaser" 567 --branch jason/kyo-567-haspr
assert_exit "refuses a branch with an open PR" 1
assert_contains "explains why" "it has 1 PR(s)"
assert_ref_sha "original ref is untouched" "$bare4" "refs/heads/jason/kyo-567-haspr" "$(git -C "$t4/workerA" rev-parse HEAD)"
gh_ok_empty
echo

# ─── Test 5: idempotency — --branch stranded/x passed directly ────────────
echo "-- Test 5: idempotent when --branch is already under stranded/"
t5="$tmpdir/t5"
mkdir -p "$t5"
bare5="$t5/remote.git"
new_bare_remote "$bare5"
seed_main "$bare5"
clone_repo "$bare5" "$t5/releaser"
run_mark "$t5/releaser" 567 --branch stranded/jason/kyo-567-already
assert_exit "reports already tombstoned and exits 0" 0
assert_contains "says so" "ALREADY TOMBSTONED"
echo

# ─── Test 6: idempotency — retry after a prior run already completed ──────
echo "-- Test 6: idempotent retry after prior completion"
t6="$tmpdir/t6"
mkdir -p "$t6"
bare6="$t6/remote.git"
new_bare_remote "$bare6"
seed_main "$bare6"
clone_repo "$bare6" "$t6/workerA"
push_new_branch "$t6/workerA" "jason/kyo-567-retry"
sha6="$(git -C "$t6/workerA" rev-parse HEAD)"
clone_repo "$bare6" "$t6/releaser"
gh_ok_empty
run_mark "$t6/releaser" 567 --branch jason/kyo-567-retry
assert_exit "first run tombstones successfully" 0
run_mark "$t6/releaser" 567 --branch jason/kyo-567-retry
assert_exit "second run recognizes it's already done" 0
assert_contains "says so" "ALREADY TOMBSTONED"
assert_ref_sha "stranded/ ref still has the right sha" "$bare6" "refs/heads/stranded/jason/kyo-567-retry" "$sha6"
echo

# ─── Test 6b: retry where BOTH refs exist at the SAME sha ─────────────────
# Distinct from Test 6 (retry after a run that fully completed, where the
# original ref is already gone): here a prior run pushed stranded/<branch>
# and then died BEFORE deleting the original, so on retry BOTH refs are
# present on the remote, pointing at the identical sha. Simulated directly
# by pushing the branch's own sha to refs/heads/stranded/<branch> without
# ever deleting refs/heads/<branch>. The push-copy step becomes a no-op
# fast-forward (the ref already points there), verification passes, and
# only then does the original get deleted — pinning that as a real,
# asserted behaviour rather than an emergent property of the sha-compare
# logic.
echo "-- Test 6b: retry where both refs exist at the same sha (died before delete)"
t6b="$tmpdir/t6b"
mkdir -p "$t6b"
bare6b="$t6b/remote.git"
new_bare_remote "$bare6b"
seed_main "$bare6b"
clone_repo "$bare6b" "$t6b/workerA"
push_new_branch "$t6b/workerA" "jason/kyo-567-samesha"
sha6b="$(git -C "$t6b/workerA" rev-parse HEAD)"
git -C "$t6b/workerA" push -q origin "jason/kyo-567-samesha:refs/heads/stranded/jason/kyo-567-samesha"
clone_repo "$bare6b" "$t6b/releaser"
gh_ok_empty
run_mark "$t6b/releaser" 567 --branch jason/kyo-567-samesha
assert_exit "completes the interrupted tombstone" 0
assert_contains "reports a genuine completion, not a no-op idempotency message" "TOMBSTONED: jason/kyo-567-samesha -> stranded/jason/kyo-567-samesha"
assert_ref_sha "original ref is now gone" "$bare6b" "refs/heads/jason/kyo-567-samesha" "MISSING"
assert_ref_sha "stranded/ ref still exists at the same sha" "$bare6b" "refs/heads/stranded/jason/kyo-567-samesha" "$sha6b"
echo

# ─── Test 7: local branch checked out elsewhere WITHOUT a tombstone ───────
echo "-- Test 7: refuses when checked out in a worktree with no STRANDED.md"
t7="$tmpdir/t7"
mkdir -p "$t7"
bare7="$t7/remote.git"
new_bare_remote "$bare7"
seed_main "$bare7"
clone_repo "$bare7" "$t7/primary"
git -C "$t7/primary" worktree add -q -b jason/kyo-567-checked "$t7/wt-checked"
echo "work" >>"$t7/wt-checked/README.md"
git -C "$t7/wt-checked" commit -q -am "wip"
git -C "$t7/wt-checked" push -q origin jason/kyo-567-checked
gh_ok_empty
run_mark "$t7/primary" 567 --branch jason/kyo-567-checked
assert_exit "refuses without a valid worktree tombstone" 1
assert_contains "names the checked-out worktree" "$t7/wt-checked"
assert_contains "prints the exact command to run first" "$MARK_WORKTREE KYO-567 --worktree $t7/wt-checked"
assert_ref_sha "remote ref is untouched" "$bare7" "refs/heads/jason/kyo-567-checked" "$(git -C "$t7/wt-checked" rev-parse HEAD)"
echo

# ─── Test 8: local branch checked out elsewhere WITH a valid tombstone ────
echo "-- Test 8: succeeds when the checked-out worktree already has a tombstone"
t8="$tmpdir/t8"
mkdir -p "$t8"
bare8="$t8/remote.git"
new_bare_remote "$bare8"
seed_main "$bare8"
clone_repo "$bare8" "$t8/primary"
git -C "$t8/primary" worktree add -q -b jason/kyo-567-tombstoned "$t8/wt-tombstoned"
echo "work" >>"$t8/wt-tombstoned/README.md"
git -C "$t8/wt-tombstoned" commit -q -am "wip"
git -C "$t8/wt-tombstoned" push -q origin jason/kyo-567-tombstoned
"$MARK_WORKTREE" 567 --worktree "$t8/wt-tombstoned" >/dev/null
gh_ok_empty
run_mark "$t8/primary" 567 --branch jason/kyo-567-tombstoned
assert_exit "succeeds — the worktree's own tombstone covers the local branch" 0
assert_contains "reports the remote rename" "TOMBSTONED: jason/kyo-567-tombstoned -> stranded/jason/kyo-567-tombstoned"
assert_contains "reports the local branch was left in place" "left in place, checked out at $t8/wt-tombstoned"
if [ "$(git -C "$t8/wt-tombstoned" rev-parse --abbrev-ref HEAD)" = "jason/kyo-567-tombstoned" ]; then
    pass "local branch name in the worktree is unchanged"
else
    fail "local branch name in the worktree is unchanged" "$(git -C "$t8/wt-tombstoned" rev-parse --abbrev-ref HEAD)"
fi
echo

# ─── Test 9: a failed push leaves the original remote ref intact ──────────
# THE fail-closed proof. Force a non-fast-forward push by pre-creating
# stranded/<branch> pointing at an UNRELATED commit before running the
# script — git's own push protection then rejects the write, exactly the
# real-world case of a botched partial prior run or a name collision, and
# the original ref must survive completely untouched.
echo "-- Test 9: failed push leaves the original ref intact (fail-closed)"
t9="$tmpdir/t9"
mkdir -p "$t9"
bare9="$t9/remote.git"
new_bare_remote "$bare9"
seed_main "$bare9"
clone_repo "$bare9" "$t9/workerA"
push_new_branch "$t9/workerA" "jason/kyo-567-conflict"
sha9="$(git -C "$t9/workerA" rev-parse HEAD)"
git -C "$t9/workerA" checkout -q -b unrelated-history main
echo "unrelated" >"$t9/workerA/OTHER.md"
git -C "$t9/workerA" add OTHER.md
git -C "$t9/workerA" commit -q -m "unrelated commit"
git -C "$t9/workerA" push -q origin unrelated-history:refs/heads/stranded/jason/kyo-567-conflict
clone_repo "$bare9" "$t9/releaser"
gh_ok_empty
run_mark "$t9/releaser" 567 --branch jason/kyo-567-conflict
assert_exit "the conflicting push fails" 1
assert_contains "explains the original is untouched" "Original refs/heads/jason/kyo-567-conflict is untouched"
assert_ref_sha "original ref still exists at its original sha" "$bare9" "refs/heads/jason/kyo-567-conflict" "$sha9"
echo

# ─── Test 10: usage errors ───────────────────────────────────────────────────
echo "-- Test 10: usage errors"
run_mark "$tmpdir" # no ticket
assert_exit "no argument" 2
run_mark "$tmpdir" "not-a-ticket" --branch x
assert_exit "unparseable ticket" 2
run_mark "$tmpdir" 567 # missing --branch
assert_exit "missing --branch" 2
run_mark "$tmpdir" 567 --branch
assert_exit "--branch missing its value" 2
run_mark "$tmpdir" 567 --branch x --bogus-flag
assert_exit "unknown flag" 2
echo

echo "Results: $PASS passed, $FAIL failed"
[ "$FAIL" -eq 0 ]
