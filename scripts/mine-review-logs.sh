#!/usr/bin/env bash
# ------------------------------------------------------------------------------
# scripts/mine-review-logs.sh — locate recent code-review logs for standards
# mining, from any worktree.
#
# WHY THIS EXISTS (KYO-373 / KYO-386)
#
# KYO-373 moved the "mine docs/CODING_STANDARDS.md from the last 7 days of
# docs/review-logs/*.md" step into the ticket worktree, so the resulting
# standards commit lands on the ticket branch instead of stranding on local
# main. That fix is correct and must not be reverted.
#
# But docs/review-logs/ is gitignored (.gitignore: `docs/*` with
# `!docs/product/`) — review logs describe code-review findings, many of
# them still-open weaknesses, and this repo is public (see CLAUDE.md
# "Where things live"). So the directory only ever exists in the canonical
# clone that has actually run reviews; it is untracked and therefore absent
# from every fresh clone and every worktree. The mining step used to be
# guarded by "if docs/review-logs/ exists", which never matched in a
# worktree and made the whole step a silent, permanent no-op (KYO-386) —
# every /backlog-fast and /kyomi-backlog run since KYO-373 skipped standards
# mining without reporting it.
#
# The fix here is NOT to track the logs (that would publish internal audit
# findings, some describing unfixed issues, to a public repo — needs a human
# redaction pass, see CLAUDE.md) and NOT to just edit the instruction files
# to hardcode the canonical clone's path (those files — .claude/build-test.md,
# ~/.claude/skills/*/SKILL.md — are themselves gitignored or outside any
# repo, so an edit there is invisible to every other machine and
# unverifiable by CI). Instead: a script tracked in THIS repo, present in
# every worktree on every machine, that resolves the canonical clone
# dynamically and fails loudly (rather than silently) when it can't find the
# logs. The caller (the mining step) is expected to treat a non-zero exit as
# "report the skip explicitly", not "continue as if nothing happened".
#
# HOW THE CANONICAL CLONE IS RESOLVED
#
# Delegated to scripts/lib/canonical-root.sh, which this script sources
# below. That file is also sourced by scripts/append-review-log.sh
# (KYO-396) — the write-side counterpart to this read-side script — so the
# resolution exists in exactly one place instead of being hand-copied into
# each script that needs it. See that file's header for the full
# `--path-format=absolute` rationale and the git-version guard.
#
# EXIT CODES
#   0 — docs/review-logs/ was found (possibly with zero logs in the window;
#       that is printed as a note on stderr, not an error). Matching log
#       paths (if any) are on stdout, one per line, oldest to newest.
#   1 — usage error: bad argument, or not inside a git repository at all.
#   2 — docs/review-logs/ does not exist in the canonical clone. This is the
#       loud-failure path the whole script exists for: the caller MUST
#       treat this as "standards mining was skipped" and report it, not
#       swallow it and continue silently.
#
# USAGE
#   scripts/mine-review-logs.sh [days]
#     days   optional positive integer, default 7. Selects review logs
#            named YYYY-MM-DD.md whose date is within the last `days` days
#            (inclusive of today).
# ------------------------------------------------------------------------------

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=lib/canonical-root.sh
source "${SCRIPT_DIR}/lib/canonical-root.sh"

days="${1:-7}"

if ! [[ "$days" =~ ^[0-9]+$ ]] || [ "$days" -eq 0 ]; then
    echo "ERROR: argument must be a positive integer number of days (got: '${days}')." >&2
    echo "Usage: $0 [days]" >&2
    exit 1
fi

# resolve_canonical_root() / resolve_review_logs_dir() already print a
# diagnostic to stderr on failure (either "not inside a git repository" or
# "unsupported git version" — see scripts/lib/canonical-root.sh). Both map
# to usage-error exit 1 here, matching this script's own contract. Called
# twice (once for the error message below, once for the actual path) rather
# than derived from one another with `dirname`, so this script never
# hardcodes how many path segments resolve_review_logs_dir appends.
if ! canonical_root=$(resolve_canonical_root); then
    exit 1
fi
review_logs_dir=$(resolve_review_logs_dir)

if [ ! -d "$review_logs_dir" ]; then
    {
        echo "ERROR: ${review_logs_dir} does not exist."
        echo ""
        echo "docs/review-logs/ is gitignored (.gitignore: 'docs/*' minus 'docs/product/'),"
        echo "so review logs live ONLY in the canonical clone that has actually run code"
        echo "reviews (${canonical_root}) — they are never committed and never present in"
        echo "a fresh clone or a worktree of one that hasn't accumulated any yet."
        echo ""
        echo "This is expected the first time a clone is used, before any review has run."
        echo "It is NOT safe to treat as 'no standards to mine' and continue silently:"
        echo "report the skip explicitly to the user/ticket rather than proceeding as if"
        echo "standards mining happened."
    } >&2
    exit 2
fi

cutoff=$(date -d "${days} days ago" +%Y-%m-%d)

echo "==> Canonical review-logs dir: ${review_logs_dir}" >&2
echo "==> Window: last ${days} day(s), cutoff >= ${cutoff}" >&2

matches=()
for f in "$review_logs_dir"/*; do
    [ -e "$f" ] || continue
    name=$(basename "$f")
    if [[ "$name" =~ ^([0-9]{4}-[0-9]{2}-[0-9]{2})\.md$ ]]; then
        file_date="${BASH_REMATCH[1]}"
        if [[ "$file_date" > "$cutoff" || "$file_date" == "$cutoff" ]]; then
            matches+=("${file_date}:${f}")
        fi
    fi
done

if [ "${#matches[@]}" -eq 0 ]; then
    echo "==> No review logs found in the last ${days} day(s) — a quiet week, not an error." >&2
    exit 0
fi

echo "==> ${#matches[@]} matching log(s):" >&2

# Sort oldest -> newest by the date prefix, then print only the path.
#
# The `sort` runs through an intermediate variable, NOT `mapfile -t sorted <
# <(... | sort)`, on purpose: process substitution's exit status is invisible
# to the parent shell (only `mapfile`'s own status is checked, and `mapfile`
# succeeds as long as it can read from the FD regardless of what fed it) —
# `set -e` will NOT catch a failing `sort` there. That reintroduces exactly
# the silent-failure class this script exists to avoid (see "WHY THIS EXISTS"
# above). Capturing to `sorted_str` first puts `sort` directly in the `||`
# chain so a failure aborts loudly, matching the fail-loud intent throughout
# this script. Do not "simplify" this back to process substitution.
#
# `mapfile -t sorted <<< "$sorted_str"` on an EMPTY string yields a
# one-element array (a single empty string), not an empty array. That can't
# happen here only because the zero-matches guard above already `exit 0`s
# before this point — if that guard is ever moved or removed, this here-string
# will need an explicit empty check reinstated.
sorted_str=$(printf '%s\n' "${matches[@]}" | sort) || { echo "ERROR: sort failed unexpectedly" >&2; exit 1; }
mapfile -t sorted <<< "$sorted_str"
for entry in "${sorted[@]}"; do
    path="${entry#*:}"
    echo "$path"
done
