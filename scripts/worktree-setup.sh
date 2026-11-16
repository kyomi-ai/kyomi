#!/usr/bin/env bash
# ------------------------------------------------------------------------------
# scripts/worktree-setup.sh — make the CURRENT linked worktree ready to build,
# test and verify. Idempotent; safe to run every time a session starts.
#
# WHY THIS EXISTS
#
# Agents run in a worktree that the Claude desktop app (or `claude --worktree`)
# already created for the session. Creating a second, hand-made sibling
# `kyomi-wt-*` tree on top of it duplicated cold builds, split the work across
# two checkouts and left stale trees behind. This script is the one executable
# answer to "what does a fresh worktree still need?", so no skill or doc has to
# restate the steps.
#
# Run it from inside any worktree of this repository, including the canonical
# clone (it then only reports; it never rewrites the canonical clone's files).
#
# WHAT IT DOES
#   1. Links the canonical clone's `.env` into the worktree (when one exists).
#   2. Pre-creates `crates/kyomi-ui/dist` (RustEmbed needs the directory).
#   3. Fast-forwards a brand-new worktree to `origin/main` when HEAD is a
#      strict ancestor of it (the app may cut a worktree from a stale local
#      `main`). Never touches a worktree that already has its own commits or
#      uncommitted changes; it warns instead.
#   4. Warns when sccache is not the rustc wrapper, or when CARGO_TARGET_DIR is
#      set (a target dir shared by concurrent agents serialises their builds).
#   5. Warns about uncommented relative `../` patch paths in Cargo.toml that do
#      not resolve from this worktree (see the note above the check).
#   6. Prints WORKTREE= and PORT= on stdout. Port = 3100 + (ticket number % 900)
#      from a `kyo-<NN>` branch name; 3100 is reserved for the global dev
#      instance. Without a ticket number the port is derived from the path.
#
# Human-facing status goes to stderr; stdout carries only the two KEY=VALUE
# lines, so `eval "$(scripts/worktree-setup.sh)"` exports them.
#
# Environment: WORKTREE_SETUP_NO_FETCH=1 skips `git fetch` (used by the self-test).
# ------------------------------------------------------------------------------
set -euo pipefail

say() { printf 'worktree-setup: %s\n' "$*" >&2; }

root=$(git rev-parse --show-toplevel)
# The first entry of `git worktree list` is always the canonical clone.
main=$(git -C "$root" worktree list --porcelain | sed -n '1s/^worktree //p')

# 1. .env
if [ "$root" != "$main" ]; then
  if [ -e "$main/.env" ] && [ ! -e "$root/.env" ]; then
    ln -s "$main/.env" "$root/.env"
    say "linked .env from the canonical clone"
  elif [ ! -e "$main/.env" ]; then
    say "no .env in the canonical clone ($main); copy .env.example if the server needs one"
  fi
fi

# 2. RustEmbed directory
mkdir -p "$root/crates/kyomi-ui/dist"

# 3. Stale base
branch=$(git -C "$root" rev-parse --abbrev-ref HEAD 2>/dev/null || echo HEAD)
if [ "$root" != "$main" ] && [ "${WORKTREE_SETUP_NO_FETCH:-0}" != "1" ]; then
  git -C "$root" fetch -q origin main 2>/dev/null || say "WARNING: could not fetch origin/main; base may be stale"
fi
if git -C "$root" rev-parse -q --verify origin/main >/dev/null 2>&1 && [ "$root" != "$main" ]; then
  head=$(git -C "$root" rev-parse HEAD)
  base=$(git -C "$root" rev-parse origin/main)
  if [ "$head" != "$base" ]; then
    if git -C "$root" merge-base --is-ancestor "$head" "$base"; then
      if [ -z "$(git -C "$root" status --porcelain)" ]; then
        git -C "$root" merge -q --ff-only origin/main
        say "fast-forwarded $branch to origin/main ($(git -C "$root" rev-parse --short HEAD))"
      else
        say "WARNING: $branch is behind origin/main but has uncommitted changes; rebase it yourself"
      fi
    else
      ahead=$(git -C "$root" rev-list --count origin/main..HEAD)
      say "$branch has $ahead commit(s) not on origin/main; base is $(git -C "$root" merge-base HEAD origin/main | cut -c1-8)"
    fi
  fi
fi

# 4. Build environment
if ! command -v sccache >/dev/null 2>&1; then
  say "WARNING: sccache not found; every worktree will do a full cold build"
elif [ "${RUSTC_WRAPPER:-}" != "sccache" ] && ! grep -qs 'rustc-wrapper *= *"sccache"' \
  "${CARGO_HOME:-$HOME/.cargo}/config.toml" "$root/.cargo/config.toml"; then
  say "WARNING: sccache is installed but not the rustc wrapper (set RUSTC_WRAPPER=sccache or build.rustc-wrapper)"
fi
if [ -n "${CARGO_TARGET_DIR:-}" ]; then
  say "WARNING: CARGO_TARGET_DIR is set; concurrent worktrees sharing it block each other on the build lock"
fi

# Relative local-dev patch paths (e.g. `../chartml/...`) only resolve when the
# worktree is a sibling of those repos. Keep local overrides out of Cargo.toml:
# put a `[patch.crates-io]` with ABSOLUTE paths in ~/.cargo/config.toml instead.
while IFS= read -r rel; do
  [ -n "$rel" ] || continue
  if [ ! -d "$root/$rel" ]; then
    say "WARNING: Cargo.toml has an active relative patch path '$rel' that does not exist from this worktree; use absolute paths in ~/.cargo/config.toml instead"
  fi
done <<EOT
$(sed -n 's/^[^#]*path *= *"\(\.\.\/[^"]*\)".*/\1/p' "$root/Cargo.toml" | sort -u)
EOT

# 5. Port
ticket=$(printf '%s' "$branch" | sed -n 's/.*[kK][yY][oO]-\{0,1\}\([0-9][0-9]*\).*/\1/p' | head -n1)
if [ -n "$ticket" ]; then
  port=$((3100 + (10#$ticket % 900)))
else
  sum=$(printf '%s' "$root" | cksum | cut -d' ' -f1)
  port=$((3100 + (sum % 900)))
fi
if [ "$port" -eq 3100 ]; then port=3101; fi

printf 'WORKTREE=%s\n' "$root"
printf 'PORT=%s\n' "$port"
