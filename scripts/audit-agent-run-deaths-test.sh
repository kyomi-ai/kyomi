#!/usr/bin/env bash
# ------------------------------------------------------------------------------
# scripts/audit-agent-run-deaths-test.sh — self-test for
# audit-agent-run-deaths.sh (KYO-546)
#
# Follows the shape of check-ticket-in-flight-test.sh: a fresh mktemp -d,
# synthetic fixtures instead of the real journal (which will not contain
# these fixtures on someone else's machine), and a PASS/FAIL harness with
# assert_exit / assert_contains. The script under test reads fixtures via
# its own `--from-file <path>` option, so no real `journalctl` call is
# needed for any of these tests — only Test 12 below (journalctl actually
# failing) uses the real journalctl binary, and skips itself if journalctl
# is not on PATH.
#
# Every fixture is built by shelling out to python3 for JSON encoding
# (see emit_line below) rather than hand-quoting JSON in bash — the payloads
# being tested (real "type":"result" objects) contain nested quotes and
# braces that bash string interpolation would get wrong, and getting this
# wrong would defeat the point of the test.
#
# Exit 0 = all pass, exit 1 = any failure.
# ------------------------------------------------------------------------------

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
SCRIPT="$SCRIPT_DIR/audit-agent-run-deaths.sh"
PASS=0
FAIL=0

tmpdir="$(mktemp -d)"
trap 'rm -rf "$tmpdir"' EXIT

# ─── fixture helpers ─────────────────────────────────────────────────────────
# emit_line <outfile> <pid> <kind: OUT|END> <content> <ts>
# Appends one journalctl-shaped JSONL record: {"_PID":pid,
# "MESSAGE":"(jason) CMD<kind> (<content>)", "__REALTIME_TIMESTAMP":ts} —
# exactly the shape `journalctl -o json
# --output-fields=_PID,MESSAGE,__REALTIME_TIMESTAMP` produces, which is what
# audit-agent-run-deaths.sh's --from-file expects.
emit_line() {
    local outfile="$1" pid="$2" kind="$3" content="$4" ts="$5"
    python3 -c '
import json, sys
outfile, pid, kind, content, ts = sys.argv[1:6]
message = "(jason) CMD" + kind + " (" + content + ")"
with open(outfile, "a") as f:
    f.write(json.dumps({"_PID": pid, "MESSAGE": message, "__REALTIME_TIMESTAMP": ts}) + "\n")
' "$outfile" "$pid" "$kind" "$content" "$ts"
}
emit_cmdout() { emit_line "$1" "$2" "OUT" "$3" "$4"; }  # outfile pid content ts
emit_cmdend() { emit_line "$1" "$2" "END" "$3" "$4"; }  # outfile pid script ts

# result_json <session_id> <num_turns> <stop_reason> <terminal_reason> <subtype>
#             <spawned> <req_bg> <req_fg> <started_bg> <completed> <failed>
#             <killed_system> <mode>
#   mode: "full" (normal), "no_stats" (omit subagent_stats entirely),
#         "no_killed" (subagent_stats present, .killed missing)
# Echoes one complete "type":"result" JSON object, matching the real shape
# observed in the journal (a subset of the real fields — only what this
# script reads, plus enough padding fields to look realistic).
result_json() {
    python3 -c '
import json, sys
(session_id, num_turns, stop_reason, terminal_reason, subtype,
 spawned, req_bg, req_fg, started_bg, completed, failed, killed_system, mode) = sys.argv[1:14]
obj = {
    "is_error": False,
    "duration_api_ms": 123456,
    "num_turns": int(num_turns),
    "stop_reason": stop_reason,
    "session_id": session_id,
    "total_cost_usd": 1.23,
    "terminal_reason": terminal_reason,
    "subtype": subtype,
    "api_error_status": None,
    "result": "Priority: Normal (3) — some text with (parens) and \"quotes\" in it.",
    "type": "result",
    "uuid": "00000000-0000-0000-0000-000000000000",
}
if mode != "no_stats":
    stats = {
        "spawned": int(spawned),
        "requested": {"background": int(req_bg), "foreground": int(req_fg), "unset": 0},
        "started_in_background": int(started_bg),
        "max_depth": 1,
        "completed": int(completed),
        "failed": int(failed),
        "refused": {"depth_limit": 0, "concurrency_limit": 0, "budget": 0},
    }
    if mode != "no_killed":
        stats["killed"] = {"parent": 0, "user": 0, "system": int(killed_system)}
    obj["subagent_stats"] = stats
print(json.dumps(obj))
' "$@"
}

# task_notification_json <session_id> — a realistic non-"result" JSON object
# that legitimately appears in the same CMDOUT stream, to prove it does not
# confuse the "type":"result" filter.
task_notification_json() {
    python3 -c '
import json, sys
print(json.dumps({"type": "system", "subtype": "task_notification", "task_id": "abc",
                   "status": "stopped", "summary": "doing work", "session_id": sys.argv[1]}))
' "$1"
}

# ─── invoke the script under test ───────────────────────────────────────────
CHECK_STATUS=""
CHECK_OUTPUT=""
run_audit() {
    # run_audit <fixture-file> [extra args...]
    local fixture="$1" out
    shift
    if out="$("$SCRIPT" --from-file "$fixture" "$@" 2>&1)"; then
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

echo "Running audit-agent-run-deaths self-tests..."
echo

# ─── Test 1: a clean run ─────────────────────────────────────────────────────
echo "-- Test 1: clean run"
t1="$tmpdir/t1.jsonl"
: >"$t1"
r1="$(result_json "aaaaaaaa-0000-0000-0000-000000000001" 24 end_turn completed success 2 0 2 0 2 0 0 full)"
emit_cmdout "$t1" 1001 "$r1" 1700000000000000
emit_cmdend "$t1" 1001 "/home/jason/.local/bin/kyomi-backlog-cron.sh" 1700000000000000
run_audit "$t1"
assert_exit "a clean run with no background sub-agents exits 0" 0
assert_contains "found exactly one run" "Found 1 kyomi cron run(s)"
assert_contains "verdict says clean" "RESULT: CLEAN"
echo

# ─── Test 2: THE KYO-546 acceptance criterion — a background sub-agent ─────
# killed at end_turn while the run still reports success.
echo "-- Test 2: killed run (the KYO-468 death pattern)"
t2="$tmpdir/t2.jsonl"
: >"$t2"
r2="$(result_json "bbbbbbbb-0000-0000-0000-000000000002" 2 end_turn completed success 2 1 0 2 1 0 1 full)"
emit_cmdout "$t2" 2002 "Background tasks still running after 600s; terminating." 1700000001000000
emit_cmdout "$t2" 2002 "$r2" 1700000001000000
emit_cmdend "$t2" 2002 "/home/jason/.local/bin/kyomi-backlog-cron.sh" 1700000001000000
run_audit "$t2"
assert_exit "a run with killed.system > 0 exits 1" 1
assert_contains "verdict names the session" "session=bbbbbbbb-0000-0000-0000-000000000002"
assert_contains "verdict shows killed.system=1" "killed.system=1"
assert_contains "verdict result line" "RESULT: 1 of 1 run(s) silently discarded"
echo

# ─── Test 3: background sub-agents that all completed — must NOT be flagged
echo "-- Test 3: background sub-agents, all completed, not killed"
t3="$tmpdir/t3.jsonl"
: >"$t3"
r3="$(result_json "cccccccc-0000-0000-0000-000000000003" 40 end_turn completed success 2 2 0 2 2 0 0 full)"
emit_cmdout "$t3" 3003 "$r3" 1700000002000000
emit_cmdend "$t3" 3003 "/home/jason/.local/bin/kyomi-backlog-cron.sh" 1700000002000000
run_audit "$t3"
assert_exit "background agents that finish before end_turn are not a death" 0
assert_contains "verdict says clean" "RESULT: CLEAN"
assert_not_contains "no killed.system entry in the verdict" "killed a background sub-agent"
echo

# ─── Test 4: malformed / empty journal — must fail closed, never exit 0 ────
echo "-- Test 4a: empty journal file"
t4a="$tmpdir/t4a.jsonl"
: >"$t4a"
run_audit "$t4a"
assert_exit "an empty journal must exit 3, never 0" 3
assert_contains "names the reason" "NO RUNS PARSED"
echo

echo "-- Test 4b: journal full of garbage, no valid records"
t4b="$tmpdir/t4b.jsonl"
{
    echo "not json at all"
    echo '{"this": "is valid json but not a journal record shape"}'
    echo "{{{ broken json"
} >"$t4b"
run_audit "$t4b"
assert_exit "a garbage journal must exit 3, never 0" 3
assert_contains "names the reason" "NO RUNS PARSED"
echo

# ─── Test 5: indeterminate — result JSON present but subagent_stats missing
echo "-- Test 5: indeterminate (no subagent_stats at all)"
t5="$tmpdir/t5.jsonl"
: >"$t5"
r5="$(result_json "dddddddd-0000-0000-0000-000000000005" 10 end_turn completed success 0 0 0 0 0 0 0 no_stats)"
emit_cmdout "$t5" 5005 "$r5" 1700000003000000
emit_cmdend "$t5" 5005 "/home/jason/.local/bin/kyomi-backlog-cron.sh" 1700000003000000
run_audit "$t5"
assert_exit "missing subagent_stats must exit 3, never 0" 3
assert_contains "names the reason" "could not be determined"
assert_contains "explains why" "no subagent_stats"
echo

echo "-- Test 5b: indeterminate (subagent_stats present, .killed missing)"
t5b="$tmpdir/t5b.jsonl"
: >"$t5b"
r5b="$(result_json "eeeeeeee-0000-0000-0000-00000000005b" 10 end_turn completed success 1 1 0 1 1 0 0 no_killed)"
emit_cmdout "$t5b" 5006 "$r5b" 1700000003500000
emit_cmdend "$t5b" 5006 "/home/jason/.local/bin/kyomi-backlog-cron.sh" 1700000003500000
run_audit "$t5b"
assert_exit "missing .killed must exit 3, never treat as 0" 3
assert_contains "explains why" "killed.system missing"
echo

# ─── Test 6: mid-token chunk split, reconstructed correctly ────────────────
# Reproduces the real journal's behaviour: a long JSON value gets cut across
# consecutive CMDOUT entries, sometimes mid-token, with cron adding no
# separator of its own. This is THE non-obvious part of the parser.
echo "-- Test 6: JSON split mid-token across CMDOUT chunks"
t6="$tmpdir/t6.jsonl"
: >"$t6"
r6="$(result_json "ffffffff-0000-0000-0000-000000000006" 12 end_turn completed success 1 0 1 0 1 0 0 full)"
# Split at an arbitrary byte offset, deliberately inside a token (not on a
# delimiter), to prove the parser doesn't assume chunk boundaries are safe.
split_at=57
part1="${r6:0:$split_at}"
part2="${r6:$split_at}"
emit_cmdout "$t6" 6006 "$part1" 1700000004000000
emit_cmdout "$t6" 6006 "$part2" 1700000004000000
emit_cmdend "$t6" 6006 "/home/jason/.local/bin/kyomi-backlog-cron.sh" 1700000004000000
run_audit "$t6"
assert_exit "a JSON object split mid-token across chunks still parses cleanly" 0
assert_contains "verdict says clean" "RESULT: CLEAN"
assert_contains "found the run" "Found 1 kyomi cron run(s)"
echo

# ─── Test 7: multiple "type":"result" objects — the LAST one is authoritative
echo "-- Test 7: interim result shows clean, final result shows killed"
t7="$tmpdir/t7.jsonl"
: >"$t7"
tn7="$(task_notification_json "77777777-0000-0000-0000-000000000007")"
interim7="$(result_json "77777777-0000-0000-0000-000000000007" 30 end_turn completed success 2 1 1 1 1 0 0 full)"
final7="$(result_json "77777777-0000-0000-0000-000000000007" 5 end_turn completed success 2 1 1 2 1 0 1 full)"
emit_cmdout "$t7" 7007 "$tn7" 1700000005000000
emit_cmdout "$t7" 7007 "$interim7" 1700000005000000
emit_cmdout "$t7" 7007 "$final7" 1700000005000000
emit_cmdend "$t7" 7007 "/home/jason/.local/bin/kyomi-backlog-cron.sh" 1700000005000000
run_audit "$t7"
assert_exit "the FINAL result object wins, not an earlier interim one" 1
assert_contains "uses the final object's turn count context via killed.system" "killed.system=1"
echo

# ─── Test 8: mixed window — one clean, one killed ───────────────────────────
echo "-- Test 8: mixed window, multiple runs"
t8="$tmpdir/t8.jsonl"
: >"$t8"
r8a="$(result_json "88888888-0000-0000-0000-00000000008a" 20 end_turn completed success 1 0 1 0 1 0 0 full)"
r8b="$(result_json "88888888-0000-0000-0000-00000000008b" 8 end_turn completed success 1 1 0 1 0 0 1 full)"
emit_cmdout "$t8" 8001 "$r8a" 1700000006000000
emit_cmdend "$t8" 8001 "/home/jason/.local/bin/kyomi-backlog-cron.sh" 1700000006000000
emit_cmdout "$t8" 8002 "$r8b" 1700000007000000
emit_cmdend "$t8" 8002 "/home/jason/.local/bin/kyomi-backlog-cron.sh" 1700000007000000
run_audit "$t8"
assert_exit "one killed run among several is enough to fail the window" 1
assert_contains "found both runs" "Found 2 kyomi cron run(s)"
assert_contains "verdict names only the killed session" "session=88888888-0000-0000-0000-00000000008b"
assert_not_contains "the clean session is not in the killed list" "session=88888888-0000-0000-0000-00000000008a  killed"
echo

# ─── Test 9: non-matching script name is not counted ────────────────────────
echo "-- Test 9: CMDEND for a script outside the kyomi-*-cron.sh pattern"
t9="$tmpdir/t9.jsonl"
: >"$t9"
r9="$(result_json "99999999-0000-0000-0000-000000000009" 5 end_turn completed success 1 1 0 1 0 0 1 full)"
emit_cmdout "$t9" 9009 "$r9" 1700000008000000
emit_cmdend "$t9" 9009 "/etc/cron.daily/timeshift-something.sh" 1700000008000000
run_audit "$t9"
assert_exit "a non-kyomi cron script yields zero relevant records, so exit 3" 3
assert_contains "names the reason" "NO RUNS PARSED"
echo

# ─── Test 10: usage errors ───────────────────────────────────────────────────
echo "-- Test 10: usage"
run_audit_raw() {
    local out
    if out="$("$SCRIPT" "$@" 2>&1)"; then
        CHECK_STATUS=0
    else
        CHECK_STATUS=$?
    fi
    CHECK_OUTPUT="$out"
}
run_audit_raw --bogus-flag
assert_exit "unknown flag" 2
run_audit_raw --from-file
assert_exit "--from-file with no value" 2
run_audit_raw --from-file "$tmpdir/does-not-exist.jsonl"
assert_exit "--from-file pointing at a missing path" 2
run_audit_raw --from-file "$t1" "3 days ago" "5 days ago"
assert_exit "WINDOW given twice" 2
echo

# ─── Test 11: input is still self-consistent when --from-file and a WINDOW
# are both given (WINDOW is simply unused) ───────────────────────────────────
echo "-- Test 11: --from-file with a WINDOW present is not an error"
run_audit "$t1" "3 days ago"
assert_exit "WINDOW is accepted (and ignored) alongside --from-file" 0
echo

# ─── Test 12: journalctl itself failing — real binary, no fixture ──────────
# Uses the REAL journalctl (not a stub) with a --since value it is
# guaranteed to reject, proving the journalctl-unavailable/failing path maps
# to exit 3. Skips itself if journalctl is not on this machine's PATH.
echo "-- Test 12: journalctl failure path (real journalctl, no --from-file)"
if command -v journalctl >/dev/null 2>&1; then
    if out="$("$SCRIPT" "not-a-real-date-expression-at-all" 2>&1)"; then
        CHECK_STATUS=0
    else
        CHECK_STATUS=$?
    fi
    CHECK_OUTPUT="$out"
    assert_exit "an invalid --since expression makes journalctl fail -> exit 3" 3
    assert_contains "names journalctl as the failing step" "journalctl failed"
else
    echo "  (skipped — journalctl not on PATH on this machine)"
fi
echo

# ─── Test 13: `--` is a real end-of-options marker ───────────────────────────
# Before KYO-546's review nits were addressed, the `--` arm merely dropped the
# token and re-entered the same dispatch, so a WINDOW beginning with a dash was
# still rejected as an unknown argument. `--` must hand every remaining token to
# the positional branch, and must keep enforcing the give-WINDOW-once rule.
echo "-- Test 13: -- end-of-options"
run_audit "$t1" -- "3 days ago"
assert_exit "-- followed by a WINDOW is accepted" 0
run_audit_raw -- "-3 days ago"
CHECK_STATUS_AFTER_DASHDASH="$CHECK_STATUS"
if [ "$CHECK_STATUS_AFTER_DASHDASH" -eq 2 ]; then
    echo "  ✗ a dash-leading WINDOW after -- was still treated as a flag (exit 2)"
    FAIL=$((FAIL + 1))
else
    echo "  ✓ a dash-leading WINDOW after -- is not treated as a flag (exit $CHECK_STATUS_AFTER_DASHDASH)"
    PASS=$((PASS + 1))
fi
run_audit_raw --from-file "$t1" -- "3 days ago" "5 days ago"
assert_exit "-- still enforces WINDOW-given-once" 2
echo

# ─── Test 14: the SCRIPT column widens to the longest script name ────────────
# A hardcoded width silently misaligned every row once a cron wrapper longer
# than the constant existed (kyomi-merge-sweeper-cron.sh, 28 chars vs 26).
echo "-- Test 14: SCRIPT column is sized to the data"
t14="$tmpdir/t14.jsonl"
: >"$t14"
r14a="$(result_json "cccccccc-0000-0000-0000-00000000000a" 12 end_turn completed success 0 0 0 0 0 0 0 full)"
emit_cmdout "$t14" 14001 "$r14a" 1700000010000000
emit_cmdend "$t14" 14001 "/home/jason/.local/bin/kyomi-merge-sweeper-cron.sh" 1700000010000000
run_audit "$t14"
assert_exit "long-script-name fixture parses" 0
assert_contains "long script name is present in full" "kyomi-merge-sweeper-cron.sh"
# The SESSION column must start at the same offset in the header and the data
# row; that is exactly what a too-narrow SCRIPT column breaks.
header_off="$(printf '%s\n' "$CHECK_OUTPUT" | awk '/TIMESTAMP \(UTC\)/ { print index($0, "SESSION"); exit }')"
data_off="$(printf '%s\n' "$CHECK_OUTPUT" | awk '/kyomi-merge-sweeper-cron\.sh/ { print index($0, "cccccccc"); exit }')"
if [ -n "$header_off" ] && [ -n "$data_off" ] && [ "$header_off" -eq "$data_off" ]; then
    echo "  ✓ SESSION column aligns between header and data row (offset $header_off)"
    PASS=$((PASS + 1))
else
    echo "  ✗ SESSION column misaligned (header offset '$header_off', data offset '$data_off')"
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
