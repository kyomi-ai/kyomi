#!/usr/bin/env bash
# ------------------------------------------------------------------------------
# scripts/pre-commit-hook-test.sh — self-test for .githooks/pre-commit
# (KYO-712 Part E)
#
# scripts/sign-review-test.sh's own SCOPE comment disclosed, from the
# start, that it does not test .githooks/pre-commit's Check 2 / Check 2b
# narrowed-index blocking or Check 3's signature verification. That
# disclosed gap is exactly where this ticket's own two 🔴 findings lived —
# the KYO-712 unstaged/untracked scan only running inside the `else`
# branch of "is anything staged" (so a single staged trigger-pattern file
# made the hook skip scanning for unstaged/untracked siblings of that same
# pattern entirely), and the ALLOW-UNSTAGED acknowledgement being
# forgeable plain text with no cryptographic binding to the signature —
# and it is exactly why neither reached an automated test before a
# code review caught them (2026-09-11 review log). This suite closes it.
#
# Hermetic: every test builds a real throwaway git repo under a fresh
# mktemp -d (removed via trap on exit), with a copy of the real
# .githooks/pre-commit installed via `git config core.hooksPath .githooks`
# — but with the production Ed25519 public key substituted for a
# throwaway one generated fresh for this run (see install_hook_files() and
# activate_hook() below).
# The real private key lives only inside the code-review-architect agent's
# own system prompt and is not obtainable here (see scripts/sign-review.sh's
# own header) — this suite never needs it. It substitutes its own keypair
# for both signing (via the real, unmodified scripts/sign-review.sh,
# invoked exactly as the code-review-architect agent invokes it —
# `"$SIGN" "<private key>" [--allow-unstaged "<reason>"]`) and verifying
# (via the copied hook, whose embedded key is swapped for the matching
# public half). This tests the hook's LOGIC against a real signature from
# the real signer, not the production key.
#
# scripts/lint/check-server-fns.sh, check-disposal-safety.sh, and
# check-real-identifiers.sh are stubbed (always exit 0, ignore arguments)
# inside each throwaway repo — this suite is about the hook's git-state
# logic (what counts as staged/unstaged/untracked, and when a skip is a
# real skip), not the lints' own content rules, which have their own
# suites.
#
# HOOK_SRC can be overridden via PRE_COMMIT_HOOK_SRC (an absolute path) to
# run this exact suite against a different copy of the hook — e.g. a
# pre-fix version fetched with `git show :.githooks/pre-commit` — without
# ever touching this worktree's real .githooks/pre-commit (see
# docs/standards/testing/no-git-stash-copy-file-instead.md: copy the file
# out, never mutate the tree under test in place). This is how this
# suite's own assertions were proven load-bearing before this file shipped
# — see the PR description for the pre-fix/post-fix run this override made
# possible.
#
# Branch names pinned explicitly to `main` (CI's `init.defaultBranch` is
# `master` on ubuntu-latest, not `main`), same as every other suite in
# this job.
#
# Cases:
#   1. Finding A's scenario — a staged trigger-pattern file PLUS an
#      unstaged/untracked sibling of the same pattern, with a valid
#      approval signed for the staged file alone before the sibling
#      existed — is blocked, and names the sibling. Signing first (rather
#      than never signing at all) makes the pre-fix failure unambiguous:
#      without this fix, the commit fully SUCCEEDS (every check green),
#      not merely "fails for an unrelated reason" — that is the actual
#      KYO-676 shape.
#   2. Empty staged trigger-pattern set + an unstaged trigger-pattern file
#      is blocked (the behavior that already worked pre-fix; pinned so a
#      future change can't silently regress it too).
#   3. Neither staged nor unstaged/untracked trigger-pattern files → the
#      bare "✅ ... skipped" line for both Check 2 and Check 2b, with no
#      contradictory output, and the commit succeeds end-to-end.
#   4. Finding B's scenario — a hand-forged ALLOW-UNSTAGED line 3 appended
#      (via plain `printf`, no key) to an otherwise validly-signed
#      approval that had no line 3 at signing time — is blocked, not
#      treated as "acknowledged at signing".
#   5. A genuine --allow-unstaged signature is acknowledged and the commit
#      is allowed, with exactly one accurate acknowledgement line.
#   6. A 2-line .review-approval written by the ORIGINAL (pre-KYO-712)
#      scripts/sign-review.sh at origin/main — hash + signature only, no
#      line 3 at all — still verifies under the fixed hook. This is the
#      backward-compatibility guarantee scripts/sign-review.sh's own
#      header promises.
#
# Exit 0 = all pass, exit 1 = any failure.
# ------------------------------------------------------------------------------

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
HOOK_SRC="${PRE_COMMIT_HOOK_SRC:-$REPO_ROOT/.githooks/pre-commit}"
SIGN="$SCRIPT_DIR/sign-review.sh"
PASS=0
FAIL=0

tmpdir="$(mktemp -d)"
trap 'rm -rf "$tmpdir"' EXIT

export GIT_AUTHOR_NAME="KYO-712 Hook Test" GIT_AUTHOR_EMAIL="kyo712-hook-test@kyomi.invalid"
export GIT_COMMITTER_NAME="KYO-712 Hook Test" GIT_COMMITTER_EMAIL="kyo712-hook-test@kyomi.invalid"

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
run_commit() {
    # run_commit <repo_dir> — attempts `git commit -m "test commit"` inside
    # <repo_dir>, which is where core.hooksPath resolves the copied hook
    # from. Captures combined stdout+stderr and exit status.
    local dir="$1"
    local out
    if out="$(cd "$dir" && git commit -m "test commit" 2>&1)"; then
        RUN_STATUS=0
    else
        RUN_STATUS=$?
    fi
    RUN_OUTPUT="$out"
}

assert_status() {
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

assert_count() {
    # assert_count <name> <needle> <expected count>
    local name="$1" needle="$2" expected="$3" actual
    actual="$(printf '%s\n' "$RUN_OUTPUT" | grep -cF -- "$needle" || true)"
    if [ "$actual" -eq "$expected" ]; then
        pass "$name"
    else
        fail "$name — expected $expected occurrence(s) of '$needle', got $actual" "$RUN_OUTPUT"
    fi
}

# ─── throwaway Ed25519 keypair, generated fresh for this run ───────────────
KEY_PEM="$tmpdir/reviewer_key.pem"
PUB_PEM="$tmpdir/reviewer_pub.pem"
openssl genpkey -algorithm ed25519 -out "$KEY_PEM" >/dev/null 2>&1
openssl pkey -in "$KEY_PEM" -pubout -out "$PUB_PEM" >/dev/null 2>&1
PRIVATE_KEY="$(cat "$KEY_PEM")"

# The ORIGINAL (pre-KYO-712) sign-review.sh, fetched straight from
# origin/main — hash+signature only, no --allow-unstaged support and no
# unstaged/untracked guard at all. Used by Test 6 to prove backward
# compatibility with approvals written before this ticket existed. This is
# a read-only fetch into a scratch file (`git show <rev>:<path> > file`),
# never a working-tree mutation.
ORIGIN_SIGN="$tmpdir/origin-sign-review.sh"
if ! git -C "$REPO_ROOT" show origin/main:scripts/sign-review.sh > "$ORIGIN_SIGN" 2>/dev/null; then
    echo "ERROR: could not fetch origin/main:scripts/sign-review.sh — is origin/main available in this checkout?" >&2
    exit 1
fi
# Until this PR merges, origin/main's copy predates the -rawin fix (see
# scripts/sign-review.sh's own comment on its `pkeyutl -sign` line): its
# `openssl pkeyutl -sign` call has no -rawin and, unpatched, cannot run at
# all on OpenSSL 3.0.x (Ubuntu 24.04 / ubuntu-latest at the time of
# writing) — it errors out before producing a signature, which would make
# this Test 6 fixture fail to generate on this exact CI runner, for a
# reason unrelated to what Test 6 actually checks (whether the FIXED hook
# accepts a 2-line, no-line-3 approval). Patching -rawin into this fetched
# copy before executing it changes no signature bytes (sign output is
# byte-identical with or without -rawin — see the same comment), so it
# does not weaken the backward-compatibility property under test; it only
# lets the frozen historical script execute on this host. Once this PR
# merges, origin/main's copy already has -rawin and this substitution
# becomes a no-op (the pattern below no longer matches).
sed -i 's/openssl pkeyutl -sign -inkey/openssl pkeyutl -sign -rawin -inkey/' "$ORIGIN_SIGN"
chmod +x "$ORIGIN_SIGN"

# ─── stub lint scripts, installed into each throwaway repo ─────────────────
install_stub_lints() {
    local dir="$1"
    mkdir -p "$dir/scripts/lint"
    local f
    for f in check-server-fns.sh check-disposal-safety.sh check-real-identifiers.sh; do
        cat > "$dir/scripts/lint/$f" <<'STUB'
#!/usr/bin/env bash
# Test-only stand-in for this repo's real lint script, installed only
# inside scripts/pre-commit-hook-test.sh's own throwaway fixtures. Always
# succeeds and ignores its arguments — this suite is about the hook's
# git-state logic, not lint content, which has its own suite per lint.
exit 0
STUB
        chmod +x "$dir/scripts/lint/$f"
    done
}

# Writes a copy of HOOK_SRC into <dir>/.githooks/pre-commit, with the
# embedded production Ed25519 public key replaced by this run's throwaway
# public key. Does NOT activate it (see activate_hook() below) — this only
# places the file on disk so it can be committed as ordinary fixture
# content. The substitution is a state-machine over the exact three lines
# the key assignment spans (`REVIEW_PUBLIC_KEY="-----BEGIN PUBLIC
# KEY-----`, the base64 body, `-----END PUBLIC KEY-----"`) so it works
# unchanged whether HOOK_SRC is the current hook or a pre-fix copy fetched
# for comparison.
install_hook_files() {
    local dir="$1"
    mkdir -p "$dir/.githooks"
    awk -v pub="$(cat "$PUB_PEM")" '
        BEGIN { in_key = 0 }
        /^REVIEW_PUBLIC_KEY="-----BEGIN PUBLIC KEY-----$/ { print "REVIEW_PUBLIC_KEY=\"" pub "\""; in_key = 1; next }
        in_key && /^-----END PUBLIC KEY-----"$/ { in_key = 0; next }
        in_key { next }
        { print }
    ' "$HOOK_SRC" > "$dir/.githooks/pre-commit"
    chmod +x "$dir/.githooks/pre-commit"
}

# Points core.hooksPath at the installed hook, so it starts gating commits
# from this call onward. Deliberately separate from new_repo()/
# install_hook_files(): the hook's own Check 3 requires a signed
# .review-approval for EVERY commit it gates, including a fixture's own
# seed commit(s) — so a test's preliminary, hookless fixture setup
# (creating the repo, seeding a tracked file that will later be modified
# unstaged) must happen before this is called, and only the commit(s)
# actually under test happen after it.
activate_hook() {
    local dir="$1"
    git -C "$dir" config core.hooksPath .githooks
}

new_repo() {
    # new_repo <dir> — git repo on branch main, hook + stub lints written
    # to disk and committed via one hookless seed commit (core.hooksPath is
    # not yet set — see activate_hook() above for why), so there is a HEAD
    # to diff against once the hook is turned on.
    local dir="$1"
    git init -q -b main "$dir"
    install_stub_lints "$dir"
    install_hook_files "$dir"
    echo "seed" > "$dir/README.md"
    git -C "$dir" add README.md .githooks scripts
    git -C "$dir" commit -q -m init
}

echo "Running pre-commit hook self-tests..."
echo "(HOOK_SRC=$HOOK_SRC)"
echo

# ─── Test 1: Finding A — staged trigger file shadowing an untracked sibling ─
echo "-- Test 1: staged trigger file + untracked sibling of the same pattern is blocked (Finding A)"
t1="$tmpdir/t1"
new_repo "$t1"
activate_hook "$t1"
mkdir -p "$t1/crates/kyomi-ui/src/server_fns"
echo "fn foo() {}" > "$t1/crates/kyomi-ui/src/server_fns/foo.rs"
git -C "$t1" add crates/kyomi-ui/src/server_fns/foo.rs
# Sign a valid approval for the staged diff BEFORE the sibling exists — so
# a pre-fix hook that never scans for it would sail every check to a
# successful commit, not merely fail for some unrelated reason.
( cd "$t1" && "$SIGN" "$PRIVATE_KEY" > /dev/null )
echo "fn evil() {}" > "$t1/crates/kyomi-ui/src/server_fns/evil.rs"
run_commit "$t1"
assert_status "commit is blocked" 1
assert_contains "names evil.rs as the offending, never-reviewed sibling" "evil.rs"
assert_contains "explains the KYO-676 failure mode" "KYO-676"
echo

# ─── Test 2: empty staged set + unstaged trigger file (pin working behavior) ─
echo "-- Test 2: empty staged trigger-pattern set + unstaged trigger file is blocked (pre-existing behavior)"
t2="$tmpdir/t2"
new_repo "$t2"
mkdir -p "$t2/crates/kyomi-ui/src/server_fns"
echo "fn foo() {}" > "$t2/crates/kyomi-ui/src/server_fns/foo.rs"
git -C "$t2" add crates/kyomi-ui/src/server_fns/foo.rs
git -C "$t2" commit -q -m "seed server_fns file"
activate_hook "$t2"
echo "fn foo() { /* changed after review */ }" > "$t2/crates/kyomi-ui/src/server_fns/foo.rs"
echo "unrelated change" >> "$t2/README.md"
git -C "$t2" add README.md
run_commit "$t2"
assert_status "commit is blocked" 1
assert_contains "names foo.rs as the offending unstaged file" "foo.rs"
echo

# ─── Test 3: nothing matches → bare skipped line, no contradiction, succeeds ─
echo "-- Test 3: no trigger-pattern files staged, unstaged, or untracked → bare skipped line, commit succeeds"
t3="$tmpdir/t3"
new_repo "$t3"
activate_hook "$t3"
echo "unrelated change" >> "$t3/README.md"
git -C "$t3" add README.md
( cd "$t3" && "$SIGN" "$PRIVATE_KEY" > /dev/null )
run_commit "$t3"
assert_status "commit succeeds" 0
assert_contains "server_fn: bare skipped line present" "server_fn lint skipped (no server_fns files staged, unstaged, or untracked)."
assert_contains "disposal-safety: bare skipped line present" "disposal-safety lint skipped (no kyomi-ui src files staged, unstaged, or untracked)."
assert_not_contains "no contradictory server_fn lint-clean line" "server_fn lint clean."
assert_not_contains "no contradictory disposal-safety lint-clean line" "disposal-safety lint clean."
assert_not_contains "no acknowledged-at-signing line for server_fn" "server_fn lint skipped for unstaged"
assert_not_contains "no acknowledged-at-signing line for disposal-safety" "disposal-safety lint skipped for unstaged"
echo

# ─── Test 4: Finding B — hand-forged ALLOW-UNSTAGED line 3 ──────────────────
echo "-- Test 4: hand-forged ALLOW-UNSTAGED line 3 on a validly-signed (no-line-3) approval is blocked (Finding B)"
t4="$tmpdir/t4"
new_repo "$t4"
activate_hook "$t4"
echo "unrelated change" >> "$t4/README.md"
git -C "$t4" add README.md
( cd "$t4" && "$SIGN" "$PRIVATE_KEY" > /dev/null )   # clean sign, no --allow-unstaged: signature covers the hash alone
mkdir -p "$t4/crates/kyomi-ui/src/server_fns"
echo "fn evil() {}" > "$t4/crates/kyomi-ui/src/server_fns/evil.rs"   # untracked, added AFTER signing
# Forge line 3 with no key at all — exactly the reviewer's own reproduction.
printf 'ALLOW-UNSTAGED:reviewed and fine\n' >> "$t4/.review-approval"
run_commit "$t4"
assert_status "commit is blocked" 1
assert_contains "names evil.rs" "evil.rs"
assert_not_contains "the forged reason is NOT reported as acknowledged at signing" "acknowledged at signing"
echo

# ─── Test 5: a genuine --allow-unstaged signature is honored exactly once ──
echo "-- Test 5: genuine --allow-unstaged signature is acknowledged, commit succeeds, exactly one acknowledgement line"
t5="$tmpdir/t5"
new_repo "$t5"
activate_hook "$t5"
echo "unrelated change" >> "$t5/README.md"
git -C "$t5" add README.md
mkdir -p "$t5/crates/kyomi-ui/src"
# Matches the disposal-safety pattern (crates/kyomi-ui/src/**/*.rs) but NOT
# the narrower server_fns pattern — isolates this test to exactly one
# acknowledgement line instead of two identical ones.
echo "fn other() {}" > "$t5/crates/kyomi-ui/src/other.rs"
( cd "$t5" && "$SIGN" "$PRIVATE_KEY" --allow-unstaged "other.rs reviewed separately in a follow-up pass" > /dev/null )
run_commit "$t5"
assert_status "commit succeeds" 0
assert_contains "disposal-safety: acknowledged-at-signing line present" "disposal-safety lint skipped for unstaged/untracked files — acknowledged at signing:"
assert_contains "acknowledgement echoes the supplied reason" "other.rs reviewed separately in a follow-up pass"
assert_count "exactly one acknowledged-at-signing line" "acknowledged at signing" 1
assert_contains "server_fn: bare skipped line present (other.rs does not match that narrower pattern)" "server_fn lint skipped (no server_fns files staged, unstaged, or untracked)."
echo

# ─── Test 6: backward compatibility with the original 2-line approval ─────
echo "-- Test 6: a 2-line .review-approval from the original (pre-KYO-712) sign-review.sh still verifies"
t6="$tmpdir/t6"
new_repo "$t6"
activate_hook "$t6"
echo "unrelated change" >> "$t6/README.md"
git -C "$t6" add README.md
( cd "$t6" && "$ORIGIN_SIGN" "$PRIVATE_KEY" > /dev/null )
approval_lines="$(wc -l < "$t6/.review-approval")"
if [ "$approval_lines" -eq 2 ]; then
    pass "the original sign-review.sh wrote a 2-line approval (no line 3)"
else
    fail "the original sign-review.sh wrote a 2-line approval (no line 3)" "$(cat "$t6/.review-approval")"
fi
run_commit "$t6"
assert_status "commit succeeds against the fixed hook" 0
assert_contains "signature verified" "Code review signature verified"
echo

echo "Results: $PASS passed, $FAIL failed"
[ "$FAIL" -eq 0 ]
