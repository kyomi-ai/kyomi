#!/usr/bin/env bash
# ------------------------------------------------------------------------------
# scripts/sign-review-test.sh — self-test for sign-review.sh (KYO-712)
#
# Follows the same pattern as check-ticket-in-flight-test.sh and
# mark-worktree-stranded-test.sh: real throwaway git repos under a fresh
# mktemp -d, removed on exit via trap, branch names pinned explicitly to
# `main` (CI's `init.defaultBranch` is `master` on ubuntu-latest). No `gh`
# stub needed — sign-review.sh never calls `gh`.
#
# A throwaway Ed25519 keypair is generated fresh with `openssl genpkey` for
# this run only. The real reviewer key is never read from anywhere here.
#
# SCOPE: this suite tests scripts/sign-review.sh directly. It does NOT test
# .githooks/pre-commit's Check 2 / Check 2b narrowed-index blocking (the
# review_approval_allow_unstaged_reason() / unstaged_or_untracked_matching()
# helpers and their call sites), Check 3's signature verification, or
# review_approval_signature_valid() — that is scripts/pre-commit-hook-test.sh's
# job (KYO-712 Part E), which installs the real hook (with a throwaway
# public key substituted for the production one) via core.hooksPath in a
# throwaway repo, stubs scripts/lint/check-server-fns.sh,
# check-disposal-safety.sh, and check-real-identifiers.sh, and drives it
# through real `git commit` invocations. Before Part E, the hook's Check 2 /
# Check 2b narrowed-index behavior was UNTESTED by any automated suite —
# that gap is what let this ticket's own two 🔴 findings (the scan only
# running in the `else` branch, and the forgeable ALLOW-UNSTAGED
# acknowledgement) reach review undetected; see
# scripts/pre-commit-hook-test.sh's own header for how it closes it. This
# suite does still carry one crypto-level assertion that pins a
# sign-review.sh-only property without needing the hook at all (see Test 4):
# that the --allow-unstaged signature verifies over the combined
# hash+reason value and NOT over the hash alone.
#
# Cases (KYO-712 Part A added Tests 9-11 and inverted Test 8; Part C added
# the crypto-binding assertions inside Test 4 — see each test's own
# comment):
#   1. AC4 — narrowing a tracked, modified file between two sign calls
#   2. clean fully staged index signs and hashes correctly
#   3. empty staged diff is refused (pre-existing guard)
#   4. --allow-unstaged permits the narrowed sign, echoes both, and signs
#      the reason INTO the signature rather than as unsigned plain text
#      (KYO-712 Part C)
#   5. --allow-unstaged requires a non-empty reason
#   6. the one-positional-argument invocation still works (compatibility)
#   7. unstaged file outside the staged set still trips the guard
#   8. untracked, non-ignored files DO trip the guard (Part A)
#   9. a gitignored file does NOT trip the guard
#  10. new-file variant of the AC4 scenario: a newly-added file dropped
#      from the index (not a modification) is still refused
#  11. the refusal message distinguishes a modified-tracked file from an
#      untracked-new one when both are present
#
# Exit 0 = all pass, exit 1 = any failure.
# ------------------------------------------------------------------------------

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
SIGN="$SCRIPT_DIR/sign-review.sh"
PASS=0
FAIL=0

tmpdir="$(mktemp -d)"
trap 'rm -rf "$tmpdir"' EXIT

export GIT_AUTHOR_NAME="KYO-712 Test" GIT_AUTHOR_EMAIL="kyo712-test@kyomi.invalid"
export GIT_COMMITTER_NAME="KYO-712 Test" GIT_COMMITTER_EMAIL="kyo712-test@kyomi.invalid"

pass() {
    printf "  \xe2\x9c\x93 %s\n" "$1"
    PASS=$((PASS + 1))
}

fail() {
    printf "  \xe2\x9c\x97 %s\n" "$1"
    printf '    %s\n' "$2" | sed 's/^/    | /'
    FAIL=$((FAIL + 1))
}

# ─── throwaway Ed25519 keypair, generated fresh for this run ───────────────
KEY_PEM="$tmpdir/reviewer_key.pem"
PUB_PEM="$tmpdir/reviewer_pub.pem"
openssl genpkey -algorithm ed25519 -out "$KEY_PEM" >/dev/null 2>&1
openssl pkey -in "$KEY_PEM" -pubout -out "$PUB_PEM" >/dev/null 2>&1
PRIVATE_KEY="$(cat "$KEY_PEM")"

# ─── invocation helper, capturing exit code + combined output ───────────────
RUN_STATUS=""
RUN_OUTPUT=""
run_sign() {
    # run_sign <repo_dir> <arg>...  — invokes sign-review.sh with cwd set to
    # <repo_dir> (it operates on the current directory's git index and
    # writes .review-approval there), private key always as $1.
    local dir="$1"
    shift
    local out
    if out="$(cd "$dir" && "$SIGN" "$PRIVATE_KEY" "$@" 2>&1)"; then
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

# ─── real git repo helper ────────────────────────────────────────────────────
new_repo_with_files() {
    # new_repo_with_files <dir> <file>... -> a real repo on branch main with
    # one commit containing each named file (seeded content = filename).
    local dir="$1"
    shift
    git init -q -b main "$dir"
    local f
    for f in "$@"; do
        echo "seed: $f" >"$dir/$f"
    done
    git -C "$dir" add "$@"
    git -C "$dir" commit -q -m init
}

diff_cached_hash() {
    # diff_cached_hash <dir> -> sha256 of `git diff --cached` in <dir>,
    # matching exactly what sign-review.sh itself computes.
    git -C "$1" diff --cached | sha256sum | awk '{print $1}'
}

echo "Running sign-review self-tests..."
echo

# ─── Test 1: the exact ticket scenario (AC4) ────────────────────────────────
# Stage 3 files, sign (succeeds), unstage 2 of them, sign again — must fail
# and name the offending paths. This is the KYO-676 shape: a reviewer looked
# at N files, and by the time signing happens, fewer than N are staged.
echo "-- Test 1: AC4 — narrowing between two sign calls must be refused"
t1="$tmpdir/t1"
new_repo_with_files "$t1" a.txt b.txt c.txt
echo "change a" >>"$t1/a.txt"
echo "change b" >>"$t1/b.txt"
echo "change c" >>"$t1/c.txt"
git -C "$t1" add a.txt b.txt c.txt
run_sign "$t1"
assert_exit "first sign (fully staged) succeeds" 0
# Narrow the index: unstage b.txt and c.txt, leaving only a.txt staged —
# worktree content is untouched, exactly the KYO-676 " M" shape.
git -C "$t1" reset -q -- b.txt c.txt
run_sign "$t1"
assert_exit "second sign (narrowed index) is refused" 1
assert_contains "names b.txt as offending" "b.txt"
assert_contains "names c.txt as offending" "c.txt"
assert_contains "explains the KYO-676 shape" "KYO-676"
echo

# ─── Test 2: clean fully staged index signs and hashes correctly ───────────
echo "-- Test 2: clean fully staged index signs successfully"
t2="$tmpdir/t2"
new_repo_with_files "$t2" a.txt b.txt
echo "change a" >>"$t2/a.txt"
echo "change b" >>"$t2/b.txt"
git -C "$t2" add a.txt b.txt
run_sign "$t2"
assert_exit "signs successfully" 0
if [ -f "$t2/.review-approval" ]; then
    pass ".review-approval was written"
else
    fail ".review-approval was written" "not found at $t2/.review-approval"
fi
recorded_hash="$(sed -n '1p' "$t2/.review-approval" 2>/dev/null || echo MISSING)"
expected_hash="$(diff_cached_hash "$t2")"
if [ "$recorded_hash" = "$expected_hash" ]; then
    pass "recorded hash matches git diff --cached | sha256sum"
else
    fail "recorded hash matches git diff --cached | sha256sum" "recorded=$recorded_hash expected=$expected_hash"
fi
# The signature must actually verify against the public key — not just be
# present. A test that only checks the hash line would pass even if signing
# were silently broken.
#
# -rawin here matches scripts/sign-review.sh's own -rawin on the sign side
# (see that script's comment on its `pkeyutl -sign` line): required on
# OpenSSL 3.0.x, a no-op on versions where the flagless form already works.
sig_line="$(sed -n '2p' "$t2/.review-approval" 2>/dev/null || echo MISSING)"
hash_file="$tmpdir/t2_hash"
sig_file="$tmpdir/t2_sig"
printf -- '%s' "$expected_hash" >"$hash_file"
printf '%s' "$sig_line" | base64 -d >"$sig_file" 2>/dev/null || true
if openssl pkeyutl -verify -rawin -pubin -inkey "$PUB_PEM" -in "$hash_file" -sigfile "$sig_file" >/dev/null 2>&1; then
    pass "recorded signature cryptographically verifies against the public key"
else
    fail "recorded signature cryptographically verifies against the public key" "sig_line=$sig_line"
fi
echo

# ─── Test 3: empty index is still refused (pre-existing guard) ─────────────
echo "-- Test 3: empty staged diff is refused"
t3="$tmpdir/t3"
new_repo_with_files "$t3" a.txt
run_sign "$t3"
assert_exit "refuses an empty staged diff" 1
assert_contains "explains no staged changes" "No staged changes"
echo

# ─── Test 4: --allow-unstaged permits the narrowed sign and echoes both ────
echo "-- Test 4: --allow-unstaged permits + echoes paths and reason"
t4="$tmpdir/t4"
new_repo_with_files "$t4" a.txt b.txt
echo "change a" >>"$t4/a.txt"
echo "change b" >>"$t4/b.txt"
git -C "$t4" add a.txt b.txt
git -C "$t4" reset -q -- b.txt
run_sign "$t4" --allow-unstaged "b.txt reviewed separately in a follow-up pass"
assert_exit "signs with the escape hatch" 0
assert_contains "echoes the offending path" "b.txt"
assert_contains "echoes the supplied reason" "b.txt reviewed separately in a follow-up pass"
allow_line="$(sed -n '3p' "$t4/.review-approval" 2>/dev/null || echo MISSING)"
case "$allow_line" in
    "ALLOW-UNSTAGED:b.txt reviewed separately in a follow-up pass")
        pass ".review-approval line 3 records the acknowledgement"
        ;;
    *)
        fail ".review-approval line 3 records the acknowledgement" "$allow_line"
        ;;
esac
# KYO-712 Part C: the signature must cover the combined
# "<hash>\nALLOW-UNSTAGED:<reason>" value, not the hash alone — otherwise a
# line 3 hand-appended after the fact to a hash-only-signed approval would
# still verify, which is exactly the forgeable-acknowledgement gap this
# closes (see .githooks/pre-commit's review_approval_signature_valid() and
# scripts/pre-commit-hook-test.sh's Test 4 for the end-to-end,
# forged-line-3-is-blocked version of this same property). Prove both
# halves directly against the raw signature bytes: it verifies against the
# combined value, and it does NOT verify against the hash alone.
t4_hash="$(sed -n '1p' "$t4/.review-approval")"
t4_sig_line="$(sed -n '2p' "$t4/.review-approval")"
t4_sig_file="$tmpdir/t4_sig"
printf '%s' "$t4_sig_line" | base64 -d >"$t4_sig_file" 2>/dev/null || true
t4_combined_file="$tmpdir/t4_combined"
printf '%s\nALLOW-UNSTAGED:%s' "$t4_hash" "b.txt reviewed separately in a follow-up pass" >"$t4_combined_file"
if openssl pkeyutl -verify -rawin -pubin -inkey "$PUB_PEM" -in "$t4_combined_file" -sigfile "$t4_sig_file" >/dev/null 2>&1; then
    pass "signature verifies against the combined hash+reason value"
else
    fail "signature verifies against the combined hash+reason value" "hash=$t4_hash sig=$t4_sig_line"
fi
t4_hash_only_file="$tmpdir/t4_hash_only"
printf '%s' "$t4_hash" >"$t4_hash_only_file"
if openssl pkeyutl -verify -rawin -pubin -inkey "$PUB_PEM" -in "$t4_hash_only_file" -sigfile "$t4_sig_file" >/dev/null 2>&1; then
    fail "signature does NOT verify against the hash alone (proves line 3 is bound in)" "verified against hash alone unexpectedly"
else
    pass "signature does NOT verify against the hash alone (proves line 3 is bound in)"
fi
echo

# ─── Test 5: --allow-unstaged with a missing or empty reason is refused ───
echo "-- Test 5: --allow-unstaged requires a non-empty reason"
t5="$tmpdir/t5"
new_repo_with_files "$t5" a.txt b.txt
echo "change a" >>"$t5/a.txt"
echo "change b" >>"$t5/b.txt"
git -C "$t5" add a.txt b.txt
git -C "$t5" reset -q -- b.txt
run_sign "$t5" --allow-unstaged
assert_exit "missing reason is refused" 1
assert_contains "explains the reason is required" "non-empty reason"
run_sign "$t5" --allow-unstaged ""
assert_exit "empty-string reason is refused" 1
assert_contains "explains the reason is required (empty string)" "non-empty reason"
echo

# ─── Test 6: the one-positional-argument invocation still works ───────────
# This is the compatibility guarantee that protects the untracked caller
# (the code-review-architect agent prompt) — it must keep calling
# `sign-review.sh "<key>"` with no flags and get the same behavior it always
# has.
echo "-- Test 6: one-positional-argument invocation (compatibility guarantee)"
t6="$tmpdir/t6"
new_repo_with_files "$t6" a.txt
echo "change a" >>"$t6/a.txt"
git -C "$t6" add a.txt
if out="$(cd "$t6" && "$SIGN" "$PRIVATE_KEY" 2>&1)"; then
    status6=0
else
    status6=$?
fi
if [ "$status6" -eq 0 ]; then
    pass "single-positional-arg invocation succeeds (exit 0)"
else
    fail "single-positional-arg invocation succeeds (exit 0)" "exit=$status6; $out"
fi
if [ -f "$t6/.review-approval" ] && [ "$(sed -n '1p' "$t6/.review-approval")" = "$(diff_cached_hash "$t6")" ]; then
    pass "writes a valid .review-approval with exactly the historical invocation"
else
    fail "writes a valid .review-approval with exactly the historical invocation" "$(cat "$t6/.review-approval" 2>/dev/null || echo MISSING)"
fi
echo

# ─── Test 7: unstaged change to a file OUTSIDE the staged set still trips ──
# the guard — this is about the state of the tree, not about overlap with
# what happens to be staged. b.txt was never staged at all in this test.
echo "-- Test 7: unstaged file outside the staged set still trips the guard"
t7="$tmpdir/t7"
new_repo_with_files "$t7" a.txt b.txt
echo "change a" >>"$t7/a.txt"
git -C "$t7" add a.txt
echo "never staged" >>"$t7/b.txt"
run_sign "$t7"
assert_exit "refuses because of b.txt, though only a.txt was ever staged" 1
assert_contains "names b.txt as offending" "b.txt"
echo

# ─── Test 8: untracked, non-ignored files DO trip the guard (KYO-712) ──────
# Inverted from the pre-Part-A version of this test, which asserted the
# opposite. That old assertion was exactly the gap the ticket closes: a
# file staged as new (`A `), reviewed, then dropped with `git restore
# --staged` becomes untracked (`??`), not "modified" — `git diff
# --name-only` never sees it. The real KYO-676 diff contained a new file;
# it happened to be the one that stayed staged. Had it been one of the
# ones dropped, the pre-Part-A guard would have signed the narrowed index
# anyway, exactly like this test used to assert was fine.
echo "-- Test 8: untracked non-ignored files trip the guard (KYO-712 Part A)"
t8="$tmpdir/t8"
new_repo_with_files "$t8" a.txt
echo "change a" >>"$t8/a.txt"
git -C "$t8" add a.txt
echo "brand new, never tracked" >"$t8/untracked.txt"
run_sign "$t8"
assert_exit "refuses because of the untracked file" 1
assert_contains "names untracked.txt as offending" "untracked.txt"
echo

# ─── Test 9: a gitignored file does NOT trip the guard ─────────────────────
# This is the property the old Test 8 was actually reaching for, and the
# one that matters in practice: --exclude-standard is load-bearing because
# .review-approval itself (.gitignore:88), docs/review-logs/, and target/
# are all gitignored and get created right around signing time. Without
# --exclude-standard, Test 8's new guard would refuse every legitimate
# signature.
echo "-- Test 9: a gitignored file does NOT trip the guard"
t9="$tmpdir/t9"
git init -q -b main "$t9"
echo "ignored.txt" >"$t9/.gitignore"
echo "seed: a.txt" >"$t9/a.txt"
git -C "$t9" add a.txt .gitignore
git -C "$t9" commit -q -m init
echo "change a" >>"$t9/a.txt"
git -C "$t9" add a.txt
echo "should be invisible to the guard" >"$t9/ignored.txt"
run_sign "$t9"
assert_exit "signs successfully despite a gitignored file present" 0
assert_not_contains "does not mention the gitignored file" "ignored.txt"
echo

# ─── Test 10: new-file variant of the AC4 scenario (KYO-712 Part A) ───────
# Mirrors Test 1 but with the shape the ticket calls out: a newly-ADDED
# file dropped from the index, not a modified one. Stage a modified
# tracked file plus a brand-new file, sign (succeeds), unstage the new
# file, sign again — must refuse and name it.
echo "-- Test 10: new-file variant of the ticket scenario"
t10="$tmpdir/t10"
new_repo_with_files "$t10" a.txt
echo "change a" >>"$t10/a.txt"
echo "brand new file, part of the reviewed diff" >"$t10/new.txt"
git -C "$t10" add a.txt new.txt
run_sign "$t10"
assert_exit "first sign (fully staged, including the new file) succeeds" 0
git -C "$t10" restore --staged new.txt
run_sign "$t10"
assert_exit "second sign (new file dropped from index) is refused" 1
assert_contains "names new.txt as offending" "new.txt"
echo

# ─── Test 11: refusal distinguishes modified-tracked vs untracked-new ─────
# When both categories are present at once, a reader needs to know which
# remedy applies to which file: `git add` on a modification vs `git add`
# on a brand-new file. Same verb, but knowing which file is which still
# matters for a reviewer deciding whether the narrowing was deliberate.
echo "-- Test 11: refusal distinguishes modified-tracked vs untracked-new"
t11="$tmpdir/t11"
new_repo_with_files "$t11" a.txt b.txt
echo "change a" >>"$t11/a.txt"
echo "change b" >>"$t11/b.txt"
git -C "$t11" add a.txt b.txt
git -C "$t11" reset -q -- b.txt
echo "brand new" >"$t11/new.txt"
run_sign "$t11"
assert_exit "refuses with both categories present" 1
assert_contains "names b.txt (modified tracked)" "b.txt"
assert_contains "names new.txt (untracked new)" "new.txt"
assert_contains "labels the modified-tracked category" "Modified tracked files"
assert_contains "labels the untracked-new category" "New files that were never staged"
echo

echo "Results: $PASS passed, $FAIL failed"
[ "$FAIL" -eq 0 ]
