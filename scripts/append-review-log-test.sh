#!/usr/bin/env bash
# ------------------------------------------------------------------------------
# scripts/append-review-log-test.sh — self-test for append-review-log.sh and
# the shared scripts/lib/canonical-root.sh (KYO-396).
#
# Follows the fixture-based pattern in
# scripts/lint/check-disposal-safety-test.sh: build synthetic fixtures in a
# temp dir, run the script under test against them, assert on exit codes
# and file contents. Exit 0 = all pass, exit 1 = failures.
#
# This is the executable pin for the ticket's minimum contract:
#   - appends (not overwrites)
#   - creates the dated file when absent
#   - resolves the canonical root from inside a worktree (the specific gap
#     KYO-396 exists to close — the fixture below builds a real throwaway
#     git repo with a real `git worktree add` linked worktree, `cd`s a
#     single shell into it, and asserts the entry lands in the CANONICAL
#     clone, not the worktree)
#   - rejects empty stdin rather than writing an empty entry
#
# It also pins two things beyond that minimum:
#   - an entry survives `git worktree remove` (the actual bug KYO-387/396
#     exist to prevent — the earlier fixture only proves the write went to
#     the right place; this proves the worktree's removal doesn't take it
#     with it)
#   - the git < 2.31 guard in scripts/lib/canonical-root.sh, exercised via a
#     stubbed `git` on PATH rather than a real old git binary (this
#     machine runs git 2.54.0 — see that file's header for why the guard
#     can't be hit with a real old git here, and why stubbing is the
#     chosen substitute for "reasoning only")
# ------------------------------------------------------------------------------

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
APPEND="$SCRIPT_DIR/append-review-log.sh"
PASS=0
FAIL=0

tmpdir="$(mktemp -d)"
trap 'rm -rf "$tmpdir"' EXIT

pass() {
    printf '  \xe2\x9c\x93 %s\n' "$1"
    PASS=$((PASS + 1))
}

fail() {
    printf '  \xe2\x9c\x97 %s\n' "$1"
    printf '    %s\n' "$2" | sed 's/^/    | /'
    FAIL=$((FAIL + 1))
}

echo "Running append-review-log tests..."
echo

# ─── Fixture: a throwaway canonical clone + a linked worktree ────────────
repo="$tmpdir/canonical-clone"
mkdir -p "$repo"
git -C "$repo" init -q -b main
git -C "$repo" config user.email "test@example.com"
git -C "$repo" config user.name "append-review-log-test"
echo "fixture repo" > "$repo/README.md"
git -C "$repo" add README.md
git -C "$repo" commit -q -m "init"

worktree="$tmpdir/wt"
git -C "$repo" worktree add -q -b wt-branch "$worktree" main >/dev/null

today="$(date +%F)"
target="$repo/docs/review-logs/${today}.md"

# ─── Test 1-3: appends (not overwrites), creates the file when absent,   ─
#     and resolves the canonical root from a genuinely-in-worktree cwd.   ─
#
# This `cd` and the two script invocations run as ONE shell (this whole
# block is a single `(...)` subshell within one bash process), so cwd is
# genuinely set to the worktree for both calls below — not `cd X && cmd`
# re-issued per invocation, which is the exact false-negative KYO-396 was
# filed about (see the PR description for the manual reproduction of this
# same proof outside the test harness).
if [ -e "$target" ]; then
    fail "setup" "target file already existed before any append: $target"
else
    (
        cd "$worktree"
        pwd > "$tmpdir/observed_cwd"
        printf 'first entry\n' | "$APPEND"
        printf 'second entry\n' | "$APPEND"
    )

    observed_cwd="$(cat "$tmpdir/observed_cwd")"
    real_worktree="$(cd "$worktree" && pwd)"
    if [ "$observed_cwd" = "$real_worktree" ]; then
        pass "cwd was genuinely inside the worktree during both appends"
    else
        fail "cwd was genuinely inside the worktree during both appends" \
            "observed '$observed_cwd', expected '$real_worktree'"
    fi

    if [ -f "$target" ]; then
        pass "creates the dated file when absent"
    else
        fail "creates the dated file when absent" "not found: $target"
    fi

    if [ ! -e "$worktree/docs/review-logs" ]; then
        pass "resolves canonical root from inside a worktree (nothing written into the worktree itself)"
    else
        fail "resolves canonical root from inside a worktree" \
            "docs/review-logs was created INSIDE the worktree: $worktree/docs/review-logs"
    fi

    if [ -f "$target" ] \
        && grep -q "^first entry$" "$target" \
        && grep -q "^second entry$" "$target" \
        && [ "$(grep -n "^first entry$" "$target" | head -1 | cut -d: -f1)" -lt \
             "$(grep -n "^second entry$" "$target" | head -1 | cut -d: -f1)" ]; then
        pass "appends (not overwrites) — both entries present, in call order"
    else
        fail "appends (not overwrites) — both entries present, in call order" \
            "$(cat "$target" 2>&1 || echo MISSING)"
    fi
fi

# ─── Test 4: survives `git worktree remove` ───────────────────────────────
git -C "$repo" worktree remove "$worktree" --force
if [ -f "$target" ] && grep -q "^first entry$" "$target" && grep -q "^second entry$" "$target"; then
    pass "entry survives 'git worktree remove'"
else
    fail "entry survives 'git worktree remove'" "$(cat "$target" 2>&1 || echo MISSING)"
fi

# ─── Test 5: rejects empty stdin (exit 2), does not touch the log file ──
before_bytes=$(wc -c < "$target")
set +e
output="$(cd "$repo" && printf '' | "$APPEND" 2>&1)"
status=$?
set -e
after_bytes=$(wc -c < "$target")
if [ "$status" -eq 2 ] && [ "$before_bytes" -eq "$after_bytes" ]; then
    pass "rejects empty stdin (exit 2), file untouched"
else
    fail "rejects empty stdin (exit 2), file untouched" \
        "status=$status before=$before_bytes after=$after_bytes output: $output"
fi

# ─── Test 6: rejects whitespace-only stdin (exit 2) ──────────────────────
set +e
output="$(cd "$repo" && printf '   \n  \t \n' | "$APPEND" 2>&1)"
status=$?
set -e
if [ "$status" -eq 2 ]; then
    pass "rejects whitespace-only stdin (exit 2)"
else
    fail "rejects whitespace-only stdin (exit 2)" "status=$status output: $output"
fi

# ─── Test 7: outside any git repository entirely -> exit 1 ──────────────
outside="$tmpdir/not-a-repo"
mkdir -p "$outside"
set +e
output="$(cd "$outside" && printf 'entry' | "$APPEND" 2>&1)"
status=$?
set -e
if [ "$status" -eq 1 ]; then
    pass "fails with exit 1 outside a git repository"
else
    fail "fails with exit 1 outside a git repository" "status=$status output: $output"
fi

# ─── Test 8: rejects unexpected arguments (exit 1) ───────────────────────
set +e
output="$(cd "$repo" && printf 'entry' | "$APPEND" some-argument 2>&1)"
status=$?
set -e
if [ "$status" -eq 1 ]; then
    pass "rejects unexpected arguments (exit 1)"
else
    fail "rejects unexpected arguments (exit 1)" "status=$status output: $output"
fi

# ─── Test 9: multiple review cycles from DIFFERENT worktrees land in one ─
#     file, in call order — the ticket's "regardless of which cycle ran   ─
#     where" requirement.
worktree_a="$tmpdir/wt-a"
worktree_b="$tmpdir/wt-b"
git -C "$repo" worktree add -q -b wt-a-branch "$worktree_a" main >/dev/null
git -C "$repo" worktree add -q -b wt-b-branch "$worktree_b" main >/dev/null

multi_today_marker="multi-cycle-$$"
(cd "$worktree_a" && printf '%s entry-from-a\n' "$multi_today_marker" | "$APPEND")
(cd "$worktree_b" && printf '%s entry-from-b\n' "$multi_today_marker" | "$APPEND")
(cd "$repo" && printf '%s entry-from-canonical\n' "$multi_today_marker" | "$APPEND")

git -C "$repo" worktree remove "$worktree_a" --force
git -C "$repo" worktree remove "$worktree_b" --force

if [ -f "$target" ] \
    && grep -q "^${multi_today_marker} entry-from-a$" "$target" \
    && grep -q "^${multi_today_marker} entry-from-b$" "$target" \
    && grep -q "^${multi_today_marker} entry-from-canonical$" "$target" \
    && [ "$(grep -n "${multi_today_marker} entry-from-a$" "$target" | cut -d: -f1)" -lt \
         "$(grep -n "${multi_today_marker} entry-from-b$" "$target" | cut -d: -f1)" ] \
    && [ "$(grep -n "${multi_today_marker} entry-from-b$" "$target" | cut -d: -f1)" -lt \
         "$(grep -n "${multi_today_marker} entry-from-canonical$" "$target" | cut -d: -f1)" ]; then
    pass "cycles from different worktrees land in one file, in call order, and survive removal"
else
    fail "cycles from different worktrees land in one file, in call order, and survive removal" \
        "$(cat "$target" 2>&1 || echo MISSING)"
fi

# ─── Test 10: git < 2.31 guard fires (stubbed, since this machine runs   ─
#     git 2.54.0 — see scripts/lib/canonical-root.sh header for why a     ─
#     real old git can't be used here, and why stubbing is the executed  ─
#     substitute for that half of the proof).
real_git="$(command -v git)"
fake_git_dir="$tmpdir/fake-old-git"
mkdir -p "$fake_git_dir"
cat > "$fake_git_dir/git" <<STUB
#!/usr/bin/env bash
# Simulates git < 2.31: rejects --path-format the way git 2.30 and earlier
# actually do (an unrecognized-option error), for every other subcommand
# and flag it forwards to the real git so --is-inside-work-tree still works.
for arg in "\$@"; do
    case "\$arg" in
        --path-format=*)
            echo "error: unknown option \\\`path-format=absolute'" >&2
            exit 129
            ;;
    esac
done
exec "$real_git" "\$@"
STUB
chmod +x "$fake_git_dir/git"

set +e
output="$(cd "$repo" && PATH="$fake_git_dir:$PATH" sh -c "printf 'entry' | '$APPEND'" 2>&1)"
status=$?
set -e
if [ "$status" -eq 1 ] && echo "$output" | grep -qi "unsupported by this git version"; then
    pass "git < 2.31 guard fires with exit 1 (stubbed git, real old git not available on this machine)"
else
    fail "git < 2.31 guard fires with exit 1 (stubbed git)" "status=$status output: $output"
fi

echo
echo "Results: $PASS passed, $FAIL failed"
[ "$FAIL" -eq 0 ]
