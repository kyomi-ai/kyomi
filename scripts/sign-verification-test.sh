#!/usr/bin/env bash
# ------------------------------------------------------------------------------
# scripts/sign-verification-test.sh — self-test for sign-verification.sh
# (KYO-747)
#
# sign-verification.sh is called by the test-verification-architect agent to
# sign a PR's head SHA with the verifier's Ed25519 private key, producing the
# approval file that ~/.local/bin/gh's `gh pr merge` wrapper requires before
# it will merge. KYO-712 fixed the identical "-rawin missing" defect in
# scripts/sign-review.sh (the code-review approval signer, tested by
# scripts/sign-review-test.sh) — this suite is that same fix and the same
# test shape, one layer over: `pkeyutl -sign` for an Ed25519 key fails
# outright on OpenSSL 3.0.x (Ubuntu 24.04, i.e. ubuntu-latest, the runner
# this suite executes on in CI) without -rawin, and only starts working
# flagless on OpenSSL 3.2+. See sign-verification.sh's own comment on its
# `pkeyutl -sign` line for the measured version matrix.
#
# Follows scripts/sign-review-test.sh's conventions: hermetic, real
# throwaway git repos under a fresh mktemp -d (removed on exit via trap),
# branch names pinned explicitly to `main` (CI's `init.defaultBranch` is
# `master` on ubuntu-latest), a throwaway Ed25519 keypair generated fresh
# with `openssl genpkey` for this run only — the real verifier key is never
# read from anywhere here — and no network access.
#
# sign-verification.sh also shells out to `gh pr view` to resolve the PR's
# head SHA, so this suite places a stub `gh` first on PATH (same pattern as
# scripts/check-ticket-in-flight-test.sh): it ignores its arguments and
# always reports a fixed, fake head SHA, unless the harness points
# GH_EXIT_FILE at a non-zero exit to simulate `gh` failing.
#
# SCOPE: this suite tests scripts/sign-verification.sh directly. It does
# NOT test ~/.local/bin/gh's own verification of the approval file it
# reads (that file lives outside this repo and is not part of this change —
# see this ticket's PR body for the one-line -rawin edit it still needs) or
# any interaction with a real GitHub PR.
#
# Cases:
#   1. signing succeeds and writes the approval file at
#      <git-common-dir>/verification-approvals/pr-<N> in the documented
#      3-line format (PR number, head SHA, base64 signature)
#   2. the recorded signature cryptographically verifies against the public
#      key using `openssl pkeyutl -verify -rawin -pubin …` — the exact form
#      ~/.local/bin/gh's verifier must use after its own pending manual fix
#   3. a signature produced with -rawin is byte-identical to one produced
#      flagless on this box's OpenSSL version, and cross-verifies in both
#      directions, so existing approvals stay valid — skipped cleanly and
#      visibly (not counted as a failure) on an openssl build where the
#      flagless form itself does not work (e.g. genuinely pre-3.2 in a way
#      that also can't sign flagless — practically: never on 3.2+, and on
#      3.0.x this assertion just doesn't apply, matching
#      sign-review-test.sh's own documented behavior on this point)
#   4. failure path: a malformed private key exits non-zero, writes no
#      approval file, and the printed error does not claim the key format
#      is the (only) problem — it names openssl version / -rawin support as
#      possibilities too and surfaces openssl's own stderr
#   5. failure path: a missing private key argument (pre-existing usage
#      guard) exits non-zero and writes no approval file
#   6. failure path: `gh pr view` failing to resolve a head SHA (PR not
#      found / not open) exits non-zero and writes no approval file
#      (pre-existing guard, exercised here to prove the stub-gh harness
#      itself is wired correctly, not a KYO-747 behavior change)
#
# Exit 0 = all pass, exit 1 = any failure.
# ------------------------------------------------------------------------------

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
SIGN="$SCRIPT_DIR/sign-verification.sh"
PASS=0
FAIL=0
SKIP=0

tmpdir="$(mktemp -d)"
trap 'rm -rf "$tmpdir"' EXIT

export GIT_AUTHOR_NAME="KYO-747 Test" GIT_AUTHOR_EMAIL="kyo747-test@kyomi.invalid"
export GIT_COMMITTER_NAME="KYO-747 Test" GIT_COMMITTER_EMAIL="kyo747-test@kyomi.invalid"

pass() {
    printf "  \xe2\x9c\x93 %s\n" "$1"
    PASS=$((PASS + 1))
}

fail() {
    printf "  \xe2\x9c\x97 %s\n" "$1"
    printf '    %s\n' "$2" | sed 's/^/    | /'
    FAIL=$((FAIL + 1))
}

skip() {
    printf "  \xe2\x9a\xa0 SKIP: %s\n" "$1"
    SKIP=$((SKIP + 1))
}

# ─── throwaway Ed25519 keypair, generated fresh for this run ───────────────
KEY_PEM="$tmpdir/verifier_key.pem"
PUB_PEM="$tmpdir/verifier_pub.pem"
openssl genpkey -algorithm ed25519 -out "$KEY_PEM" >/dev/null 2>&1
openssl pkey -in "$KEY_PEM" -pubout -out "$PUB_PEM" >/dev/null 2>&1
PRIVATE_KEY="$(cat "$KEY_PEM")"

FAKE_SHA="deadbeefcafef00d0123456789abcdeffedcba9"

# ─── stub `gh`, controlled by files the harness writes before each call ───
STUB_BIN="$tmpdir/bin"
mkdir -p "$STUB_BIN"
export GH_STDOUT_FILE="$tmpdir/gh_stdout"
export GH_EXIT_FILE="$tmpdir/gh_exit"
printf '%s' "$FAKE_SHA" >"$GH_STDOUT_FILE"
printf '0' >"$GH_EXIT_FILE"

cat >"$STUB_BIN/gh" <<'STUB'
#!/usr/bin/env bash
# Test-only stand-in for `gh`. Ignores its arguments entirely and replays
# whatever the test harness staged in GH_STDOUT_FILE / GH_EXIT_FILE. Ships
# only inside this test's own $tmpdir/bin, first on PATH for the duration
# of the run — never touches the real `gh` or network.
cat "$GH_STDOUT_FILE"
exit "$(cat "$GH_EXIT_FILE")"
STUB
chmod +x "$STUB_BIN/gh"
export PATH="$STUB_BIN:$PATH"

gh_ok() {
    printf '%s' "$FAKE_SHA" >"$GH_STDOUT_FILE"
    printf '0' >"$GH_EXIT_FILE"
}

gh_fail() {
    printf '' >"$GH_STDOUT_FILE"
    printf '1' >"$GH_EXIT_FILE"
}

# ─── invocation helper, capturing exit code + combined output ───────────────
RUN_STATUS=""
RUN_OUTPUT=""
run_sign() {
    # run_sign <repo_dir> <pr_number> <key>  — invokes sign-verification.sh
    # with cwd set to <repo_dir> (it operates on the current directory's git
    # common dir and writes the approval file there).
    local dir="$1" pr="$2" key="$3"
    local out
    if out="$(cd "$dir" && "$SIGN" "$pr" "$key" 2>&1)"; then
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

new_repo() {
    # new_repo <dir> -> a real git repo on branch main with one commit.
    local dir="$1"
    git init -q -b main "$dir"
    echo "seed" >"$dir/seed.txt"
    git -C "$dir" add seed.txt
    git -C "$dir" commit -q -m init
}

approval_path() {
    # approval_path <dir> <pr_number> -> the approval file path
    # sign-verification.sh should write for a plain (non-worktree) repo.
    printf '%s/.git/verification-approvals/pr-%s' "$1" "$2"
}

echo "Running sign-verification self-tests..."
echo

# ─── Test 1: signing succeeds and writes the approval file correctly ──────
echo "-- Test 1: signing succeeds, approval file written in expected format/location"
t1="$tmpdir/t1"
new_repo "$t1"
gh_ok
run_sign "$t1" "101" "$PRIVATE_KEY"
assert_exit "sign succeeds" 0

t1_approval="$(approval_path "$t1" 101)"
if [ -f "$t1_approval" ]; then
    pass "approval file written at the documented location"
else
    fail "approval file written at the documented location" "not found at $t1_approval"
fi

t1_line1="$(sed -n '1p' "$t1_approval" 2>/dev/null || echo MISSING)"
t1_line2="$(sed -n '2p' "$t1_approval" 2>/dev/null || echo MISSING)"
t1_line3="$(sed -n '3p' "$t1_approval" 2>/dev/null || echo MISSING)"

if [ "$t1_line1" = "101" ]; then
    pass "line 1 is the PR number"
else
    fail "line 1 is the PR number" "got: $t1_line1"
fi

if [ "$t1_line2" = "$FAKE_SHA" ]; then
    pass "line 2 is the resolved head SHA"
else
    fail "line 2 is the resolved head SHA" "got: $t1_line2 expected: $FAKE_SHA"
fi

if [ -n "$t1_line3" ] && [ "$t1_line3" != "MISSING" ]; then
    pass "line 3 (signature) is present and non-empty"
else
    fail "line 3 (signature) is present and non-empty" "got: $t1_line3"
fi
echo

# ─── Test 2: the recorded signature cryptographically verifies ────────────
# with -rawin — the exact form ~/.local/bin/gh's verifier must use after its
# own pending manual fix (see the PR body).
echo "-- Test 2: recorded signature verifies with 'openssl pkeyutl -verify -rawin'"
t1_sha_file="$tmpdir/t1_sha"
t1_sig_file="$tmpdir/t1_sig"
printf -- '%s' "$FAKE_SHA" >"$t1_sha_file"
printf '%s' "$t1_line3" | base64 -d >"$t1_sig_file" 2>/dev/null || true
if openssl pkeyutl -verify -rawin -pubin -inkey "$PUB_PEM" -in "$t1_sha_file" -sigfile "$t1_sig_file" >/dev/null 2>&1; then
    pass "signature cryptographically verifies against the public key with -rawin"
else
    fail "signature cryptographically verifies against the public key with -rawin" "sig=$t1_line3"
fi
echo

# ─── Test 3: -rawin signature is byte-identical to a flagless one ─────────
# on this box's OpenSSL version, and cross-verifies in both directions, so
# existing approvals signed before KYO-747 stay valid. Skipped cleanly and
# visibly if this box's openssl cannot sign Ed25519 flagless at all (older
# than 3.2 in a way that never worked without -rawin either — matches
# scripts/sign-review-test.sh's own documented scope on this point).
echo "-- Test 3: -rawin signature is byte-identical to flagless on this OpenSSL"
OPENSSL_VER="$(openssl version)"
t3_sha_file="$tmpdir/t3_sha"
printf -- '%s' "$FAKE_SHA" >"$t3_sha_file"
t3_rawin_sig="$(openssl pkeyutl -sign -rawin -inkey "$KEY_PEM" -in "$t3_sha_file" 2>/dev/null | base64 -w 0)"

# Binary signature bytes must never pass through a bare `$(...)` command
# substitution (it strips embedded NUL bytes and trailing newlines,
# silently corrupting the signature) — write the raw flagless signature
# straight to a file instead, exactly as the -rawin form above pipes
# straight into base64 without an intermediate variable.
t3_flagless_sig_raw_file="$tmpdir/t3_flagless_sig_raw"
if openssl pkeyutl -sign -inkey "$KEY_PEM" -in "$t3_sha_file" >"$t3_flagless_sig_raw_file" 2>/dev/null \
    && [ -s "$t3_flagless_sig_raw_file" ]; then
    t3_flagless_sig="$(base64 -w 0 <"$t3_flagless_sig_raw_file")"
    if [ "$t3_rawin_sig" = "$t3_flagless_sig" ]; then
        pass "rawin and flagless signatures are byte-identical ($OPENSSL_VER)"
    else
        fail "rawin and flagless signatures are byte-identical ($OPENSSL_VER)" "rawin=$t3_rawin_sig flagless=$t3_flagless_sig"
    fi

    # Cross-verify: the flagless signature verifies with -rawin verify, and
    # the rawin signature verifies with flagless verify.
    t3_flagless_sig_file="$tmpdir/t3_flagless_sig"
    printf '%s' "$t3_flagless_sig" | base64 -d >"$t3_flagless_sig_file" 2>/dev/null || true
    if openssl pkeyutl -verify -rawin -pubin -inkey "$PUB_PEM" -in "$t3_sha_file" -sigfile "$t3_flagless_sig_file" >/dev/null 2>&1; then
        pass "flagless-produced signature cross-verifies with -rawin verify"
    else
        fail "flagless-produced signature cross-verifies with -rawin verify" "sig=$t3_flagless_sig"
    fi

    t3_rawin_sig_file="$tmpdir/t3_rawin_sig"
    printf '%s' "$t3_rawin_sig" | base64 -d >"$t3_rawin_sig_file" 2>/dev/null || true
    if openssl pkeyutl -verify -pubin -inkey "$PUB_PEM" -in "$t3_sha_file" -sigfile "$t3_rawin_sig_file" >/dev/null 2>&1; then
        pass "rawin-produced signature cross-verifies with flagless verify"
    else
        fail "rawin-produced signature cross-verifies with flagless verify" "sig=$t3_rawin_sig"
    fi
else
    skip "flagless Ed25519 signing is not supported on this openssl ($OPENSSL_VER) — byte-identity/cross-verify not applicable here"
fi
echo

# ─── Test 4: malformed private key — failure path with a clear error ──────
echo "-- Test 4: malformed private key fails closed with a non-misleading error"
t4="$tmpdir/t4"
new_repo "$t4"
gh_ok
run_sign "$t4" "202" "this is not a valid PEM key"
assert_exit "malformed key exits non-zero" 1
t4_approval="$(approval_path "$t4" 202)"
if [ ! -f "$t4_approval" ]; then
    pass "no approval file written on signing failure"
else
    fail "no approval file written on signing failure" "found at $t4_approval"
fi
assert_contains "error mentions the key file/argument as one possibility" "key"
assert_contains "error mentions openssl version as a possibility" "openssl version"
assert_contains "error mentions -rawin support as a possibility" "-rawin"
assert_not_contains "error does not claim the key format is THE problem" "check private key format"
echo

# ─── Test 5: missing private key argument (pre-existing usage guard) ──────
echo "-- Test 5: missing private key argument is refused"
t5="$tmpdir/t5"
new_repo "$t5"
gh_ok
if out="$(cd "$t5" && "$SIGN" "303" 2>&1)"; then
    status5=0
else
    status5=$?
fi
RUN_STATUS="$status5"
RUN_OUTPUT="$out"
assert_exit "missing key argument exits non-zero" 1
assert_contains "usage error is shown" "Usage:"
t5_approval="$(approval_path "$t5" 303)"
if [ ! -f "$t5_approval" ]; then
    pass "no approval file written when the key argument is missing"
else
    fail "no approval file written when the key argument is missing" "found at $t5_approval"
fi
echo

# ─── Test 6: gh cannot resolve the PR's head SHA ───────────────────────────
# Exercises the pre-existing HEAD_SHA guard, mainly to prove the stub-gh
# harness above is wired correctly for KYO-747's own new tests, not a
# behavior change this ticket makes.
echo "-- Test 6: gh failing to resolve a head SHA is refused"
t6="$tmpdir/t6"
new_repo "$t6"
gh_fail
run_sign "$t6" "404" "$PRIVATE_KEY"
assert_exit "gh failure exits non-zero" 1
assert_contains "error explains the head SHA could not be resolved" "Could not resolve"
t6_approval="$(approval_path "$t6" 404)"
if [ ! -f "$t6_approval" ]; then
    pass "no approval file written when gh cannot resolve a head SHA"
else
    fail "no approval file written when gh cannot resolve a head SHA" "found at $t6_approval"
fi
gh_ok
echo

echo "Results: $PASS passed, $FAIL failed, $SKIP skipped"
[ "$FAIL" -eq 0 ]
