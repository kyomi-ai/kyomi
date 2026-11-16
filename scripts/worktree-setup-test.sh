#!/usr/bin/env bash
# scripts/worktree-setup-test.sh — self-test for worktree-setup.sh.
# Real throwaway repos, a real bare origin and real linked worktrees under a
# mktemp -d removed on exit. Branch names are pinned (`init -b main`).
set -uo pipefail

here=$(cd "$(dirname "$0")" && pwd)
script="$here/worktree-setup.sh"
tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT
fail=0
check() { # name, expected, actual
  if [ "$2" = "$3" ]; then echo "ok   $1"; else echo "FAIL $1: expected '$2' got '$3'"; fail=1; fi
}
g() { git -c user.email=t@t -c user.name=t "$@"; }

git init -q --bare -b main "$tmp/origin.git"
git init -q -b main "$tmp/main"
cd "$tmp/main"
g remote add origin "$tmp/origin.git"
mkdir -p crates/kyomi-ui
echo a > a.txt && g add . && g commit -qm one && g push -q origin main
echo secret > .env
echo .env >> .git/info/exclude

# Worktree cut from the current (soon stale) main.
g worktree add -q -b jason/kyo-123-thing "$tmp/wt" main
# origin/main moves ahead.
echo b > b.txt && g add . && g commit -qm two && g push -q origin main
g reset -q --hard HEAD~1

cd "$tmp/wt"
out=$(WORKTREE_SETUP_NO_FETCH=0 "$script" 2>"$tmp/err")
check "ticket port 3100+123" "PORT=3223" "$(printf '%s\n' "$out" | sed -n 2p)"
check "reports worktree root" "WORKTREE=$(git rev-parse --show-toplevel)" "$(printf '%s\n' "$out" | sed -n 1p)"
check ".env linked" "secret" "$(cat .env 2>/dev/null)"
check "dist pre-created" "yes" "$([ -d crates/kyomi-ui/dist ] && echo yes || echo no)"
check "stale base fast-forwarded" "$(git rev-parse origin/main)" "$(git rev-parse HEAD)"
second=$("$script" 2>/dev/null)
check "idempotent output" "$out" "$second"

# A worktree with its own commit is never moved.
cd "$tmp/main"
g worktree add -q -b jason/kyo-5-own "$tmp/wt2" origin/main
cd "$tmp/wt2" && echo c > c.txt && g add . && g commit -qm own
echo d > d.txt && g add . && g commit -qm more >/dev/null
before=$(git rev-parse HEAD)
"$script" >/dev/null 2>&1
check "own commits untouched" "$before" "$(git rev-parse HEAD)"

# No ticket number: port derived from path, never 3100, within range.
cd "$tmp/main"
g worktree add -q -b jason/misc "$tmp/wt3" origin/main
p=$(cd "$tmp/wt3" && "$script" 2>/dev/null | sed -n 2p | cut -d= -f2)
if [ "$p" -ge 3101 ] && [ "$p" -le 3999 ]; then check "fallback port in range" ok ok; else check "fallback port in range" ok "$p"; fi

# An active relative patch path that does not resolve is reported.
cd "$tmp/wt3"
printf '[patch.crates-io]\nfoo = { path = "../nope/foo" }\n' >> Cargo.toml
"$script" >/dev/null 2>"$tmp/err3"
check "warns on unresolved relative patch" "1" "$(grep -c "relative patch path '../nope/foo'" "$tmp/err3")"
git checkout -q Cargo.toml 2>/dev/null || rm -f Cargo.toml

# The canonical clone is only reported on, never rewritten.
cd "$tmp/main"
"$script" >/dev/null 2>&1
check "canonical clone .env unchanged" "secret" "$(cat .env)"

exit $fail
