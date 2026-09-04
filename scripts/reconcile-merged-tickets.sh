#!/usr/bin/env bash
# ------------------------------------------------------------------------------
# scripts/reconcile-merged-tickets.sh — find merged PRs whose ticket was never
# moved to Done, because it merged some way other than /merge-sweeper. (KYO-617)
#
# WHY THIS EXISTS
#
# /merge-sweeper is the ONLY thing that moves a ticket from In Review to Done,
# and it finds work by enumerating OPEN PRs (`gh pr list --state open`). A PR
# merged by any other route — by hand on GitHub, `gh pr merge` run directly,
# another tool — is closed before the next sweep, so it is never enumerated
# and its ticket is never updated. Nothing reconciles a merged PR against a
# stale ticket, so it sits In Review forever looking like unfinished work, and
# comes back at the top of every priority-ranked `agent-ready` backlog sweep.
#
# Eight tickets were found stranded this way across nine days (KYO-516 and
# KYO-517 both priority 1). The evidence is the merge-time distribution: those
# eight merged at scattered times, while a genuine sweeper run merged
# #452/#453/#454 within 30 seconds of each other at the cron slot — and those
# tickets were correctly set to Done. Sweeper-merged -> Done; merged any other
# way -> stranded. The correlation is exact.
#
# WHAT THIS SCRIPT DOES, AND WHAT IT DELIBERATELY DOES NOT DO
#
# This is Option 1 from the ticket: reconciliation happens on this side (a
# script /merge-sweeper's skill calls), not via a GitHub Action on
# `pull_request: closed` (Option 2, rejected — it would need a Trakkt token in
# this PUBLIC repo's Actions secrets, which is a credential decision the
# ticket itself flags as needing security review, not an implementation one).
# Running both would race two writers to set the same ticket Done; only one
# may exist.
#
# It ENUMERATES and REPORTS. It never writes to Trakkt. The Trakkt write stays
# agent-side, in the calling skill, for two reasons: (1) the skill already
# holds those credentials, so this script needs none; (2) idempotency and
# "don't overwrite a ticket a human already moved by hand" both require
# reading the ticket's CURRENT status first, which the skill can do via MCP
# and a shell script cannot. This script's only job is to hand the skill a
# correct candidate list of (PR number, PR title, merged-at, ticket key) —
# what to go check — never a verdict about what to do with a ticket.
#
# A PR whose body says "Relates to KYO-NN" instead of "Closes KYO-NN" is
# invisible to this script by design (see MATCHING below) — reconciling that
# shape is KYO-608, already filed, deliberately out of scope here.
#
# MATCHING — "Closes KYO-NN" ONLY, ANCHORED, NEVER A FREE-TEXT SEARCH
#
# Do NOT use `gh pr list --search "KYO-NN"` or any other free-text match
# against the PR body. This repo's own convention (CLAUDE.md, "When You
# Discover Something Mid-Task") REQUIRES a PR body to link every ticket it
# considered and deferred, not just the one it implemented — so a
# well-behaved, already-merged PR routinely mentions a ticket it never
# touched. Free-text matching on ticket ID produced exactly this false-hit
# shape for KYO-411, KYO-413 and KYO-406, all merged PRs that merely LISTED
# those tickets as deferrals in the same body that closed something else.
# scripts/check-ticket-in-flight.sh hit the identical trap searching PR
# bodies (see its own header, KYO-471) and fixed it by matching branch name
# instead; a merged PR's branch is gone/renamed by the time this script runs,
# so branch matching isn't available here — the fix here is instead to
# anchor on the literal `Closes` keyword, at the start of the (trimmed) line,
# case-insensitively, one ticket per line — the exact shape this repo's own
# PR template requires (see CLAUDE.md's "PR body" rule under Trakkt Ticket
# References). "Relates to KYO-NN", "Closing KYO-NN" (wrong verb), and
# "...this closes KYO-NN..." (not anchored at line start) all correctly do
# NOT match. Multiple `Closes KYO-NN` lines in one body are each extracted —
# they are one per line by convention, never comma-separated — so
# "Closes KYO-1 and KYO-2" on one line matches NEITHER ticket (the anchor
# requires nothing but optional trailing punctuation after the key), and
# likewise "Closes KYO-99 (fixes the race)" — trailing prose after the key —
# does NOT match either. Both are convention violations in the PR body, not
# bugs in this matcher: CLAUDE.md's own template is one bare `Closes KYO-NN`
# per line, nothing else on it.
#
# FENCED AND INDENTED CODE BLOCKS ARE SKIPPED — FOUND IN KYO-617'S OWN
# REVIEW, READ THIS BEFORE "SIMPLIFYING" THE SCANNER BACK TO A FLAT LOOP
#
# CLOSES_RE is applied per-line with no fence awareness would match
# `Closes KYO-99` sitting inside a ```-fenced or ~~~-fenced code block, or
# inside a 4-space-indented one — i.e. text a PR body is QUOTING or
# DOCUMENTING, not actually closing. This is not a hypothetical shape: a
# live sample of 50 of this repo's own recently-merged PR bodies showed 20
# of 50 (40%) contain fenced code blocks, and this repo's OWN `CLAUDE.md`
# documents the `Closes KYO-NN` convention inside a fenced example block —
# so any PR that edits or quotes that section of CLAUDE.md is a concrete,
# guaranteed trigger, not an edge case. A false positive here is strictly
# worse than the miss this whole script exists to fix: a stranded In Review
# ticket is visible and recoverable; a wrongly-Done ticket silently leaves
# real work unfinished and drops out of every backlog sweep — the same
# asymmetry CLAUDE.md's own "deferred" label trap warns about. The scanning
# loop below therefore tracks fence state while walking `body.splitlines()`
# and skips CLOSES_RE matching entirely for any line inside an open fence,
# a fence delimiter line itself, or a line indented 4+ spaces (or a leading
# tab) — see the loop's own comments for the exact toggle rules, why the
# indentation check MUST run on the UNTRIMMED line (trimming first is
# exactly what let the indented case slip through originally), and the
# deliberate choice for an unterminated fence.
#
# FAIL CLOSED (KYO-511, and
# docs/standards/error-handling/empty-on-failure-must-not-look-like-a-real-result.md)
#
# A false "nothing to reconcile" is worse than useless here — it renders
# exactly like a quiet week, and a stranded ticket stays stranded with nobody
# any the wiser. So any check this script cannot complete — `gh` failing, a
# malformed or truncated PR listing, a PR row it cannot minimally understand
# (no `number`, no parseable `mergedAt`) — exits non-zero rather than
# printing an empty candidate list. Never pipe a status-bearing command into
# `wc -l`/`grep -c`/`head` and discard its exit status; every fallible
# command here is captured via `if out="$(cmd)"; then ... else ... fi`, whose
# `if` inspects that command's own exit status directly.
#
# THE FETCH IS BOUNDED BY THE LOOKBACK WINDOW, NOT BY REPO HISTORY — READ
# THIS BEFORE "SIMPLIFYING" IT BACK TO A PLAIN `--state merged` FETCH
#
# `gh pr list` has no server-side merge-date filter of its own, but `--search`
# does: GitHub's search API accepts a `merged:` qualifier. This script builds
# `--search "merged:>=<cutoff>"` (verified live against this repo, see below)
# so the FETCH ITSELF returns only PRs merged within the lookback window,
# instead of pulling the newest `PR_LIST_LIMIT` merged PRs in the WHOLE
# REPO'S HISTORY and discarding most of them client-side. The difference is
# not cosmetic: this repo has 460 merged PRs today and gains several a day.
# A plain `--state merged --limit 500` fetch (the first version of this
# script) will, once the repo passes 500 merged PRs total, return EXACTLY
# `PR_LIST_LIMIT` rows on every single run forever — tripping the truncation
# guard below permanently and taking reconciliation down for good, which is
# exactly the "stops working and leaves tickets stranded" failure class this
# ticket exists to fix. Verified live (2026-09-03, this repo):
# `--search "merged:>=2026-09-02"` returned 21 rows (vs. 460 unfiltered), and
# `--search "merged:>=2027-01-01"` — a date with nothing merged after it —
# correctly returned zero, proving the filter is genuinely applied
# server-side and not a coincidence of `gh`'s default sort order.
#
# `--search` was independently verified to accept a full ISO-8601 UTC
# timestamp (`merged:>=2026-09-02T14:47:23Z`), not just a bare
# `YYYY-MM-DD` date, and to honour hour-level granularity: it returned
# exactly the 7 PRs a 12-hour lookback should return, matching this script's
# own client-side count for the identical window. GitHub's public docs for
# the `merged:` qualifier only document day granularity, though — the
# ISO-8601 acceptance is observed CLI behaviour, not a documented contract,
# and GitHub's search index is also documented as eventually consistent
# (indexing lag on the order of seconds to minutes is normal for their
# search API). BELT AND BRACES IS THEREFORE DELIBERATE, NOT REDUNDANT: the
# server-side `--search` bounds how many rows come back — turning
# `PR_LIST_LIMIT` back into a meaningful guard against genuine in-window
# volume — while the client-side `merged_ts < cutoff` comparison in the
# embedded parser below remains the AUTHORITATIVE cutoff, using this
# script's own documented, tested parsing of the exact `mergedAt` field `gh`
# returns. Do not remove either half: dropping the server-side filter
# reintroduces the repo-history-bound cliff above; dropping the client-side
# one trades a tested, documented comparison for an external service's
# undocumented granularity and consistency guarantees.
#
# THE PR-LISTING LIMIT IS LOAD-BEARING, THE SAME WAY IT IS IN
# check-ticket-in-flight.sh: `gh pr list` silently truncates at `--limit`
# (default 30). `PR_LIST_LIMIT` (env-overridable, same name and convention as
# check-ticket-in-flight.sh's own guard) bounds the request, and a listing
# that comes back at >= that many rows is treated as POSSIBLY TRUNCATED and
# fails closed (exit 3) rather than silently reporting only part of the
# window. Now that the fetch itself is window-bounded (see above), hitting
# this guard means "more PRs merged in this window than we asked `gh` for" —
# a real, actionable condition — rather than "the repo has grown past an
# arbitrary constant". Raising the number alone is still not a substitute for
# the guard — it only moves the threshold — so both halves are required: a
# named, overridable constant, and a check that the returned row count did
# not hit it.
#
# THE LOOKBACK WINDOW IS BOUNDED AND STATED, NOT UNBOUNDED
#
# `--lookback-hours` (default 336h / 14 days — double the 9-day spread the
# KYO-617 incident evidence actually showed, so the default comfortably
# covers a normal reconciliation cadence with headroom) bounds how far back a
# PR's `mergedAt` may be and still be considered, and is now the SAME value
# used to build both the server-side `--search` qualifier and the
# client-side comparison — computed once, into `$CUTOFF`, before either is
# used, so the two can never disagree. It is printed on stderr on every run
# so a caller can see exactly what window was searched. A one-off backfill of
# an OLDER stranded ticket (there is no guarantee every zombie found during
# KYO-617's own triage is within 14 days by the time this ships) needs an
# explicit wider `--lookback-hours` — that is an operational choice for the
# caller, not something this script should guess at by defaulting to
# "unbounded".
#
# `--print-gh-args` prints the exact `gh pr list ...` argument list this run
# would use — one argument per line, so the date-bounded `--search` string
# (which itself contains a space) round-trips exactly — and exits 0 without
# calling `gh`, reading `--from-file`, or touching the network. It exists
# solely so scripts/reconcile-merged-tickets-test.sh can assert the fetch is
# still genuinely window-bounded without shelling out to the real `gh`;
# nothing about normal operation uses it.
#
# OUTPUT CONTRACT — STDOUT IS MACHINE-READABLE, STDERR IS FOR HUMANS
#
# Same split as scripts/mine-review-logs.sh: stdout carries ONLY the
# candidate rows, one per line, tab-separated, in the order
# `PR_NUMBER\tPR_TITLE\tMERGED_AT\tTICKET` — nothing else ever goes to
# stdout, so a caller can consume it directly without stripping banner text.
# An EMPTY stdout on exit 0 is a legitimate "nothing to reconcile this run"
# result (a quiet window is not an error) — see EXIT CODES below for how
# that is told apart from "this run could not be completed". All
# human-facing status (the window used, the PR count, error detail) goes to
# stderr.
#
# TESTABILITY — FETCH IS SEPARATE FROM PARSE (KYO-617's own requirement)
#
# `--from-file <path>` reads a pre-fetched `gh pr list --json
# number,title,mergedAt,body` JSON array from PATH instead of invoking `gh`
# at all — the exact mechanism scripts/audit-agent-run-deaths.sh already uses
# for the identical reason (KYO-546). The parsing/matching/truncation-guard
# logic itself lives entirely in the embedded Python block below, which
# reads only that JSON file plus the (already-computed) cutoff timestamp and
# limit — it never shells out to `gh` or `git`, so
# scripts/reconcile-merged-tickets-test.sh drives every extraction, matching,
# and fail-closed case entirely offline, through fixture files, with zero
# network access, and passes identically in CI.
#
# `--print-gh-args` covers the OTHER half — that the fetch itself is
# genuinely window-bounded (see THE FETCH IS BOUNDED BY THE LOOKBACK WINDOW
# above). The `gh pr list` invocation is built once into the `GH_ARGS` array
# so there is exactly one place that constructs it; `--print-gh-args` prints
# that array, one element per line, and exits before it would otherwise be
# passed to `gh`, so the self-test can assert the built argument list
# actually carries a date-bounded `--search` string without needing a stub
# `gh` or any network access for this particular check.
#
# Only Python's standard-library `json` module does any JSON parsing — no
# standalone `jq` dependency, matching the precedent already set by
# scripts/audit-agent-run-deaths.sh (this repo already ships stdlib-only
# Python helpers under scripts/, e.g. scripts/e2e-regression/seed-test-user.py,
# so this is an established pattern, not a new one) — rather than
# check-ticket-in-flight.sh's choice of gh's own bundled `--jq`, which isn't
# available once the JSON is coming from a fixture file instead of `gh`
# itself. `shellcheck` may not be installed on every box (KYO-514); quoting
# throughout is deliberately defensive rather than lint-verified.
#
# USAGE
#
#   reconcile-merged-tickets.sh [--lookback-hours N] [--from-file <path>]
#                                [--print-gh-args]
#
#   --lookback-hours N   only consider PRs merged within the last N hours
#                        (default 336 = 14 days). Must be a positive integer.
#   --from-file PATH     read the PR listing from PATH instead of invoking
#                        `gh pr list` (see TESTABILITY above). PATH must
#                        contain a JSON array in the exact shape `gh pr list
#                        --json number,title,mergedAt,body` produces.
#   --print-gh-args      print the `gh pr list ...` argument list this run
#                        would use, one per line, and exit 0 without calling
#                        `gh` or reading --from-file (see TESTABILITY above).
#
# Environment:
#   PR_LIST_LIMIT        how many merged PRs to fetch (default 500). Must
#                        exceed the number of PRs plausibly merged within the
#                        lookback window; a listing that comes back at this
#                        many rows may be truncated and exits 3 — same
#                        convention as check-ticket-in-flight.sh's own
#                        PR_LIST_LIMIT.
#
# EXIT CODES
#
#   0  the check completed. Stdout carries zero or more candidate rows —
#      EMPTY stdout on exit 0 legitimately means nothing to reconcile this
#      run, not a failure.
#   2  usage error (unknown flag, --from-file with no value or pointing at an
#      unreadable path, a malformed --lookback-hours or PR_LIST_LIMIT).
#   3  the check could not be completed: `gh pr list` failed, the PR listing
#      is not valid JSON (or not a JSON array), the listing came back at
#      PR_LIST_LIMIT rows and may therefore be truncated, or at least one PR
#      row could not be minimally understood (missing `number`, or a missing
#      / unparseable `mergedAt` — see FAIL CLOSED above: such a row might
#      actually be in-window, so it is never silently skipped). Treat exactly
#      like a real finding was missed — never trust stdout from a non-zero
#      exit, and never treat 3 as "nothing to reconcile".
#  42  this script's own on-disk content is stale relative to origin/main
#      AND KYOMI_STALE_TOOLING_STRICT=1 is set. See
#      scripts/lib/stale-tooling-guard.sh (KYO-632) — by default this is a
#      loud warning on stderr, not a failure.
# ------------------------------------------------------------------------------

set -euo pipefail

SCRIPT_NAME="$(basename "$0")"
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=lib/stale-tooling-guard.sh
source "${SCRIPT_DIR}/lib/stale-tooling-guard.sh"
stale_tooling_guard "${BASH_SOURCE[0]}"

LOOKBACK_HOURS_DEFAULT=336 # 14 days — see LOOKBACK WINDOW note above
PR_LIST_LIMIT="${PR_LIST_LIMIT:-500}"

usage() {
    cat >&2 <<EOF
Usage: $SCRIPT_NAME [--lookback-hours N] [--from-file <path>] [--print-gh-args]

  --lookback-hours N   only consider PRs merged within the last N hours
                       (default ${LOOKBACK_HOURS_DEFAULT} = 14 days)
  --from-file PATH     read the PR listing from PATH instead of invoking
                       \`gh pr list\` (see script header — used by the
                       self-test, requires no network access)
  --print-gh-args      print the \`gh pr list ...\` argument list this run
                       would use, one per line, and exit 0 (see script
                       header — used by the self-test, requires no gh call)

Environment:
  PR_LIST_LIMIT         how many merged PRs to fetch (default 500). Must
                        exceed the number of PRs plausibly merged within the
                        lookback window; a listing that comes back at this
                        many rows may be truncated and exits 3.

Output:
  stdout carries ONLY candidate rows, tab-separated:
    PR_NUMBER<TAB>PR_TITLE<TAB>MERGED_AT<TAB>TICKET
  All human-facing status and errors go to stderr.

Exit codes:
  0  completed — stdout may legitimately be empty (nothing to reconcile)
  2  usage error
  3  could not complete the check — never treat like exit 0
EOF
}

case "$PR_LIST_LIMIT" in
    '' | *[!0-9]* | 0)
        echo "ERROR: PR_LIST_LIMIT must be a positive integer, got '$PR_LIST_LIMIT'" >&2
        exit 2
        ;;
esac

LOOKBACK_HOURS="$LOOKBACK_HOURS_DEFAULT"
FROM_FILE=""
PRINT_GH_ARGS=0

while [ "$#" -gt 0 ]; do
    case "$1" in
        --lookback-hours)
            if [ "$#" -lt 2 ]; then
                echo "ERROR: --lookback-hours requires a value" >&2
                exit 2
            fi
            LOOKBACK_HOURS="$2"
            shift 2
            ;;
        --from-file)
            if [ "$#" -lt 2 ]; then
                echo "ERROR: --from-file requires a value" >&2
                exit 2
            fi
            FROM_FILE="$2"
            shift 2
            ;;
        --print-gh-args)
            PRINT_GH_ARGS=1
            shift
            ;;
        *)
            echo "ERROR: unknown argument: $1" >&2
            usage
            exit 2
            ;;
    esac
done

case "$LOOKBACK_HOURS" in
    '' | *[!0-9]* | 0)
        echo "ERROR: --lookback-hours must be a positive integer, got '$LOOKBACK_HOURS'" >&2
        exit 2
        ;;
esac

if [ -n "$FROM_FILE" ] && { [ ! -f "$FROM_FILE" ] || [ ! -r "$FROM_FILE" ]; }; then
    echo "ERROR: --from-file path does not exist or is not readable: $FROM_FILE" >&2
    exit 2
fi

if ! command -v python3 >/dev/null 2>&1; then
    echo "ERROR: python3 is required to parse the PR listing and is not on PATH" >&2
    exit 3
fi

# ---- cutoff timestamp -------------------------------------------------------
# Computed once, here, so both branches below (gh fetch and --from-file) and
# the printed status line agree on exactly the same instant. GNU `date -d` is
# assumed — this repo's scripts already assume a Linux dev box and
# ubuntu-latest CI throughout (see e.g. audit-agent-run-deaths.sh, which
# assumes GNU journalctl); there is no macOS/BSD target here.
if ! CUTOFF="$(date -u -d "-${LOOKBACK_HOURS} hours" +%Y-%m-%dT%H:%M:%SZ)"; then
    echo "ERROR: could not compute a cutoff timestamp for --lookback-hours $LOOKBACK_HOURS" >&2
    exit 3
fi

# ---- gh pr list argument list ------------------------------------------------
# Built ONCE, here, into an array, so the real fetch below and
# --print-gh-args (the self-test's hook — see TESTABILITY in the header) can
# never drift apart into two different invocations. `--search
# "merged:>=$CUTOFF"` is what makes the fetch itself window-bounded instead
# of repo-history-bounded — see THE FETCH IS BOUNDED BY THE LOOKBACK WINDOW
# in the header for the live verification behind this exact query string.
# `--state merged` is kept alongside it (redundant with `merged:` but
# explicit and harmless) purely to match this repo's existing convention of
# always naming --state explicitly on a `gh pr list` call.
GH_ARGS=(pr list --state merged --search "merged:>=${CUTOFF}" --limit "$PR_LIST_LIMIT" --json number,title,mergedAt,body)

if [ "$PRINT_GH_ARGS" -eq 1 ]; then
    printf '%s\n' "${GH_ARGS[@]}"
    exit 0
fi

# ---- fetch (or read the fixture) --------------------------------------------
RAW_FILE=""
CLEANUP_RAW_FILE=0
cleanup() {
    if [ "$CLEANUP_RAW_FILE" -eq 1 ] && [ -n "$RAW_FILE" ]; then
        rm -f "$RAW_FILE"
    fi
}
trap cleanup EXIT

if [ -n "$FROM_FILE" ]; then
    RAW_FILE="$FROM_FILE"
else
    RAW_FILE="$(mktemp)"
    CLEANUP_RAW_FILE=1
    gh_stderr_file="$(mktemp)"
    if ! gh "${GH_ARGS[@]}" \
        >"$RAW_FILE" 2>"$gh_stderr_file"; then
        echo "ERROR: gh pr list failed:" >&2
        cat "$gh_stderr_file" >&2
        rm -f "$gh_stderr_file"
        exit 3
    fi
    rm -f "$gh_stderr_file"
fi

echo "Reconciling merged PRs: lookback=${LOOKBACK_HOURS}h, cutoff=${CUTOFF} (UTC), PR_LIST_LIMIT=${PR_LIST_LIMIT}${FROM_FILE:+, source=$FROM_FILE}" >&2

# ---- hand off to the embedded parser ----------------------------------------
# The parser owns the exit-code decision (0 completed / 3 could not complete)
# for everything downstream of "we have a PR listing to look at" — usage
# errors (2) and a failed `gh pr list` (3) are already handled above. This
# block is the ENTIRE "parse bodies and emit candidates" half the header's
# TESTABILITY section describes: it touches no network and no `gh`/`git`
# binary, only the JSON file named by RAW_FILE, so the self-test exercises it
# byte-for-byte identically whether that file came from a real `gh` call or a
# fixture.
if python3 - "$RAW_FILE" "$CUTOFF" "$PR_LIST_LIMIT" <<'PYEOF'
import json
import re
import sys
from datetime import datetime, timezone

RAW_PATH = sys.argv[1]
CUTOFF_STR = sys.argv[2]
LIMIT = int(sys.argv[3])

# The exact canonical shape `gh`'s --json emits for a UTC timestamp field.
# Fixed-width, no fractional seconds, no numeric offset — matches
# check-ticket-in-flight.sh's own is_iso8601_z convention for the identical
# reason: two strings of this shape order identically as strings or as
# instants, so no `date -d` parsing of untrusted PR data is needed here.
TS_RE = re.compile(r"^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}Z$")


def parse_ts(value):
    """Returns an aware UTC datetime, or None if `value` is not exactly the
    canonical shape above. FAILS CLOSED on purpose — see the caller, which
    treats a row it cannot parse a mergedAt for as UNREADABLE, never as
    'must be out of window'."""
    if not isinstance(value, str) or not TS_RE.match(value):
        return None
    return datetime.strptime(value, "%Y-%m-%dT%H:%M:%SZ").replace(tzinfo=timezone.utc)


cutoff = parse_ts(CUTOFF_STR)
if cutoff is None:
    print(f"ERROR: internal error — cutoff '{CUTOFF_STR}' is not a canonical UTC timestamp", file=sys.stderr)
    sys.exit(3)

# CLOSES_RE — see the MATCHING section of the script header for why this is
# anchored on the literal "Closes" keyword at the START of a (trimmed) line,
# case-insensitively, rather than any free-text search of the body. Matches:
#   "Closes KYO-19", "closes kyo-19", "CLOSES   KYO-19", "Closes KYO-19."
# Does NOT match:
#   "Relates to KYO-19"   (KYO-411/413/406 shape — a deferral, not a close)
#   "Closing KYO-19"      (wrong verb)
#   "This PR closes KYO-19 too"  (not anchored at line start)
#   "Closes KYO19"        (missing hyphen — not a valid ticket key shape)
#   "Closes TICKET-19"    (wrong team prefix)
#   "Closes KYO-1 and KYO-2"       (more than one ticket on one line)
#   "Closes KYO-99 (fixes the race)"  (trailing prose after the key)
# Applied only to lines the scanning loop below has already determined are
# NOT inside a fenced or indented code block — see FENCED AND INDENTED CODE
# BLOCKS ARE SKIPPED in the header for why that gate exists.
CLOSES_RE = re.compile(r"^closes\s+kyo-(\d+)\s*[.,:;]?\s*$", re.IGNORECASE)

# FENCE_RE — a fenced-code-block delimiter line: 0-3 leading spaces (a fence
# indented 4+ spaces is itself inside an outer indented code block, not a
# fence — matched by the indentation check in the loop below instead), then
# 3-or-more of the SAME fence character, backtick or tilde (CommonMark
# allows both; captured so the loop can require the CLOSING fence use the
# same character — a ``` block is not closed by a ~~~ line).
FENCE_RE = re.compile(r"^ {0,3}(`{3,}|~{3,})")


def extract_closed_tickets(body):
    """Scan `body` line by line and return the list of KYO-N ticket keys
    named by a bare `Closes KYO-N` line, in order, EXCLUDING any such line
    that sits inside a fenced (``` or ~~~) or 4-space-indented code block —
    see FENCED AND INDENTED CODE BLOCKS ARE SKIPPED in the header for why.

    THE INDENTATION CHECK RUNS ON THE UNTRIMMED LINE, DELIBERATELY. Trimming
    first — what an earlier version of this scanner did — throws away the
    exact signal ("4+ leading spaces") that marks an indented code block, so
    a trimmed `    Closes KYO-97` is byte-identical to a real top-level
    `Closes KYO-97` and both would match. CLOSES_RE itself is still applied
    to the STRIPPED line (trailing punctuation/whitespace tolerance is a
    separate, deliberate concern — see CLOSES_RE's own comment) — only the
    fence/indentation GATE that decides whether to even attempt that match
    looks at the untrimmed line.

    UNTERMINATED FENCE — DELIBERATE CHOICE, READ BEFORE "FIXING" IT BACK. If
    a fence opens and this body ends before it closes (malformed markdown —
    a truncated paste, usually), every line for the remainder of THIS body
    is treated as still inside the fence, so a `Closes KYO-N` line after a
    stray/unterminated opening delimiter is NOT matched. The alternative —
    assume an unterminated fence "doesn't count" and keep matching normally
    — would let a stray triple-backtick anywhere in a body silently launder
    a false positive back in, which is exactly the shape this function
    exists to close off. A missed candidate here just leaves that PR's
    ticket visibly In Review (recoverable); this mirrors the same
    false-positive-is-worse-than-false-negative priority explained in the
    header. Fence state is local to one call (one PR body) and always starts
    "not in a fence", so this never bleeds into the next PR.
    """
    tickets = []
    in_fence = None  # None = not in a fence; else the fence char ('`' or '~')
    for raw_line in body.splitlines():
        if in_fence is not None:
            m_fence = FENCE_RE.match(raw_line)
            if m_fence and m_fence.group(1)[0] == in_fence:
                in_fence = None
            continue  # every line while fenced is skipped, including the closer itself

        m_fence = FENCE_RE.match(raw_line)
        if m_fence:
            in_fence = m_fence.group(1)[0]
            continue  # the opening delimiter line itself is never a Closes line

        # Indented code block: 4+ leading spaces, or a leading tab (treated
        # as a tab stop, i.e. equivalent to 4 spaces — this is a heuristic,
        # not full CommonMark tab-expansion, and deliberately does not try
        # to be exact about mixed space/tab indentation beyond this).
        if raw_line.startswith("    ") or raw_line.startswith("\t"):
            continue

        m = CLOSES_RE.match(raw_line.strip())
        if not m:
            continue
        tickets.append(f"KYO-{int(m.group(1))}")
    return tickets

try:
    with open(RAW_PATH, "r", encoding="utf-8") as f:
        raw = f.read()
except OSError as e:
    print(f"ERROR: could not read PR listing at {RAW_PATH}: {e}", file=sys.stderr)
    sys.exit(3)

try:
    data = json.loads(raw)
except json.JSONDecodeError as e:
    print(f"ERROR: PR listing is not valid JSON: {e}", file=sys.stderr)
    sys.exit(3)

if not isinstance(data, list):
    print("ERROR: PR listing JSON is not a top-level array", file=sys.stderr)
    sys.exit(3)

total = len(data)

# TRUNCATION GUARD — see the header's PR-LISTING LIMIT section. A count at or
# above LIMIT means the request may have been cut off by `gh pr list
# --limit`, so any older merged PR within the lookback window may simply be
# missing from this data. Checked before anything else touches `data`, so a
# truncated fetch can never partially "succeed".
if total >= LIMIT:
    print(
        f"ERROR: PR listing returned {total} row(s), at PR_LIST_LIMIT of {LIMIT} — "
        f"the listing may be truncated, so an older merged PR could be missing from "
        f"this window; re-run with a higher PR_LIST_LIMIT (e.g. PR_LIST_LIMIT={LIMIT * 2})",
        file=sys.stderr,
    )
    sys.exit(3)

candidates = []  # (number, safe_title, merged_at, ticket)
unreadable = []
in_window = 0
prs_with_candidates = 0

for entry in data:
    if not isinstance(entry, dict):
        unreadable.append(f"non-object row: {entry!r}")
        continue

    number = entry.get("number")
    if not isinstance(number, int) or isinstance(number, bool):
        unreadable.append(f"row missing an integer 'number': {entry!r}")
        continue

    merged_at = entry.get("mergedAt")
    merged_ts = parse_ts(merged_at)
    if merged_ts is None:
        # FAIL CLOSED (see docstring above and the header's FAIL CLOSED
        # section): this row's window membership cannot be determined, so it
        # is never silently dropped as "must be out of window" — it aborts
        # the whole run instead, the same way check-ticket-in-flight.sh's
        # per-row parse failures force exit 3 rather than skipping the row.
        unreadable.append(f"PR #{number}: missing/unparseable mergedAt ({merged_at!r})")
        continue

    # BELT AND BRACES, DELIBERATELY (see THE FETCH IS BOUNDED BY THE LOOKBACK
    # WINDOW in the script header): the bash wrapper's `gh pr list --search
    # "merged:>=$CUTOFF"` already asks the server to bound the fetch to this
    # same window, but this comparison against the SAME cutoff, applied to
    # `gh`'s own returned `mergedAt` field, is the authoritative filter — the
    # server-side qualifier's exact boundary/consistency behaviour is not a
    # documented GitHub contract at this granularity. Do not drop this on the
    # theory that the search query above already covers it.
    if merged_ts < cutoff:
        continue
    in_window += 1

    title = entry.get("title")
    if not isinstance(title, str):
        title = ""
    body = entry.get("body")
    if not isinstance(body, str):
        body = ""

    # Dedup within one PR: a body that names the same ticket via more than
    # one "Closes KYO-N" line (unusual, but not forbidden by the convention)
    # emits that (PR, ticket) pair exactly once — the caller acts on a
    # ticket, not on a line count. extract_closed_tickets already excludes
    # anything inside a fenced or indented code block (see its own
    # docstring, and FENCED AND INDENTED CODE BLOCKS ARE SKIPPED in the
    # header) before this loop ever sees a ticket key.
    seen_tickets = set()
    pr_had_candidate = False
    for ticket in extract_closed_tickets(body):
        if ticket in seen_tickets:
            continue
        seen_tickets.add(ticket)
        pr_had_candidate = True
        # Titles can't contain a real newline from the GitHub API, but a
        # stray tab (or, defensively, a newline) would corrupt the one-row-
        # per-line TSV contract stdout promises — flattened to a space here
        # rather than escaped, since nothing downstream needs to reconstruct
        # the original title byte-for-byte.
        safe_title = title.replace("\t", " ").replace("\n", " ").replace("\r", " ")
        candidates.append((number, safe_title, merged_at, ticket))
    if pr_had_candidate:
        prs_with_candidates += 1

if unreadable:
    print(
        f"ERROR: {len(unreadable)} PR row(s) in the listing could not be read — "
        "this reconciliation pass is INCOMPLETE, not empty:",
        file=sys.stderr,
    )
    for u in unreadable:
        print(f"  - {u}", file=sys.stderr)
    sys.exit(3)

# ---- stdout: candidates ONLY, nothing else (see OUTPUT CONTRACT above) -----
for number, safe_title, merged_at, ticket in candidates:
    print(f"{number}\t{safe_title}\t{merged_at}\t{ticket}")

print(
    f"Reconcile summary: {total} merged PR(s) fetched, {in_window} within the "
    f"lookback window (cutoff {CUTOFF_STR}), {len(candidates)} candidate "
    f"ticket-closure(s) from {prs_with_candidates} PR(s).",
    file=sys.stderr,
)
sys.exit(0)
PYEOF
then
    exit_code=0
else
    exit_code=$?
fi

exit "$exit_code"
