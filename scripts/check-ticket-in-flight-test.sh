#!/usr/bin/env bash
# ------------------------------------------------------------------------------
# scripts/check-ticket-in-flight-test.sh — self-test for
# check-ticket-in-flight.sh (KYO-422)
#
# Exercises the script against a REAL throwaway git repo with a REAL bare
# remote (`git init`, `git init --bare`, `git push`) plus a stub `gh` placed
# first on PATH, whose canned output each test controls. Never touches the
# real kyomi repo, the real origin, or the real `gh` — everything happens
# under a fresh mktemp -d that is removed on exit.
#
# Test 2 is the KYO-422 acceptance criterion itself: an actual simulated
# double-pickup (worker A pushes, worker B — a separate, untouched clone —
# checks before starting and is told no).
#
# Tests 26-31 (KYO-593) cover `--self`, the cwd-independent self-exclusion
# flag: a worker running the pre-review call from outside its own worktree
# (cwd on `main` in the canonical clone, its implementation living in a
# linked worktree) previously had its own finished work reported IN FLIGHT
# and abandoned it as a lost race (KYO-291, 2026-09-02). Test 26 reproduces
# that symptom from a single fixture and then shows `--self` fixes it from
# the exact same fixture, so the fix — not an unrelated setup change — is
# what flips the verdict.
#
# Tests 32-39 (KYO-607) cover recycled ticket keys: Trakkt's numbering
# restarted in May 2026, so nine keys are shared between a retired ticket and
# a current one, and the merged PR (plus its surviving remote branch) of the
# retired one blocked the current one forever. Test 32 is the acceptance
# criterion — a pre-restart merged PR AND its remote branch must together
# still yield CLEAR — and test 34 is the precision test (the real KYO-293/294
# shape: one pre-restart PR and one legitimate current-numbering PR on the
# same key, where only the second may count).
#
# Exit 0 = all pass, exit 1 = any failure.
# ------------------------------------------------------------------------------

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
SCRIPT="$SCRIPT_DIR/check-ticket-in-flight.sh"
PASS=0
FAIL=0

tmpdir="$(mktemp -d)"
trap 'rm -rf "$tmpdir"' EXIT

# Give every throwaway commit a fixed identity so tests don't depend on (or
# pollute) the real machine's git config.
export GIT_AUTHOR_NAME="KYO-422 Test" GIT_AUTHOR_EMAIL="kyo422-test@kyomi.invalid"
export GIT_COMMITTER_NAME="KYO-422 Test" GIT_COMMITTER_EMAIL="kyo422-test@kyomi.invalid"

# ─── stub `gh`, controlled by files the harness writes before each call ─────
STUB_BIN="$tmpdir/bin"
mkdir -p "$STUB_BIN"
export GH_STDOUT_FILE="$tmpdir/gh_stdout"
export GH_STDERR_FILE="$tmpdir/gh_stderr"
export GH_EXIT_FILE="$tmpdir/gh_exit"

cat >"$STUB_BIN/gh" <<'STUB'
#!/usr/bin/env bash
# Test-only stand-in for `gh`. Ignores its arguments entirely and replays
# whatever the test harness staged in GH_STDOUT_FILE / GH_STDERR_FILE /
# GH_EXIT_FILE. Ships only inside this test's own $tmpdir/bin, first on
# PATH for the duration of the run — never touches the real `gh` or network.
cat "$GH_STDOUT_FILE"
cat "$GH_STDERR_FILE" >&2
exit "$(cat "$GH_EXIT_FILE")"
STUB
chmod +x "$STUB_BIN/gh"
export PATH="$STUB_BIN:$PATH"

# Fixture timestamps, either side of check-ticket-in-flight.sh's default
# KEY_RESTART_CUTOFF of 2026-05-12T00:00:00Z (KYO-607). Named rather than
# inlined so a reader can tell at a glance which side of the ticket-key
# restart a fixture PR is meant to be on, and so the two values can never
# drift apart between call sites.
#
# The real boundary they straddle: PR #39 (2026-05-09T08:44:14Z) is the last
# retired-numbering PR, PR #41 (2026-05-13T09:25:07Z) the first of the
# restarted numbering.
PRE_RESTART_TS='2026-05-08T12:00:00Z'
POST_RESTART_TS='2026-08-21T09:00:00Z'

pr_row() {
    # pr_row <number> <state> <createdAt> <headRefName> — one PR row in the
    # exact 4-column TSV shape check-ticket-in-flight.sh asks `gh` to produce
    # via `--jq '.[] | [.number, .state, .createdAt, .headRefName] | @tsv'`.
    # A helper rather than a `$'...\t...'` literal at each call site because
    # createdAt is usually one of the two named constants above, and `$'...'`
    # does not interpolate.
    printf '%s\t%s\t%s\t%s' "$1" "$2" "$3" "$4"
}

gh_ok_empty() { : >"$GH_STDOUT_FILE"; : >"$GH_STDERR_FILE"; echo 0 >"$GH_EXIT_FILE"; }
gh_ok_prs() {
    # gh_ok_prs '<number>\t<state>\t<createdAt>\t<headRefName>' [...]  — one
    # arg per PR row; build each one with pr_row above.
    printf '%s\n' "$@" >"$GH_STDOUT_FILE"
    : >"$GH_STDERR_FILE"
    echo 0 >"$GH_EXIT_FILE"
}
gh_ok_prs_n() {
    # gh_ok_prs_n <count> [<extra row>...] — stage <count> filler PR rows, then
    # any extra rows given verbatim. Filler branches are jason/kyo-90<i>-filler,
    # which no ticket number any test asks about can match, so a row's only
    # contribution is to the row COUNT — which is what the PR_LIST_LIMIT
    # truncation guard keys off. Lets a test reach the limit in three rows
    # instead of five hundred. Fillers are dated POST_RESTART_TS so they are
    # never classified as pre-restart either; a filler must influence nothing
    # but the count.
    local count="$1" i
    shift
    : >"$GH_STDOUT_FILE"
    for ((i = 1; i <= count; i++)); do
        printf '%d\tMERGED\t%s\tjason/kyo-90%d-filler\n' "$((9000 + i))" "$POST_RESTART_TS" "$i" >>"$GH_STDOUT_FILE"
    done
    if [ "$#" -gt 0 ]; then
        printf '%s\n' "$@" >>"$GH_STDOUT_FILE"
    fi
    : >"$GH_STDERR_FILE"
    echo 0 >"$GH_EXIT_FILE"
}
gh_fail() {
    : >"$GH_STDOUT_FILE"
    printf '%s\n' "$1" >"$GH_STDERR_FILE"
    echo 1 >"$GH_EXIT_FILE"
}
gh_ok_empty # default: no PRs, until a test says otherwise

# ─── real git repo helpers ───────────────────────────────────────────────────
new_bare_remote() {
    # new_bare_remote <dir> -> echoes <dir>. Pre-points HEAD at refs/heads/main
    # so a later `git clone` checks out "main" the moment it exists, without
    # depending on this machine's init.defaultBranch.
    local dir="$1"
    git init --bare -q "$dir"
    git -C "$dir" symbolic-ref HEAD refs/heads/main
    echo "$dir"
}

seed_main() {
    # seed_main <bare_remote_dir> — pushes an initial commit on main so
    # clones aren't empty.
    local bare="$1" seed
    seed="$(mktemp -d)"
    git init -q "$seed"
    git -C "$seed" checkout -q -b main
    echo "seed" >"$seed/README.md"
    git -C "$seed" add README.md
    git -C "$seed" commit -q -m init
    git -C "$seed" remote add origin "$bare"
    git -C "$seed" push -q origin main
    rm -rf "$seed"
}

clone_repo() { git clone -q "$1" "$2"; } # clone_repo <bare> <dest>

# ─── invoke the script under test, capturing exit code + combined output ────
CHECK_STATUS=""
CHECK_OUTPUT=""
run_check() {
    # run_check <dir> <ticket> [extra args to check-ticket-in-flight.sh...]
    local dir="$1" ticket="$2" out
    shift 2
    if out="$(cd "$dir" && "$SCRIPT" "$ticket" "$@" 2>&1)"; then
        CHECK_STATUS=0
    else
        CHECK_STATUS=$?
    fi
    CHECK_OUTPUT="$out"
}

assert_exit() {
    local name="$1" expected="$2"
    if [ "$CHECK_STATUS" -eq "$expected" ]; then
        printf "  \xe2\x9c\x93 %s (exit %d)\n" "$name" "$CHECK_STATUS"
        PASS=$((PASS + 1))
    else
        printf "  \xe2\x9c\x97 %s \xe2\x80\x94 expected exit %d, got %d\n" "$name" "$expected" "$CHECK_STATUS"
        echo "    output:"
        echo "$CHECK_OUTPUT" | sed 's/^/    | /'
        FAIL=$((FAIL + 1))
    fi
}

assert_contains() {
    local name="$1" needle="$2"
    if printf '%s' "$CHECK_OUTPUT" | grep -qF -- "$needle"; then
        printf "  \xe2\x9c\x93 %s\n" "$name"
        PASS=$((PASS + 1))
    else
        printf "  \xe2\x9c\x97 %s \xe2\x80\x94 expected output to contain: %s\n" "$name" "$needle"
        echo "    output:"
        echo "$CHECK_OUTPUT" | sed 's/^/    | /'
        FAIL=$((FAIL + 1))
    fi
}

assert_not_contains() {
    local name="$1" needle="$2"
    if printf '%s' "$CHECK_OUTPUT" | grep -qF -- "$needle"; then
        printf "  \xe2\x9c\x97 %s \xe2\x80\x94 expected output NOT to contain: %s\n" "$name" "$needle"
        echo "    output:"
        echo "$CHECK_OUTPUT" | sed 's/^/    | /'
        FAIL=$((FAIL + 1))
    else
        printf "  \xe2\x9c\x93 %s\n" "$name"
        PASS=$((PASS + 1))
    fi
}

echo "Running check-ticket-in-flight self-tests..."
echo

# ─── Test 1: clean — no branches, gh returns nothing ────────────────────────
echo "-- Test 1: clean"
t1="$tmpdir/t1"
mkdir -p "$t1"
bare1="$(new_bare_remote "$t1/remote.git")"
seed_main "$bare1"
clone_repo "$bare1" "$t1/workerB"
gh_ok_empty
run_check "$t1/workerB" 422
assert_exit "clean repo, no PRs" 0
echo

# ─── Test 2: THE KYO-422 acceptance criterion — an actual simulated ─────────
# double pickup. Worker A pushes jason/kyo-422-guard to the shared remote.
# Worker B is a SEPARATE clone, still on main, has done zero work, and checks
# before starting — reproducing the exact KYO-416 failure mode (PRs #367/#368).
echo "-- Test 2: double pickup (KYO-422 acceptance criterion)"
t2="$tmpdir/t2"
mkdir -p "$t2"
bare2="$(new_bare_remote "$t2/remote.git")"
seed_main "$bare2"
clone_repo "$bare2" "$t2/workerA"
git -C "$t2/workerA" checkout -q -b jason/kyo-422-guard
echo "workerA change" >>"$t2/workerA/README.md"
git -C "$t2/workerA" commit -q -am "workerA: start KYO-422"
git -C "$t2/workerA" push -q origin jason/kyo-422-guard
clone_repo "$bare2" "$t2/workerB"
gh_ok_empty
run_check "$t2/workerB" 422
assert_exit "worker B sees worker A's pushed branch before starting" 1
assert_contains "names the remote branch" "remote branch: origin/jason/kyo-422-guard"
echo

# ─── Test 3: open PR ─────────────────────────────────────────────────────────
echo "-- Test 3: open PR"
t3="$tmpdir/t3"
mkdir -p "$t3"
bare3="$(new_bare_remote "$t3/remote.git")"
seed_main "$bare3"
clone_repo "$bare3" "$t3/workerB"
gh_ok_prs "$(pr_row 501 OPEN "$POST_RESTART_TS" jason/kyo-422-guard)"
run_check "$t3/workerB" 422
assert_exit "remote clean but an open PR matches" 1
assert_contains "names the PR number and state" "PR #501 (OPEN) branch jason/kyo-422-guard"
echo

# ─── Test 4: closed/merged PR — still counts, ticket isn't done ─────────────
echo "-- Test 4: merged PR"
t4="$tmpdir/t4"
mkdir -p "$t4"
bare4="$(new_bare_remote "$t4/remote.git")"
seed_main "$bare4"
clone_repo "$bare4" "$t4/workerB"
gh_ok_prs "$(pr_row 502 MERGED "$POST_RESTART_TS" jason/kyo-422-guard)"
run_check "$t4/workerB" 422
assert_exit "a merged PR means look before reimplementing" 1
assert_contains "names the PR number and state" "PR #502 (MERGED) branch jason/kyo-422-guard"
echo

# ─── Test 5: self-exclusion ──────────────────────────────────────────────────
echo "-- Test 5: self-exclusion"
t5="$tmpdir/t5"
mkdir -p "$t5"
bare5="$(new_bare_remote "$t5/remote.git")"
seed_main "$bare5"
clone_repo "$bare5" "$t5/workerA"
git -C "$t5/workerA" checkout -q -b jason/kyo-422-guard
echo "workerA change" >>"$t5/workerA/README.md"
git -C "$t5/workerA" commit -q -am "workerA: start KYO-422"
git -C "$t5/workerA" push -q origin jason/kyo-422-guard
gh_ok_empty
run_check "$t5/workerA" 422
assert_exit "worker A re-checking its own checked-out, already-pushed branch" 0

# A second, genuinely different worker's branch lands on the remote.
git -C "$t5/workerA" push -q origin jason/kyo-422-guard:refs/heads/jason/kyo-422-other
run_check "$t5/workerA" 422
assert_exit "self-exclusion must not blind it to a different branch" 1
assert_contains "names the other worker's branch" "remote branch: origin/jason/kyo-422-other"
assert_not_contains "does not re-flag its own excluded branch" "remote branch: origin/jason/kyo-422-guard"
echo

# ─── Test 6: --ignore-branch ─────────────────────────────────────────────────
echo "-- Test 6: --ignore-branch"
t6="$tmpdir/t6"
mkdir -p "$t6"
bare6="$(new_bare_remote "$t6/remote.git")"
seed_main "$bare6"
clone_repo "$bare6" "$t6/helper"
git -C "$t6/helper" push -q origin main:refs/heads/jason/kyo-422-explicit
clone_repo "$bare6" "$t6/workerB"
gh_ok_empty
run_check "$t6/workerB" 422
assert_exit "without --ignore-branch it is a real hit" 1
run_check "$t6/workerB" 422 --ignore-branch jason/kyo-422-explicit
assert_exit "--ignore-branch suppresses the named branch" 0
echo

# ─── Test 7: substring safety — the trailing-hyphen rule ────────────────────
echo "-- Test 7: substring safety"
t7="$tmpdir/t7"
mkdir -p "$t7"
bare7="$(new_bare_remote "$t7/remote.git")"
seed_main "$bare7"
clone_repo "$bare7" "$t7/helper"
git -C "$t7/helper" push -q origin main:refs/heads/jason/kyo-422-guard
clone_repo "$bare7" "$t7/workerB"
gh_ok_empty
run_check "$t7/workerB" 42
assert_exit "ticket 42 must NOT match branch kyo-422-guard" 0
run_check "$t7/workerB" 422
assert_exit "ticket 422 must match branch kyo-422-guard" 1
echo

# ─── Test 8: local-only work, never pushed (KYO-471 hole) ───────────────────
echo "-- Test 8: local-only work"
t8="$tmpdir/t8"
mkdir -p "$t8"
bare8="$(new_bare_remote "$t8/remote.git")"
seed_main "$bare8"
clone_repo "$bare8" "$t8/worker"
git -C "$t8/worker" checkout -q -b jason/kyo-422-guard
echo "local only, never pushed" >>"$t8/worker/README.md"
git -C "$t8/worker" commit -q -am "local work, never pushed"
git -C "$t8/worker" checkout -q main
gh_ok_empty
run_check "$t8/worker" 422
assert_exit "unpushed local branch is still in-flight work" 1
assert_contains "names the local branch" "local branch: jason/kyo-422-guard"
echo

# ─── Test 9: fail closed — gh broken ─────────────────────────────────────────
echo "-- Test 9: fail closed, gh broken"
t9="$tmpdir/t9"
mkdir -p "$t9"
bare9="$(new_bare_remote "$t9/remote.git")"
seed_main "$bare9"
clone_repo "$bare9" "$t9/workerB"
gh_fail "gh: authentication required (stub failure)"
run_check "$t9/workerB" 422
assert_exit "a failing gh must exit 3, never 0" 3
assert_contains "names the failing check" "gh pr list"
gh_ok_empty
echo

# ─── Test 10: fail closed — remote unreachable ───────────────────────────────
echo "-- Test 10: fail closed, remote unreachable"
t10="$tmpdir/t10"
mkdir -p "$t10"
bare10="$(new_bare_remote "$t10/remote.git")"
seed_main "$bare10"
clone_repo "$bare10" "$t10/workerB"
gh_ok_empty
run_check "$t10/workerB" 422 --remote "$t10/does-not-exist.git"
assert_exit "an unreachable remote must exit 3, never 0" 3
assert_contains "names the failing check" "remote branches"
echo

# ─── Test 11: usage errors ───────────────────────────────────────────────────
echo "-- Test 11: usage"
if out="$(cd "$tmpdir" && "$SCRIPT" 2>&1)"; then
    CHECK_STATUS=0
else
    CHECK_STATUS=$?
fi
CHECK_OUTPUT="$out"
assert_exit "no argument" 2
run_check "$tmpdir" "not-a-ticket"
assert_exit "unparseable ticket" 2
PR_LIST_LIMIT="not-a-number" run_check "$tmpdir" 422
assert_exit "non-numeric PR_LIST_LIMIT override" 2
PR_LIST_LIMIT=0 run_check "$tmpdir" 422
assert_exit "zero PR_LIST_LIMIT override" 2
echo

# ─── Test 12: input forms — KYO-422 / kyo-422 / 422 all identical ───────────
echo "-- Test 12: input forms"
t12="$tmpdir/t12"
mkdir -p "$t12"
bare12="$(new_bare_remote "$t12/remote.git")"
seed_main "$bare12"
clone_repo "$bare12" "$t12/workerA"
git -C "$t12/workerA" checkout -q -b jason/kyo-422-guard
echo "x" >>"$t12/workerA/README.md"
git -C "$t12/workerA" commit -q -am "start"
git -C "$t12/workerA" push -q origin jason/kyo-422-guard
clone_repo "$bare12" "$t12/workerB"
gh_ok_empty
for form in KYO-422 kyo-422 422; do
    run_check "$t12/workerB" "$form"
    assert_exit "input form '$form' behaves identically" 1
done
echo

# ─── Test 13: fail closed — a PR listing that may be truncated ──────────────
# THE FAIL-CLOSED PROOF for `gh pr list --limit`. The script shipped with a
# hardcoded --limit 200 against a repo that already had 411 PRs, so every
# duplicate older than PR #212 was invisible and reported CLEAR — the KYO-511
# fail-open species by another route. A bigger number alone would not have
# caught it, so the guard is what is under test here: exactly PR_LIST_LIMIT
# rows come back, NONE of them matching the ticket, so an implementation that
# ignored the row count would confidently say "clear" (exit 0). It must say
# "could not complete this check" (exit 3) instead.
echo "-- Test 13: fail closed, PR listing at PR_LIST_LIMIT"
t13="$tmpdir/t13"
mkdir -p "$t13"
bare13="$(new_bare_remote "$t13/remote.git")"
seed_main "$bare13"
clone_repo "$bare13" "$t13/workerB"
gh_ok_prs_n 3
PR_LIST_LIMIT=3 run_check "$t13/workerB" 422
assert_exit "a listing at the limit must exit 3, never 0" 3
assert_contains "names the limit it hit" "PR_LIST_LIMIT of 3"
assert_contains "tells the operator to raise it" "re-run with a higher PR_LIST_LIMIT"
echo

# ─── Test 14: just under the limit, no match — the guard must not fire ──────
echo "-- Test 14: just under PR_LIST_LIMIT, no match"
t14="$tmpdir/t14"
mkdir -p "$t14"
bare14="$(new_bare_remote "$t14/remote.git")"
seed_main "$bare14"
clone_repo "$bare14" "$t14/workerB"
gh_ok_prs_n 2
PR_LIST_LIMIT=3 run_check "$t14/workerB" 422
assert_exit "a complete listing with no match is still CLEAR" 0
assert_not_contains "does not claim truncation" "PR_LIST_LIMIT of 3"
echo

# ─── Test 15: just under the limit, with a match — real verdict survives ────
echo "-- Test 15: just under PR_LIST_LIMIT, with a match"
t15="$tmpdir/t15"
mkdir -p "$t15"
bare15="$(new_bare_remote "$t15/remote.git")"
seed_main "$bare15"
clone_repo "$bare15" "$t15/workerB"
gh_ok_prs_n 1 "$(pr_row 503 OPEN "$POST_RESTART_TS" jason/kyo-422-guard)"
PR_LIST_LIMIT=3 run_check "$t15/workerB" 422
assert_exit "a complete listing with a match is IN FLIGHT" 1
assert_contains "names the matching PR" "PR #503 (OPEN) branch jason/kyo-422-guard"
echo

gh_ok_empty

# ─── Tests 16-20: STRANDED.md tombstones (KYO-529) ───────────────────────────
#
# check 3 (local worktrees) had NO coverage anywhere in this file before
# KYO-529 — every test above exercises checks 1/2/4 against plain clones,
# never `git worktree add`. These tests are therefore the first ones to
# exercise check 3 at all, tombstoned or not (see the KYO-529 dispatch
# report for this note; the ticket asked it to be called out explicitly).
#
# The marker itself is written with the real mark-worktree-stranded.sh
# (except test 18, which needs a marker that deliberately does NOT name the
# ticket) so these tests prove the two scripts actually interoperate, not
# just that check-ticket-in-flight.sh can parse a hand-crafted fixture.
MARK="$SCRIPT_DIR/mark-worktree-stranded.sh"

# ─── Test 16: tombstoned local worktree is the only hit → CLEAR ─────────────
echo "-- Test 16: tombstoned local worktree, only hit"
t16="$tmpdir/t16"
mkdir -p "$t16"
bare16="$(new_bare_remote "$t16/remote.git")"
seed_main "$bare16"
clone_repo "$bare16" "$t16/workerB"
git -C "$t16/workerB" worktree add -q -b jason/kyo-422-dead "$t16/wt-dead"
"$MARK" 422 --worktree "$t16/wt-dead" >/dev/null
gh_ok_empty
run_check "$t16/workerB" 422
assert_exit "a tombstoned worktree with no other evidence is CLEAR" 0
assert_contains "still names the preserved path" "$t16/wt-dead"
assert_contains "under the preserved-worktrees heading" "PRESERVED STRANDED WORK"
assert_not_contains "does not also list it as a hit" "local worktree at $t16/wt-dead"
echo

# ─── Test 17: an UNtombstoned local worktree is still a normal hit ─────────
# The fail-closed default from KYO-471 must be completely unchanged for a
# worktree that carries no marker at all.
echo "-- Test 17: untombstoned local worktree, only hit"
t17="$tmpdir/t17"
mkdir -p "$t17"
bare17="$(new_bare_remote "$t17/remote.git")"
seed_main "$bare17"
clone_repo "$bare17" "$t17/workerB"
git -C "$t17/workerB" worktree add -q -b jason/kyo-422-live "$t17/wt-live"
gh_ok_empty
run_check "$t17/workerB" 422
assert_exit "an untombstoned worktree is IN FLIGHT, unchanged from KYO-471" 1
assert_contains "names the worktree hit" "local worktree at $t17/wt-live"
echo

# ─── Test 18: STRANDED.md that does NOT name the ticket is not honoured ────
echo "-- Test 18: tombstone naming a different ticket is not honoured"
t18="$tmpdir/t18"
mkdir -p "$t18"
bare18="$(new_bare_remote "$t18/remote.git")"
seed_main "$bare18"
clone_repo "$bare18" "$t18/workerB"
git -C "$t18/workerB" worktree add -q -b jason/kyo-422-wrongkey "$t18/wt-wrongkey"
printf '# STRANDED WORKTREE — KYO-999\n\nThis names a different ticket entirely.\n' >"$t18/wt-wrongkey/STRANDED.md"
gh_ok_empty
run_check "$t18/workerB" 422
assert_exit "a marker naming a different ticket is IN FLIGHT" 1
assert_contains "names the worktree hit" "local worktree at $t18/wt-wrongkey"
echo

# ─── Test 19: tombstoned worktree, but its branch is ALSO pushed to remote ─
# Published work is still in flight — the tombstone must only ever suppress
# LOCAL evidence, never a remote-branch hit.
echo "-- Test 19: tombstoned worktree with a matching remote branch"
t19="$tmpdir/t19"
mkdir -p "$t19"
bare19="$(new_bare_remote "$t19/remote.git")"
seed_main "$bare19"
clone_repo "$bare19" "$t19/workerB"
git -C "$t19/workerB" worktree add -q -b jason/kyo-422-published "$t19/wt-published"
echo "published work" >>"$t19/wt-published/README.md"
git -C "$t19/wt-published" commit -q -am "published before the run died"
git -C "$t19/wt-published" push -q origin jason/kyo-422-published
"$MARK" 422 --worktree "$t19/wt-published" >/dev/null
gh_ok_empty
run_check "$t19/workerB" 422
assert_exit "a tombstone must not mask a matching remote branch" 1
assert_contains "names the remote branch hit" "remote branch: origin/jason/kyo-422-published"
assert_contains "still reports the preserved worktree" "PRESERVED STRANDED WORK"
echo

# ─── Test 20: tombstoned worktree, but its branch ALSO has an open PR ──────
echo "-- Test 20: tombstoned worktree with a matching open PR"
t20="$tmpdir/t20"
mkdir -p "$t20"
bare20="$(new_bare_remote "$t20/remote.git")"
seed_main "$bare20"
clone_repo "$bare20" "$t20/workerB"
git -C "$t20/workerB" worktree add -q -b jason/kyo-422-haspr "$t20/wt-haspr"
"$MARK" 422 --worktree "$t20/wt-haspr" >/dev/null
gh_ok_prs "$(pr_row 504 OPEN "$POST_RESTART_TS" jason/kyo-422-haspr)"
run_check "$t20/workerB" 422
assert_exit "a tombstone must not mask a matching open PR" 1
assert_contains "names the PR hit" "PR #504 (OPEN) branch jason/kyo-422-haspr"
assert_contains "still reports the preserved worktree" "PRESERVED STRANDED WORK"
echo

gh_ok_empty

# ─── Tests 21-25: `stranded/` REMOTE/LOCAL BRANCH tombstones (KYO-567) ──────
#
# KYO-529's tombstone (STRANDED.md) is local-only evidence and must never
# suppress a remote branch or PR hit — tests 16-20 above pin that. KYO-567's
# tombstone is different: `stranded/<branch>` is a rename ON THE REMOTE
# ITSELF (written by mark-branch-stranded.sh), so it IS allowed to suppress
# a remote-branch hit (check 1) and the symmetric local-branch hit
# (check 4) — but, same as the worktree tombstone, it must never suppress a
# PR hit. Test 23 is the KYO-567 analogue of tests 19/20 above.
MARK_BRANCH="$SCRIPT_DIR/mark-branch-stranded.sh"

# ─── Test 21: plain pushed branch, no PR — still IN FLIGHT (unchanged) ─────
# Pins the distinction the ticket calls out explicitly: this ticket relaxes
# nothing about the claim guard itself, only adds a new way to retire a
# claim. A bare pushed branch with no PR (the exact KYO-534/KYO-463 shape,
# before release) must still block a second worker. Test 2 above already
# exercises this path end to end (that's the KYO-422 acceptance criterion);
# this test exists only to make the "unchanged" claim explicit and pinned
# under its own name, right next to the tests that show what DOES change.
echo "-- Test 21: pushed branch with no PR is still IN FLIGHT (unchanged by KYO-567)"
t21="$tmpdir/t21"
mkdir -p "$t21"
bare21="$(new_bare_remote "$t21/remote.git")"
seed_main "$bare21"
clone_repo "$bare21" "$t21/workerA"
git -C "$t21/workerA" checkout -q -b jason/kyo-422-nopr
echo "died before opening a PR" >>"$t21/workerA/README.md"
git -C "$t21/workerA" commit -q -am "workerA: died before PR"
git -C "$t21/workerA" push -q origin jason/kyo-422-nopr
clone_repo "$bare21" "$t21/workerB"
gh_ok_empty
run_check "$t21/workerB" 422
assert_exit "an un-tombstoned pushed branch with no PR still blocks" 1
assert_contains "names the remote branch" "remote branch: origin/jason/kyo-422-nopr"
echo

# ─── Test 22: matching remote branch under stranded/ → CLEAR ───────────────
echo "-- Test 22: stranded/ remote branch is preserved, not a hit"
t22="$tmpdir/t22"
mkdir -p "$t22"
bare22="$(new_bare_remote "$t22/remote.git")"
seed_main "$bare22"
clone_repo "$bare22" "$t22/helper"
git -C "$t22/helper" push -q origin "main:refs/heads/stranded/jason/kyo-422-old"
clone_repo "$bare22" "$t22/workerB"
gh_ok_empty
run_check "$t22/workerB" 422
assert_exit "a stranded/ remote branch alone is CLEAR" 0
assert_contains "under the preserved-work heading" "PRESERVED STRANDED WORK"
assert_contains "names the stranded ref and what it tombstones" "remote branch origin/stranded/jason/kyo-422-old (was origin/jason/kyo-422-old)"
assert_not_contains "does not also count it as a hit" "remote branch: origin/stranded/jason/kyo-422-old"
echo

# ─── Test 23: stranded/ remote branch must NOT mask a matching PR ─────────
# The KYO-567 analogue of tests 19/20: this tombstone type IS allowed to
# suppress a remote-branch hit, but like every tombstone in this script it
# must never suppress a PR hit (check 2) — a PR has a live consumer
# (/merge-sweeper) that no tombstone substitutes for.
echo "-- Test 23: stranded/ remote branch does not mask a matching PR"
t23="$tmpdir/t23"
mkdir -p "$t23"
bare23="$(new_bare_remote "$t23/remote.git")"
seed_main "$bare23"
clone_repo "$bare23" "$t23/helper"
git -C "$t23/helper" push -q origin "main:refs/heads/stranded/jason/kyo-422-haspr"
clone_repo "$bare23" "$t23/workerB"
gh_ok_prs "$(pr_row 505 OPEN "$POST_RESTART_TS" jason/kyo-422-haspr)"
run_check "$t23/workerB" 422
assert_exit "a tombstoned remote branch must not mask a matching PR" 1
assert_contains "names the PR hit" "PR #505 (OPEN) branch jason/kyo-422-haspr"
assert_contains "still reports the preserved remote branch" "PRESERVED STRANDED WORK"
echo
gh_ok_empty

# ─── Test 24: local branch under stranded/ → CLEAR ─────────────────────────
echo "-- Test 24: stranded/ local branch is preserved, not a hit"
t24="$tmpdir/t24"
mkdir -p "$t24"
bare24="$(new_bare_remote "$t24/remote.git")"
seed_main "$bare24"
clone_repo "$bare24" "$t24/workerB"
git -C "$t24/workerB" branch "stranded/jason/kyo-422-locallystranded"
gh_ok_empty
run_check "$t24/workerB" 422
assert_exit "a stranded/ local branch alone is CLEAR" 0
assert_contains "under the preserved-work heading" "PRESERVED STRANDED WORK"
assert_contains "names the stranded local branch and what it tombstones" "local branch stranded/jason/kyo-422-locallystranded (was jason/kyo-422-locallystranded)"
assert_not_contains "does not also count it as a hit" "local branch: stranded/jason/kyo-422-locallystranded"
echo

# ─── Test 25: interop — a ref mark-branch-stranded.sh actually created ────
# Mirrors mark-worktree-stranded-test.sh's own interop test: this script
# and check-ticket-in-flight.sh must agree on the marker format because one
# writes it and the other reads it (KYO-422 principle). Released from a
# THIRD clone that never checked out the ticket branch at all, since
# mark-branch-stranded.sh refuses to touch a branch checked out elsewhere
# without its own worktree tombstone — that refusal is covered by
# mark-branch-stranded-test.sh, not here.
echo "-- Test 25: interop with the real mark-branch-stranded.sh"
t25="$tmpdir/t25"
mkdir -p "$t25"
bare25="$(new_bare_remote "$t25/remote.git")"
seed_main "$bare25"
clone_repo "$bare25" "$t25/workerA"
git -C "$t25/workerA" checkout -q -b jason/kyo-422-interop
echo "died mid-run" >>"$t25/workerA/README.md"
git -C "$t25/workerA" commit -q -am "workerA: died before PR"
git -C "$t25/workerA" push -q origin jason/kyo-422-interop
clone_repo "$bare25" "$t25/releaser"
gh_ok_empty
if release_out="$(cd "$t25/releaser" && "$MARK_BRANCH" 422 --branch jason/kyo-422-interop 2>&1)"; then
    release_status=0
else
    release_status=$?
fi
if [ "$release_status" -eq 0 ]; then
    printf "  \xe2\x9c\x93 %s\n" "mark-branch-stranded.sh releases the branch"
    PASS=$((PASS + 1))
else
    printf "  \xe2\x9c\x97 %s\n" "mark-branch-stranded.sh releases the branch"
    printf '    %s\n' "$release_out" | sed 's/^/    | /'
    FAIL=$((FAIL + 1))
fi
clone_repo "$bare25" "$t25/workerC"
run_check "$t25/workerC" 422
assert_exit "check-ticket-in-flight.sh honours the ref mark-branch-stranded.sh wrote" 0
assert_contains "names the stranded ref it wrote" "remote branch origin/stranded/jason/kyo-422-interop (was origin/jason/kyo-422-interop)"
echo

gh_ok_empty

# ─── Tests 26-31: `--self` cwd-independent self-exclusion (KYO-593) ─────────
#
# Test 26 is the KYO-593 acceptance criterion, built as ONE fixture exercised
# twice: first without --self (reproducing the bug's original symptom —
# pins that the fixture really does reproduce cwd-dependence), then with
# --self against the identical, unmodified fixture (proving the flag, not a
# setup difference, is what flips the verdict to CLEAR). Test 27 then adds a
# second worker's worktree to the SAME fixture and shows duplicate detection
# still fires even with --self set, which is the property that makes --self
# safe to ship: it excludes nothing beyond the one branch it is told about.

# ─── Test 26: cwd-independence — the bug reproduced, then fixed by --self ──
echo "-- Test 26: --self makes the check cwd-independent (KYO-593 acceptance criterion)"
t26="$tmpdir/t26"
mkdir -p "$t26"
bare26="$(new_bare_remote "$t26/remote.git")"
seed_main "$bare26"
clone_repo "$bare26" "$t26/canonical"
# The caller's own ticket work lives in a LINKED WORKTREE, not in
# $t26/canonical itself. $t26/canonical stays on "main" throughout this
# test — simulating a caller whose shell cwd is the canonical clone while
# its own finished implementation lives in a worktree elsewhere. That is
# the exact KYO-593 shape: CURRENT_BRANCH resolves to "main" (already
# discarded by the script) rather than to the caller's own branch, so cwd-
# derived self-exclusion contributes nothing here.
git -C "$t26/canonical" worktree add -q -b jason/kyo-422-ownwork "$t26/wt-own"
echo "own work" >>"$t26/wt-own/README.md"
git -C "$t26/wt-own" commit -q -am "own work for KYO-422"
gh_ok_empty

# Without --self: reproduces the original bug from this exact fixture.
run_check "$t26/canonical" 422
assert_exit "without --self, cwd-dependence reproduces the bug: own worktree reported IN FLIGHT" 1
assert_contains "names the caller's own worktree as a hit" "local worktree at $t26/wt-own (branch jason/kyo-422-ownwork)"
# The HINT is the only thing that tells a human reader this verdict may be
# about their own work. It is advisory — it changes no exit code and no
# hit/tombstone classification — but it is the signpost out of the KYO-593
# trap, so it is asserted in both directions: present here, and absent in
# test 27 where --self was passed and the hint would be wrong.
assert_contains "prints the --self HINT when --self was not passed" "HINT: if one of the above is your own branch"

# Same fixture, unmodified — only the flag differs.
run_check "$t26/canonical" 422 --self jason/kyo-422-ownwork
assert_exit "--self jason/kyo-422-ownwork makes it CLEAR from the same cwd" 0
assert_not_contains "does not name the caller's own worktree" "$t26/wt-own"
echo

# ─── Test 27: duplicate detection survives --self ────────────────────────
echo "-- Test 27: --self does not blind the check to a genuine second worker"
# Builds on test 26's fixture: a second worktree/branch for the SAME ticket
# appears, belonging to another worker. This is the criterion that proves
# the fix did not weaken the check the script exists for — --self excludes
# only the one branch it names.
git -C "$t26/canonical" worktree add -q -b jason/kyo-422-otherworker "$t26/wt-other"
echo "other worker's work" >>"$t26/wt-other/README.md"
git -C "$t26/wt-other" commit -q -am "other worker's work for KYO-422"
run_check "$t26/canonical" 422 --self jason/kyo-422-ownwork
assert_exit "a second worker's worktree for the same ticket still blocks, even with --self" 1
assert_contains "names the other worker's worktree" "local worktree at $t26/wt-other (branch jason/kyo-422-otherworker)"
assert_not_contains "still does not name the caller's own worktree" "$t26/wt-own"
assert_not_contains "suppresses the --self HINT when --self was already passed" "HINT: if one of the above is your own branch"
echo

# ─── Test 28: --self naming a branch that doesn't match the ticket ─────────
echo "-- Test 28: --self naming a branch that doesn't match the ticket is a usage error"
t28="$tmpdir/t28"
mkdir -p "$t28"
bare28="$(new_bare_remote "$t28/remote.git")"
seed_main "$bare28"
clone_repo "$bare28" "$t28/workerB"
gh_ok_empty
run_check "$t28/workerB" 422 --self jason/kyo-999-unrelated
assert_exit "--self naming a branch for a different ticket is a usage error" 2
echo

# ─── Test 29: --self passed twice ────────────────────────────────────────
echo "-- Test 29: --self passed twice is a usage error"
t29="$tmpdir/t29"
mkdir -p "$t29"
bare29="$(new_bare_remote "$t29/remote.git")"
seed_main "$bare29"
clone_repo "$bare29" "$t29/workerB"
gh_ok_empty
run_check "$t29/workerB" 422 --self jason/kyo-422-a --self jason/kyo-422-b
assert_exit "a second --self is a usage error" 2
echo

# ─── Test 30: --self with no following value ────────────────────────────
echo "-- Test 30: --self with no value is a usage error"
t30="$tmpdir/t30"
mkdir -p "$t30"
bare30="$(new_bare_remote "$t30/remote.git")"
seed_main "$bare30"
clone_repo "$bare30" "$t30/workerB"
gh_ok_empty
run_check "$t30/workerB" 422 --self
assert_exit "--self with no following value is a usage error" 2
echo

# ─── Test 31: fail-closed preserved — --self must not turn exit 3 into 0 ──
echo "-- Test 31: --self must not turn a fail-closed exit 3 into exit 0"
t31="$tmpdir/t31"
mkdir -p "$t31"
bare31="$(new_bare_remote "$t31/remote.git")"
seed_main "$bare31"
clone_repo "$bare31" "$t31/canonical"
git -C "$t31/canonical" worktree add -q -b jason/kyo-422-ownwork "$t31/wt-own"
gh_fail "gh: authentication required (stub failure)"
run_check "$t31/canonical" 422 --self jason/kyo-422-ownwork
assert_exit "a failing gh still exits 3 even with --self naming the caller's own branch" 3
gh_ok_empty
echo

# ─── Tests 32-39: recycled ticket keys (KYO-607) ─────────────────────────────
#
# Trakkt's ticket-key numbering restarted in May 2026, so nine keys are shared
# between a retired ticket and a current one. Every one of those keys had a
# MERGED pre-restart PR *and* that PR's surviving remote branch, so the gate
# reported IN FLIGHT on them forever while the current ticket sat in Backlog
# looking available — permanently unclaimable.
#
# These tests use ticket 299 (the real KYO-299 → PR #37 +
# origin/jason/kyo-299-add-get_catalog_stats-server-fn-for-datasource-catalog-tab
# shape, shortened) so the fixtures read as the case they were written for.
# Everything below straddles the script's DEFAULT KEY_RESTART_CUTOFF, except
# test 35, which overrides it — the override is what proves the constant is
# actually consulted rather than the classification falling out of some other
# property of the fixtures.
RECYCLED_HEADING="PRE-RESTART KEY REUSE"

# ─── Test 32: THE KYO-607 ACCEPTANCE CRITERION ──────────────────────────────
# A pre-restart merged PR AND its surviving remote branch — the exact shape
# all nine collisions have — must together yield CLEAR, with both still
# printed. Before the fix this fixture was exit 1, twice over.
echo "-- Test 32: pre-restart merged PR + its remote branch (KYO-607 acceptance criterion)"
t32="$tmpdir/t32"
mkdir -p "$t32"
bare32="$(new_bare_remote "$t32/remote.git")"
seed_main "$bare32"
clone_repo "$bare32" "$t32/helper"
git -C "$t32/helper" push -q origin main:refs/heads/jason/kyo-299-old-numbering
clone_repo "$bare32" "$t32/workerB"
gh_ok_prs "$(pr_row 37 MERGED "$PRE_RESTART_TS" jason/kyo-299-old-numbering)"
run_check "$t32/workerB" 299
assert_exit "a pre-restart PR and its remote branch are together CLEAR" 0
assert_contains "under the pre-restart heading" "$RECYCLED_HEADING"
assert_contains "still prints the PR, with its creation date" "PR #37 (MERGED) branch jason/kyo-299-old-numbering — created $PRE_RESTART_TS"
assert_contains "still prints the remote branch" "remote branch origin/jason/kyo-299-old-numbering"
# Hits print with a "  - " bullet, classified entries with "  ~ " — asserting
# on the bullet is what distinguishes "reported under the right heading" from
# "reported at all", which is the entire behaviour change here.
assert_not_contains "does not count the PR as a hit" "  - PR #37 (MERGED)"
assert_not_contains "does not count the remote branch as a hit" "remote branch: origin/jason/kyo-299-old-numbering"
assert_contains "verdict is CLEAR" "RESULT: CLEAR"
echo

# ─── Test 33: the same classification reaches checks 3 and 4 ────────────────
# The pre-restart decision can only be made by check 2 (PRs carry the date),
# so checks 1/3/4 read it out of RECYCLED_BRANCHES. Test 32 covered check 1;
# this covers the local worktree (check 3) and local branch (check 4) a
# retired-numbering run can leave behind on a machine. Both entries are
# expected — before the fix they were two separate HITS, and classifying
# rather than suppressing keeps the entry count identical.
echo "-- Test 33: pre-restart classification reaches the local worktree and branch checks"
t33="$tmpdir/t33"
mkdir -p "$t33"
bare33="$(new_bare_remote "$t33/remote.git")"
seed_main "$bare33"
clone_repo "$bare33" "$t33/workerB"
git -C "$t33/workerB" worktree add -q -b jason/kyo-299-old-numbering "$t33/wt-old"
gh_ok_prs "$(pr_row 37 MERGED "$PRE_RESTART_TS" jason/kyo-299-old-numbering)"
run_check "$t33/workerB" 299
assert_exit "a local worktree and branch for a pre-restart key are CLEAR" 0
assert_contains "reports the worktree as pre-restart" "local worktree $t33/wt-old (branch jason/kyo-299-old-numbering)"
assert_contains "reports the local branch as pre-restart" "local branch jason/kyo-299-old-numbering"
assert_not_contains "does not count the worktree as a hit" "local worktree at $t33/wt-old"
assert_not_contains "does not count the local branch as a hit" "local branch: jason/kyo-299-old-numbering"
echo

# ─── Test 34: THE PRECISION TEST — the real KYO-293/294 shape ──────────────
# Keys 293 and 294 have BOTH a pre-restart PR (#5, #36) and a legitimate
# current-numbering PR from August (#321, #322). The fix must drop the old one
# and keep the new one. A fix that keyed off "this ticket number is one of the
# nine" — or off anything coarser than the individual PR's date — would wrongly
# clear these, which is the failure mode this test exists to catch.
echo "-- Test 34: pre-restart AND current-numbering PRs on one key (KYO-293/294 shape)"
t34="$tmpdir/t34"
mkdir -p "$t34"
bare34="$(new_bare_remote "$t34/remote.git")"
seed_main "$bare34"
clone_repo "$bare34" "$t34/workerB"
gh_ok_prs \
    "$(pr_row 5 MERGED "$PRE_RESTART_TS" jason/kyo-293-old-numbering)" \
    "$(pr_row 321 OPEN "$POST_RESTART_TS" jason/kyo-293-real-work)"
run_check "$t34/workerB" 293
assert_exit "the current-numbering PR still blocks the ticket" 1
assert_contains "lists ONLY the current-numbering PR as a hit" "  - PR #321 (OPEN) branch jason/kyo-293-real-work"
assert_contains "still reports the pre-restart PR, classified" "PR #5 (MERGED) branch jason/kyo-293-old-numbering — created $PRE_RESTART_TS"
assert_not_contains "does not count the pre-restart PR as a hit" "  - PR #5 (MERGED)"
echo

# ─── Test 35: a post-cutoff PR alone is unchanged; the boundary is exclusive ─
# Guards against over-broad matching in the other direction: nothing about
# this change may relax the gate for a current-numbering PR. Run three ways
# from one fixture — a PR safely after the cutoff, a PR created at EXACTLY the
# cutoff instant (the comparison is strictly-before, so this is NOT
# pre-restart), and the same PR under an overridden cutoff that puts it in the
# past. The third is the positive control: it proves KEY_RESTART_CUTOFF is
# genuinely what the classification reads.
echo "-- Test 35: post-cutoff PR alone still blocks; cutoff boundary is exclusive"
t35="$tmpdir/t35"
mkdir -p "$t35"
bare35="$(new_bare_remote "$t35/remote.git")"
seed_main "$bare35"
clone_repo "$bare35" "$t35/workerB"
gh_ok_prs "$(pr_row 321 OPEN "$POST_RESTART_TS" jason/kyo-299-real-work)"
run_check "$t35/workerB" 299
assert_exit "a post-cutoff PR alone is IN FLIGHT, unchanged" 1
assert_not_contains "nothing is classified as pre-restart" "$RECYCLED_HEADING"

gh_ok_prs "$(pr_row 322 OPEN '2026-05-12T00:00:00Z' jason/kyo-299-exactly-at-cutoff)"
run_check "$t35/workerB" 299
assert_exit "a PR created exactly AT the cutoff is not pre-restart" 1
assert_contains "counts the at-cutoff PR as a hit" "  - PR #322 (OPEN) branch jason/kyo-299-exactly-at-cutoff"

gh_ok_prs "$(pr_row 321 OPEN "$POST_RESTART_TS" jason/kyo-299-real-work)"
KEY_RESTART_CUTOFF='2026-09-01T00:00:00Z' run_check "$t35/workerB" 299
assert_exit "the same PR IS pre-restart under a later cutoff override" 0
assert_contains "names the overridden cutoff it was compared against" "before the 2026-09-01T00:00:00Z key restart"
echo

# ─── Test 36: fail-closed — gh broken while a pre-restart branch exists ─────
# The whole classification depends on check 2 having succeeded. If `gh` fails,
# RECYCLED_BRANCHES is empty, so the remote branch stays a HIT and FAILURES
# forces exit 3 exactly as before. A broken PR listing must never launder a
# branch into "recycled" (KYO-511's rule, unchanged by KYO-607).
echo "-- Test 36: fail closed — a failing gh cannot launder a branch into pre-restart"
t36="$tmpdir/t36"
mkdir -p "$t36"
bare36="$(new_bare_remote "$t36/remote.git")"
seed_main "$bare36"
clone_repo "$bare36" "$t36/helper"
git -C "$t36/helper" push -q origin main:refs/heads/jason/kyo-299-old-numbering
clone_repo "$bare36" "$t36/workerB"
gh_fail "gh: authentication required (stub failure)"
run_check "$t36/workerB" 299
assert_exit "a failing gh with a pre-restart remote branch present still exits 3" 3
assert_not_contains "classifies nothing as pre-restart" "$RECYCLED_HEADING"
gh_ok_empty
echo

# ─── Test 37: a malformed KEY_RESTART_CUTOFF is a usage error ──────────────
# The ordering compare is only valid between two strings of the canonical
# `gh` shape. A cutoff that isn't one cannot be compared safely, and silently
# continuing would classify either every PR or no PR as pre-restart — so it
# exits 2 rather than guessing.
echo "-- Test 37: malformed KEY_RESTART_CUTOFF is a usage error"
t37="$tmpdir/t37"
mkdir -p "$t37"
bare37="$(new_bare_remote "$t37/remote.git")"
seed_main "$bare37"
clone_repo "$bare37" "$t37/workerB"
gh_ok_empty
for bad in "not-a-timestamp" "2026-05-12" "2026-05-12T00:00:00+00:00" "2026-05-12T00:00:00.000Z" "26-05-12T00:00:00Z" "2026-13-99T00:00:00Z junk"; do
    KEY_RESTART_CUTOFF="$bad" run_check "$t37/workerB" 299
    assert_exit "KEY_RESTART_CUTOFF='$bad' is a usage error" 2
done
# An EXPLICITLY EMPTY override is not an error — it falls back to the built-in
# default, the same `${VAR:-default}` semantics PR_LIST_LIMIT already has.
# Pinned so the distinction between "unset/empty" and "set to nonsense" can't
# be lost in a later refactor.
KEY_RESTART_CUTOFF="" run_check "$t37/workerB" 299
assert_exit "an empty KEY_RESTART_CUTOFF falls back to the default, not an error" 0
echo

# ─── Test 38: an unreadable createdAt fails CLOSED, not open ───────────────
# The asymmetry from the header applies here too: a false "clear" costs a
# duplicate implementation, a false "in flight" costs one work cycle. So a PR
# row whose date cannot be read is treated as NOT pre-restart and keeps
# blocking. Covers both an empty createdAt and a non-empty but unparseable
# one.
echo "-- Test 38: a PR with an empty or unparseable createdAt still blocks"
t38="$tmpdir/t38"
mkdir -p "$t38"
bare38="$(new_bare_remote "$t38/remote.git")"
seed_main "$bare38"
clone_repo "$bare38" "$t38/workerB"
gh_ok_prs "$(pr_row 39 MERGED '' jason/kyo-299-no-date)"
run_check "$t38/workerB" 299
assert_exit "an empty createdAt is not pre-restart — it stays a hit" 1
assert_contains "counts it as a hit" "  - PR #39 (MERGED) branch jason/kyo-299-no-date"
assert_not_contains "does not classify it as pre-restart" "$RECYCLED_HEADING"

gh_ok_prs "$(pr_row 40 MERGED 'yesterday' jason/kyo-299-junk-date)"
run_check "$t38/workerB" 299
assert_exit "an unparseable createdAt is not pre-restart — it stays a hit" 1
assert_contains "counts it as a hit" "  - PR #40 (MERGED) branch jason/kyo-299-junk-date"
assert_not_contains "does not classify it as pre-restart" "$RECYCLED_HEADING"
gh_ok_empty
echo

# ─── Test 39: a PR row that cannot be split into four fields exits 3 ────────
# The reason check 2 stopped splitting rows with `IFS=$'\t' read` (KYO-607):
# tab is an IFS *whitespace* character, so that form collapses `a\tb\t\td` to
# three fields and silently moved the branch name into the createdAt slot,
# skipping the PR entirely — a fail-OPEN. The row-splitting is now exact, and
# a row it genuinely cannot read is a check it could not complete, so it exits
# 3 like any other incomplete check rather than being skipped.
#
# Test 38's empty-createdAt row is the one that used to hit this path by
# accident; the rows here are short and long by construction. Both use a
# BRANCH THAT MATCHES the ticket where a branch is present, so an
# implementation that skipped the row would report CLEAR — the fail-open this
# pins shut.
echo "-- Test 39: an unreadable PR row exits 3, it is not skipped"
t39="$tmpdir/t39"
mkdir -p "$t39"
bare39="$(new_bare_remote "$t39/remote.git")"
seed_main "$bare39"
clone_repo "$bare39" "$t39/workerB"
gh_ok_prs "$(printf '41\tMERGED\tjason/kyo-299-three-fields-only')"
run_check "$t39/workerB" 299
assert_exit "a three-field row is a check that could not be completed" 3
assert_contains "says which row it could not read" "could not read row"

gh_ok_prs "$(printf '42\tMERGED\t%s\tjason/kyo-299-extra\tstray' "$POST_RESTART_TS")"
run_check "$t39/workerB" 299
assert_exit "a five-field row is a check that could not be completed" 3

gh_ok_prs "$(pr_row 43 MERGED "$POST_RESTART_TS" '')"
run_check "$t39/workerB" 299
assert_exit "a row with an empty headRefName is a check that could not be completed" 3
gh_ok_empty
echo

echo "Results: $PASS passed, $FAIL failed"
[ "$FAIL" -eq 0 ]
