#!/usr/bin/env bash
# ------------------------------------------------------------------------------
# scripts/reconcile-merged-tickets-test.sh — self-test for
# reconcile-merged-tickets.sh (KYO-617)
#
# Follows the shape of scripts/audit-agent-run-deaths-test.sh: fixtures built
# with python3 (JSON is too easy to get subtly wrong hand-quoted in bash,
# especially PR bodies containing embedded newlines), a PASS/FAIL harness with
# assert_* helpers, and the script under test driven entirely through its own
# `--from-file <path>` option. No real `gh` and no network access is used
# anywhere in this file except Test 15 (a genuine `gh pr list` failure), which
# uses a STUB `gh` placed first on PATH — the same technique
# check-ticket-in-flight-test.sh uses — never the real `gh` binary or network.
#
# Because reconcile-merged-tickets.sh deliberately separates stdout (machine-
# readable candidate rows only) from stderr (human status/errors — see its own
# OUTPUT CONTRACT header section), this suite captures the two streams
# independently rather than combining them with 2>&1 the way the simpler
# in-flight/death-audit suites do — collapsing them here would defeat the very
# contract under test.
#
# Exit 0 = all pass, exit 1 = any failure.
# ------------------------------------------------------------------------------

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
SCRIPT="$SCRIPT_DIR/reconcile-merged-tickets.sh"
PASS=0
FAIL=0

tmpdir="$(mktemp -d)"
trap 'rm -rf "$tmpdir"' EXIT

# ─── fixture helpers ─────────────────────────────────────────────────────────
# pr_row <number> <title> <mergedAt> <body> — echoes one JSON object in the
# exact shape `gh pr list --json number,title,mergedAt,body` produces for a
# single PR. Built with python3 (not bash string interpolation) so PR bodies
# containing embedded newlines, quotes, and tabs round-trip exactly.
pr_row() {
    python3 -c '
import json, sys
number, title, merged_at, body = sys.argv[1:5]
print(json.dumps({"number": int(number), "title": title, "mergedAt": merged_at, "body": body}))
' "$1" "$2" "$3" "$4"
}

# write_prs_file <outfile> <pr_row json...> — assembles one or more pr_row
# outputs into a single JSON array file, the shape reconcile-merged-tickets.sh
# expects from --from-file.
write_prs_file() {
    local outfile="$1"
    shift
    python3 -c '
import json, sys
outfile = sys.argv[1]
objs = [json.loads(a) for a in sys.argv[2:]]
with open(outfile, "w") as f:
    json.dump(objs, f)
' "$outfile" "$@"
}

# A recent-past and a far-past timestamp. FUTUREPROOF_TS is a FIXED string —
# safe for every test that runs under the script's default 336h (14-day)
# lookback, since it only needs to stay within two weeks of "now", which is
# true for a long time after this file is written. Test 13 below exercises a
# much NARROWER window (24h) and cannot use a fixed string for its "recent"
# fixture without the suite eventually rotting as real time moves past it —
# it computes RECENT_TS from the real clock instead. ANCIENT_TS is always
# outside any realistic lookback either way.
FUTUREPROOF_TS='2026-09-01T00:00:00Z' # within 336h (14d) of "now" for a long time
ANCIENT_TS='2015-01-01T00:00:00Z'     # always outside any realistic lookback
RECENT_TS="$(date -u -d '-1 hour' +%Y-%m-%dT%H:%M:%SZ)" # always within a 24h window

# ─── invoke the script under test, capturing stdout/stderr SEPARATELY ───────
CHECK_STATUS=""
CHECK_STDOUT=""
CHECK_STDERR=""
run_reconcile() {
    # run_reconcile <fixture-file> [extra args to the script...]
    local fixture="$1" out_file err_file
    shift
    out_file="$(mktemp)"
    err_file="$(mktemp)"
    if "$SCRIPT" --from-file "$fixture" "$@" >"$out_file" 2>"$err_file"; then
        CHECK_STATUS=0
    else
        CHECK_STATUS=$?
    fi
    CHECK_STDOUT="$(cat "$out_file")"
    CHECK_STDERR="$(cat "$err_file")"
    rm -f "$out_file" "$err_file"
}

assert_exit() {
    local name="$1" expected="$2"
    if [ "$CHECK_STATUS" -eq "$expected" ]; then
        printf "  \xe2\x9c\x93 %s (exit %d)\n" "$name" "$CHECK_STATUS"
        PASS=$((PASS + 1))
    else
        printf "  \xe2\x9c\x97 %s \xe2\x80\x94 expected exit %d, got %d\n" "$name" "$expected" "$CHECK_STATUS"
        echo "    stdout:"
        echo "$CHECK_STDOUT" | sed 's/^/    | /'
        echo "    stderr:"
        echo "$CHECK_STDERR" | sed 's/^/    | /'
        FAIL=$((FAIL + 1))
    fi
}

assert_stdout_contains() {
    local name="$1" needle="$2"
    if printf '%s' "$CHECK_STDOUT" | grep -qF -- "$needle"; then
        printf "  \xe2\x9c\x93 %s\n" "$name"
        PASS=$((PASS + 1))
    else
        printf "  \xe2\x9c\x97 %s \xe2\x80\x94 expected stdout to contain: %s\n" "$name" "$needle"
        echo "    stdout:"
        echo "$CHECK_STDOUT" | sed 's/^/    | /'
        FAIL=$((FAIL + 1))
    fi
}

assert_stdout_not_contains() {
    local name="$1" needle="$2"
    if printf '%s' "$CHECK_STDOUT" | grep -qF -- "$needle"; then
        printf "  \xe2\x9c\x97 %s \xe2\x80\x94 expected stdout NOT to contain: %s\n" "$name" "$needle"
        echo "    stdout:"
        echo "$CHECK_STDOUT" | sed 's/^/    | /'
        FAIL=$((FAIL + 1))
    else
        printf "  \xe2\x9c\x93 %s\n" "$name"
        PASS=$((PASS + 1))
    fi
}

assert_stdout_equals() {
    local name="$1" expected="$2"
    if [ "$CHECK_STDOUT" = "$expected" ]; then
        printf "  \xe2\x9c\x93 %s\n" "$name"
        PASS=$((PASS + 1))
    else
        printf "  \xe2\x9c\x97 %s \xe2\x80\x94 stdout did not match exactly\n" "$name"
        echo "    expected:"
        echo "$expected" | sed 's/^/    | /'
        echo "    got:"
        echo "$CHECK_STDOUT" | sed 's/^/    | /'
        FAIL=$((FAIL + 1))
    fi
}

assert_stderr_contains() {
    local name="$1" needle="$2"
    if printf '%s' "$CHECK_STDERR" | grep -qF -- "$needle"; then
        printf "  \xe2\x9c\x93 %s\n" "$name"
        PASS=$((PASS + 1))
    else
        printf "  \xe2\x9c\x97 %s \xe2\x80\x94 expected stderr to contain: %s\n" "$name" "$needle"
        echo "    stderr:"
        echo "$CHECK_STDERR" | sed 's/^/    | /'
        FAIL=$((FAIL + 1))
    fi
}

echo "Running reconcile-merged-tickets self-tests..."
echo

# ─── Test 1: a single "Closes KYO-NN" is extracted ──────────────────────────
echo "-- Test 1: single Closes line"
t1="$tmpdir/t1.json"
write_prs_file "$t1" "$(pr_row 100 "Fix the thing" "$FUTUREPROOF_TS" $'Closes KYO-42\n\nSummary text.')"
run_reconcile "$t1"
assert_exit "single Closes line, exit 0" 0
assert_stdout_equals "emits exactly the one candidate row" "$(printf '100\tFix the thing\t%s\tKYO-42' "$FUTUREPROOF_TS")"
echo

# ─── Test 2: MULTIPLE Closes lines in one PR body are ALL extracted ─────────
echo "-- Test 2: multiple Closes lines, one PR"
t2="$tmpdir/t2.json"
write_prs_file "$t2" "$(pr_row 200 "Batch fix" "$FUTUREPROOF_TS" $'Closes KYO-10\nCloses KYO-11\nCloses KYO-12\n')"
run_reconcile "$t2"
assert_exit "multiple Closes lines, exit 0" 0
assert_stdout_contains "extracts KYO-10" "$(printf '200\tBatch fix\t%s\tKYO-10' "$FUTUREPROOF_TS")"
assert_stdout_contains "extracts KYO-11" "$(printf '200\tBatch fix\t%s\tKYO-11' "$FUTUREPROOF_TS")"
assert_stdout_contains "extracts KYO-12" "$(printf '200\tBatch fix\t%s\tKYO-12' "$FUTUREPROOF_TS")"
line_count="$(printf '%s\n' "$CHECK_STDOUT" | grep -c . || true)"
if [ "$line_count" -eq 3 ]; then
    printf "  \xe2\x9c\x93 emits exactly 3 rows, no more\n"
    PASS=$((PASS + 1))
else
    printf "  \xe2\x9c\x97 expected exactly 3 rows, got %d\n" "$line_count"
    FAIL=$((FAIL + 1))
fi
echo

# ─── Test 3: THE REGRESSION THAT MATTERS MOST — a deferral reference (the ───
# KYO-411 shape: "Relates to KYO-NN", never "Closes") must NOT match.
echo "-- Test 3: deferral reference without Closes is NOT matched (KYO-411 shape)"
t3="$tmpdir/t3.json"
write_prs_file "$t3" "$(pr_row 300 "Unrelated fix" "$FUTUREPROOF_TS" $'Closes KYO-50\n\nRelates to KYO-411\nRelates to KYO-413\nSee also KYO-406 for context.')"
run_reconcile "$t3"
assert_exit "mixed Closes + deferrals, exit 0" 0
assert_stdout_contains "still extracts the real Closes line" "KYO-50"
assert_stdout_not_contains "does not match a Relates-to deferral" "KYO-411"
assert_stdout_not_contains "does not match a second Relates-to deferral" "KYO-413"
assert_stdout_not_contains "does not match a free-text 'See also' mention" "KYO-406"
echo

# ─── Test 4: case variations that MUST match ─────────────────────────────────
echo "-- Test 4: case variations"
t4="$tmpdir/t4.json"
write_prs_file "$t4" \
    "$(pr_row 401 "lower" "$FUTUREPROOF_TS" 'closes kyo-61')" \
    "$(pr_row 402 "upper" "$FUTUREPROOF_TS" 'CLOSES KYO-62')" \
    "$(pr_row 403 "mixed" "$FUTUREPROOF_TS" 'Closes Kyo-63')"
run_reconcile "$t4"
assert_exit "case variations, exit 0" 0
assert_stdout_contains "lowercase 'closes kyo-61' matches" "KYO-61"
assert_stdout_contains "uppercase 'CLOSES KYO-62' matches" "KYO-62"
assert_stdout_contains "mixed-case 'Closes Kyo-63' matches" "KYO-63"
echo

# ─── Test 5: whitespace variations that MUST match ──────────────────────────
echo "-- Test 5: whitespace variations"
t5="$tmpdir/t5.json"
write_prs_file "$t5" \
    "$(pr_row 501 "extra spaces" "$FUTUREPROOF_TS" 'Closes    KYO-71')" \
    "$(pr_row 502 "leading indent" "$FUTUREPROOF_TS" '   Closes KYO-72')" \
    "$(pr_row 503 "trailing space" "$FUTUREPROOF_TS" 'Closes KYO-73   ')" \
    "$(pr_row 504 "tab separated" "$FUTUREPROOF_TS" $'Closes\tKYO-74')" \
    "$(pr_row 505 "trailing period" "$FUTUREPROOF_TS" 'Closes KYO-75.')"
run_reconcile "$t5"
assert_exit "whitespace variations, exit 0" 0
assert_stdout_contains "extra internal spaces still match" "KYO-71"
assert_stdout_contains "leading indentation still matches" "KYO-72"
assert_stdout_contains "trailing whitespace still matches" "KYO-73"
assert_stdout_contains "a tab between Closes and the key still matches" "KYO-74"
assert_stdout_contains "trailing period still matches" "KYO-75"
echo

# ─── Test 6: near-misses that must NOT match ─────────────────────────────────
echo "-- Test 6: near-misses"
t6="$tmpdir/t6.json"
write_prs_file "$t6" \
    "$(pr_row 601 "wrong verb" "$FUTUREPROOF_TS" 'Closing KYO-81')" \
    "$(pr_row 602 "not anchored" "$FUTUREPROOF_TS" 'This PR closes KYO-82 as well.')" \
    "$(pr_row 603 "missing hyphen" "$FUTUREPROOF_TS" 'Closes KYO83')" \
    "$(pr_row 604 "no digits" "$FUTUREPROOF_TS" 'Closes KYO-')" \
    "$(pr_row 605 "wrong prefix" "$FUTUREPROOF_TS" 'Closes TICKET-84')" \
    "$(pr_row 606 "has a real one too" "$FUTUREPROOF_TS" $'Closing KYO-81\nCloses KYO-85')"
run_reconcile "$t6"
assert_exit "near-misses fixture, exit 0" 0
assert_stdout_not_contains "'Closing KYO-81' (wrong verb) does not match" "KYO-81"
assert_stdout_not_contains "'This PR closes KYO-82...' (not anchored) does not match" "KYO-82"
assert_stdout_not_contains "'Closes KYO83' (missing hyphen) does not match" "KYO83"
assert_stdout_not_contains "'Closes KYO-' (no digits) does not match" "604"
assert_stdout_not_contains "'Closes TICKET-84' (wrong prefix) does not match" "KYO-84"
assert_stdout_contains "a real Closes line alongside a near-miss in the same PR still matches" "KYO-85"
echo

# ─── Test 6b: fenced and indented code blocks must NOT match — found in ────
# code review, the worst-class false positive this script can produce (a
# ticket marked Done that was never done). Mirrors Test 6's shape.
echo "-- Test 6b: fenced and indented code blocks"
t6b="$tmpdir/t6b.json"
write_prs_file "$t6b" \
    "$(pr_row 651 "backtick fence" "$FUTUREPROOF_TS" $'```\nCloses KYO-99\n```\nRelates to KYO-100')" \
    "$(pr_row 652 "tilde fence" "$FUTUREPROOF_TS" $'~~~\nCloses KYO-98\n~~~')" \
    "$(pr_row 653 "indented code block" "$FUTUREPROOF_TS" $'Some text.\n\n    Closes KYO-97\n\nMore text.')" \
    "$(pr_row 654 "leading tab" "$FUTUREPROOF_TS" $'\tCloses KYO-96')" \
    "$(pr_row 655 "fence opens and closes, real Closes after" "$FUTUREPROOF_TS" $'```\nsome code\n```\nCloses KYO-201')" \
    "$(pr_row 656 "unterminated fence: real Closes before, fake Closes swallowed inside" "$FUTUREPROOF_TS" $'Closes KYO-202\n\n```\nnever closes\nCloses KYO-999')"
run_reconcile "$t6b"
assert_exit "fenced/indented fixture, exit 0" 0
assert_stdout_not_contains "Closes inside a \`\`\`-fenced block does not match" "KYO-99"
assert_stdout_not_contains "the deferral outside the fence still doesn't match (no Closes keyword)" "KYO-100"
assert_stdout_not_contains "Closes inside a ~~~-fenced block does not match" "KYO-98"
assert_stdout_not_contains "a 4-space-indented Closes line does not match" "KYO-97"
assert_stdout_not_contains "a tab-indented Closes line does not match" "KYO-96"
assert_stdout_contains "a real Closes line AFTER a closed fence still matches (the toggle turns back off)" "$(printf '655\tfence opens and closes, real Closes after\t%s\tKYO-201' "$FUTUREPROOF_TS")"
assert_stdout_contains "a real Closes line BEFORE an unterminated fence still matches" "$(printf '656\tunterminated fence: real Closes before, fake Closes swallowed inside\t%s\tKYO-202' "$FUTUREPROOF_TS")"
assert_stdout_not_contains "a Closes line swallowed inside an unterminated fence does not match" "KYO-999"
echo

# ─── Test 7: a listing returned at EXACTLY PR_LIST_LIMIT rows fails closed ──
# The truncation guard. Uses a small limit so the fixture doesn't need
# hundreds of rows to exercise it.
echo "-- Test 7: truncation guard, listing at exactly the limit"
t7="$tmpdir/t7.json"
write_prs_file "$t7" \
    "$(pr_row 701 "a" "$FUTUREPROOF_TS" '')" \
    "$(pr_row 702 "b" "$FUTUREPROOF_TS" '')" \
    "$(pr_row 703 "c" "$FUTUREPROOF_TS" '')"
PR_LIST_LIMIT=3 run_reconcile "$t7"
assert_exit "a listing at exactly the limit must exit non-zero, never 0" 3
assert_stderr_contains "names the limit it hit" "PR_LIST_LIMIT of 3"
assert_stderr_contains "tells the operator to raise it" "re-run with a higher PR_LIST_LIMIT"
assert_stdout_equals "stdout is empty on a failed check — nothing is trustworthy to print" ""
echo

# ─── Test 8: just under the limit succeeds — the guard must not false-fire ──
echo "-- Test 8: just under the limit"
t8="$tmpdir/t8.json"
write_prs_file "$t8" \
    "$(pr_row 801 "a" "$FUTUREPROOF_TS" '')" \
    "$(pr_row 802 "b" "$FUTUREPROOF_TS" '')"
PR_LIST_LIMIT=3 run_reconcile "$t8"
assert_exit "a listing under the limit succeeds" 0
echo

# ─── Test 9: a gh failure exits non-zero, never reports an empty result ────
# Uses a STUB `gh` on PATH, invoking the script WITHOUT --from-file, exactly
# like check-ticket-in-flight-test.sh's fail-closed tests. Never touches the
# real `gh` or the network.
echo "-- Test 9: gh pr list failure (real invocation path, stub gh)"
STUB_BIN="$tmpdir/bin"
mkdir -p "$STUB_BIN"
cat >"$STUB_BIN/gh" <<'STUB'
#!/usr/bin/env bash
echo "gh: authentication required (stub failure)" >&2
exit 1
STUB
chmod +x "$STUB_BIN/gh"
if out="$(PATH="$STUB_BIN:$PATH" "$SCRIPT" --lookback-hours 24 2>"$tmpdir/t9.stderr")"; then
    CHECK_STATUS=0
else
    CHECK_STATUS=$?
fi
CHECK_STDOUT="$out"
CHECK_STDERR="$(cat "$tmpdir/t9.stderr")"
assert_exit "a failing gh must exit 3, never 0" 3
assert_stderr_contains "names gh pr list as the failing step" "gh pr list failed"
assert_stdout_equals "stdout is empty — a gh failure must not look like a real empty result" ""
echo

# ─── Test 10: malformed JSON is not a false success ─────────────────────────
echo "-- Test 10: malformed JSON input"
t10="$tmpdir/t10.json"
printf 'not json at all {{{' >"$t10"
run_reconcile "$t10"
assert_exit "malformed JSON must exit non-zero" 3
assert_stdout_equals "stdout is empty on malformed input" ""
echo

# ─── Test 11: an empty file is also not a false success ─────────────────────
echo "-- Test 11: empty file"
t11="$tmpdir/t11.json"
: >"$t11"
run_reconcile "$t11"
assert_exit "an empty file must exit non-zero" 3
echo

# ─── Test 12: a genuinely empty listing ("[]") IS a legitimate success ──────
# Distinguishes "malformed/unreadable" (Tests 10-11, must fail) from
# "genuinely nothing merged this window" (must succeed with empty stdout) —
# the exact distinction docs/standards/error-handling/
# empty-on-failure-must-not-look-like-a-real-result.md exists to protect.
echo "-- Test 12: valid empty array is a legitimate quiet result"
t12="$tmpdir/t12.json"
printf '[]' >"$t12"
run_reconcile "$t12"
assert_exit "a real empty listing exits 0" 0
assert_stdout_equals "stdout is empty" ""
assert_stderr_contains "summary says zero candidates, not silence" "0 candidate"
echo

# ─── Test 13: the lookback window actually filters, and is printed ─────────
echo "-- Test 13: lookback window filtering"
t13="$tmpdir/t13.json"
write_prs_file "$t13" \
    "$(pr_row 1301 "recent" "$RECENT_TS" 'Closes KYO-90')" \
    "$(pr_row 1302 "ancient" "$ANCIENT_TS" 'Closes KYO-91')"
run_reconcile "$t13" --lookback-hours 24
assert_exit "narrow window, exit 0" 0
assert_stdout_contains "recent PR within the window is included" "KYO-90"
assert_stdout_not_contains "ancient PR outside the window is excluded" "KYO-91"
assert_stderr_contains "prints the lookback hours used" "lookback=24h"
assert_stderr_contains "prints the computed cutoff" "cutoff="
echo "-- Test 13b: same fixture, a very wide window includes both"
run_reconcile "$t13" --lookback-hours 1000000
assert_exit "wide window, exit 0" 0
assert_stdout_contains "recent PR still included under a wide window" "KYO-90"
assert_stdout_contains "ancient PR now included under a wide enough window" "KYO-91"
echo

# ─── Test 14: usage errors ───────────────────────────────────────────────────
echo "-- Test 14: usage errors"
run_reconcile "$t1" --bogus-flag
assert_exit "unknown flag" 2
if out="$("$SCRIPT" --from-file 2>&1)"; then CHECK_STATUS=0; else CHECK_STATUS=$?; fi
assert_exit "--from-file with no value" 2
run_reconcile "$tmpdir/does-not-exist.json"
assert_exit "--from-file pointing at a missing path" 2
run_reconcile "$t1" --lookback-hours not-a-number
assert_exit "non-numeric --lookback-hours" 2
run_reconcile "$t1" --lookback-hours 0
assert_exit "zero --lookback-hours" 2
run_reconcile "$t1" --lookback-hours -5
assert_exit "negative --lookback-hours" 2
PR_LIST_LIMIT=not-a-number run_reconcile "$t1"
assert_exit "non-numeric PR_LIST_LIMIT" 2
PR_LIST_LIMIT=0 run_reconcile "$t1"
assert_exit "zero PR_LIST_LIMIT" 2
echo

# ─── Test 15: a row that cannot be minimally understood fails closed ───────
echo "-- Test 15: unreadable PR row"
t15a="$tmpdir/t15a.json"
printf '[{"title":"no number field","mergedAt":"%s","body":"Closes KYO-95"}]' "$FUTUREPROOF_TS" >"$t15a"
run_reconcile "$t15a"
assert_exit "a row missing 'number' exits non-zero, is not silently skipped" 3
assert_stdout_equals "stdout is empty — a partial result is not printed" ""

t15b="$tmpdir/t15b.json"
printf '[{"number":9999,"title":"bad date","mergedAt":"not-a-timestamp","body":"Closes KYO-96"}]' >"$t15b"
run_reconcile "$t15b"
assert_exit "a row with an unparseable mergedAt exits non-zero" 3
assert_stderr_contains "names the offending PR" "PR #9999"
echo

# ─── Test 16: non-object entries and a non-array top level both fail closed ─
echo "-- Test 16: malformed shapes"
t16a="$tmpdir/t16a.json"
printf '[1, 2, 3]' >"$t16a"
run_reconcile "$t16a"
assert_exit "an array of non-objects exits non-zero" 3

t16b="$tmpdir/t16b.json"
printf '{"not": "an array"}' >"$t16b"
run_reconcile "$t16b"
assert_exit "a JSON object instead of an array exits non-zero" 3
assert_stderr_contains "says why" "not a top-level array"
echo

# ─── Test 17: dedup — the same ticket named twice in one body emits once ───
echo "-- Test 17: duplicate Closes line for the same ticket is deduped"
t17="$tmpdir/t17.json"
write_prs_file "$t17" "$(pr_row 1700 "dup" "$FUTUREPROOF_TS" $'Closes KYO-97\nCloses KYO-97\n')"
run_reconcile "$t17"
assert_exit "duplicate Closes line, exit 0" 0
dup_count="$(printf '%s\n' "$CHECK_STDOUT" | grep -c 'KYO-97' || true)"
if [ "$dup_count" -eq 1 ]; then
    printf "  \xe2\x9c\x93 KYO-97 emitted exactly once, not twice\n"
    PASS=$((PASS + 1))
else
    printf "  \xe2\x9c\x97 expected KYO-97 exactly once, got %d\n" "$dup_count"
    FAIL=$((FAIL + 1))
fi
echo

# ─── Test 18: multi-PR real-world shape, and the stdout/stderr split holds ──
echo "-- Test 18: realistic multi-PR fixture, output contract"
t18="$tmpdir/t18.json"
write_prs_file "$t18" \
    "$(pr_row 1801 "Ships the feature" "$FUTUREPROOF_TS" $'Closes KYO-501\n\n## Summary\nDid the thing.')" \
    "$(pr_row 1802 "No tickets here" "$FUTUREPROOF_TS" 'Just a refactor, nothing to close.')" \
    "$(pr_row 1803 "Deferred work only" "$FUTUREPROOF_TS" $'Relates to KYO-502\n\nInvestigated but out of scope.')"
run_reconcile "$t18"
assert_exit "realistic mixed fixture, exit 0" 0
assert_stdout_contains "the real close is present" "$(printf '1801\tShips the feature\t%s\tKYO-501' "$FUTUREPROOF_TS")"
assert_stdout_not_contains "the PR with no ticket contributes nothing" "1802"
assert_stdout_not_contains "the deferral-only PR contributes nothing" "KYO-502"
assert_stdout_not_contains "stdout never carries the human summary line" "Reconcile summary"
assert_stdout_not_contains "stdout never carries the window banner" "Reconciling merged PRs"
assert_stderr_contains "stderr carries the human summary instead" "Reconcile summary"
echo

# ─── Tests 19-22: --print-gh-args — the fetch is genuinely window-bounded ──
#
# The coordinator's fix-round requirement: nothing else in this suite proves
# the `gh pr list` invocation actually carries a date-bounded `--search`
# string rather than a plain `--state merged --limit N` fetch of the whole
# repo's history — every other test bypasses the fetch entirely via
# --from-file. These tests invoke the real script with NO --from-file and a
# `gh` stub, placed first on PATH, that ALWAYS fails loudly with a
# distinctive marker if it is ever actually invoked — so --print-gh-args's
# own claim (it exits before ever calling `gh`) is verified, not assumed. If
# a future change accidentally moved the fetch ahead of the --print-gh-args
# check, or dropped the --search construction, these would fail loudly
# instead of silently reverting to the repo-history-bound cliff.
NO_GH_BIN="$tmpdir/no-gh-bin"
mkdir -p "$NO_GH_BIN"
cat >"$NO_GH_BIN/gh" <<'STUB'
#!/usr/bin/env bash
echo "TEST FAILURE MARKER: gh was invoked; --print-gh-args must never call gh" >&2
exit 111
STUB
chmod +x "$NO_GH_BIN/gh"

run_print_gh_args() {
    # run_print_gh_args [extra args...] — invokes the script with the
    # always-fails `gh` stub first on PATH (real bash/coreutils/python3
    # remain reachable via the rest of $PATH, unlike a PATH pointed at only
    # this one directory, which would also break the script's own
    # `#!/usr/bin/env bash` shebang lookup).
    local out_file err_file
    out_file="$(mktemp)"
    err_file="$(mktemp)"
    if PATH="$NO_GH_BIN:$PATH" "$SCRIPT" --print-gh-args "$@" >"$out_file" 2>"$err_file"; then
        CHECK_STATUS=0
    else
        CHECK_STATUS=$?
    fi
    CHECK_STDOUT="$(cat "$out_file")"
    CHECK_STDERR="$(cat "$err_file")"
    rm -f "$out_file" "$err_file"
}

echo "-- Test 19: --print-gh-args never calls gh, and carries --search"
run_print_gh_args --lookback-hours 12
assert_exit "--print-gh-args exits 0 with a gh stub that would fail loudly if called" 0
assert_stdout_contains "carries --state" "--state"
assert_stdout_contains "carries the merged state value" "merged"
assert_stdout_contains "carries --search" "--search"
assert_stdout_contains "carries --limit" "--limit"
assert_stdout_contains "carries --json with the four fields this script reads" "number,title,mergedAt,body"
if printf '%s' "$CHECK_STDERR" | grep -qF "TEST FAILURE MARKER"; then
    printf "  \xe2\x9c\x97 the gh stub WAS invoked -- --print-gh-args called gh\n"
    echo "$CHECK_STDERR" | sed 's/^/    | /'
    FAIL=$((FAIL + 1))
else
    printf "  \xe2\x9c\x93 the gh stub was never invoked (no marker in stderr)\n"
    PASS=$((PASS + 1))
fi
search_line="$(printf '%s\n' "$CHECK_STDOUT" | grep -E '^merged:>=[0-9]{4}-[0-9]{2}-[0-9]{2}T[0-9]{2}:[0-9]{2}:[0-9]{2}Z$' || true)"
if [ -n "$search_line" ]; then
    printf "  \xe2\x9c\x93 the --search value is a date-bounded 'merged:>=<ISO-8601 UTC timestamp>' qualifier, not a bare --state fetch\n"
    PASS=$((PASS + 1))
else
    printf "  \xe2\x9c\x97 expected a 'merged:>=<ISO-8601 timestamp>' line in stdout, found none\n"
    echo "    stdout:"
    echo "$CHECK_STDOUT" | sed 's/^/    | /'
    FAIL=$((FAIL + 1))
fi
echo

echo "-- Test 20: the --search cutoff actually tracks --lookback-hours"
now_year="$(date -u +%Y)"
run_print_gh_args --lookback-hours 1
narrow_cutoff="$(printf '%s\n' "$CHECK_STDOUT" | grep -oE 'merged:>=[0-9]{4}' | head -n1)"
narrow_cutoff="${narrow_cutoff#merged:>=}"
if [ "$narrow_cutoff" = "$now_year" ]; then
    printf "  \xe2\x9c\x93 a 1-hour lookback's cutoff falls in the current year (%s)\n" "$now_year"
    PASS=$((PASS + 1))
else
    printf "  \xe2\x9c\x97 expected a 1-hour lookback's cutoff year to be %s, got '%s'\n" "$now_year" "$narrow_cutoff"
    FAIL=$((FAIL + 1))
fi

run_print_gh_args --lookback-hours 1000000
wide_cutoff="$(printf '%s\n' "$CHECK_STDOUT" | grep -oE 'merged:>=[0-9]{4}' | head -n1)"
wide_cutoff="${wide_cutoff#merged:>=}"
if [ -n "$wide_cutoff" ] && [ "$wide_cutoff" -lt 2020 ] 2>/dev/null; then
    printf "  \xe2\x9c\x93 a ~114-year lookback's cutoff falls well before this repo existed (%s)\n" "$wide_cutoff"
    PASS=$((PASS + 1))
else
    printf "  \xe2\x9c\x97 expected a ~114-year lookback's cutoff year to be < 2020, got '%s'\n" "$wide_cutoff"
    FAIL=$((FAIL + 1))
fi

if [ -n "$narrow_cutoff" ] && [ -n "$wide_cutoff" ] && [ "$narrow_cutoff" != "$wide_cutoff" ]; then
    printf "  \xe2\x9c\x93 --lookback-hours genuinely changes the --search cutoff (not a hardcoded string)\n"
    PASS=$((PASS + 1))
else
    printf "  \xe2\x9c\x97 --lookback-hours 1 and --lookback-hours 1000000 produced the same (or empty) cutoff year\n"
    FAIL=$((FAIL + 1))
fi
echo

echo "-- Test 21: --print-gh-args reflects a PR_LIST_LIMIT override"
if PATH="$NO_GH_BIN:$PATH" PR_LIST_LIMIT=77 "$SCRIPT" --print-gh-args >"$tmpdir/t21.out" 2>"$tmpdir/t21.err"; then
    CHECK_STATUS=0
else
    CHECK_STATUS=$?
fi
CHECK_STDOUT="$(cat "$tmpdir/t21.out")"
CHECK_STDERR="$(cat "$tmpdir/t21.err")"
assert_exit "PR_LIST_LIMIT override, exit 0" 0
assert_stdout_contains "the overridden limit appears in the printed args" "77"
echo

echo "-- Test 22: --print-gh-args is silent on stderr"
run_print_gh_args --lookback-hours 24
assert_exit "plain --print-gh-args, exit 0" 0
# scripts/reconcile-merged-tickets.sh sources scripts/lib/stale-tooling-guard.sh
# (KYO-632), which may legitimately write its own diagnostic to stderr —
# every line of it starts with the fixed "[stale-tooling-guard]" prefix
# (see that file's STALE_TOOLING_GUARD_LOG_PREFIX). That diagnostic is
# expected and is not what this test guards against; what it guards against
# is $SCRIPT itself printing something unexpected here. Filter the guard's
# own lines out before checking for silence, so this assertion keeps its
# original meaning instead of becoming permanently unsatisfiable the moment
# this checkout's copy of the script differs from (or can't be compared
# against) origin/main — which is true of every PR that touches this
# script, including the one that added the guard.
non_guard_stderr="$(printf '%s\n' "$CHECK_STDERR" | grep -v '^\[stale-tooling-guard\]' || true)"
if [ -z "$non_guard_stderr" ]; then
    printf "  \xe2\x9c\x93 --print-gh-args produces no stderr output of its own (guard diagnostics aside)\n"
    PASS=$((PASS + 1))
else
    printf "  \xe2\x9c\x97 expected no non-guard stderr output, got:\n"
    echo "$non_guard_stderr" | sed 's/^/    | /'
    FAIL=$((FAIL + 1))
fi
echo

# ─── summary ──────────────────────────────────────────────────────────────
echo "============================================"
echo "Results: $PASS passed, $FAIL failed"
if [ "$FAIL" -gt 0 ]; then
    exit 1
fi
exit 0
