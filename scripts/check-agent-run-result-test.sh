#!/usr/bin/env bash
# ------------------------------------------------------------------------------
# scripts/check-agent-run-result-test.sh — self-test for
# check-agent-run-result.sh (KYO-710)
#
# Follows the shape of scripts/audit-agent-run-deaths-test.sh: a fresh
# mktemp -d, synthetic fixtures built via python3 (JSON is too easy to get
# subtly wrong hand-quoted in bash), a PASS/FAIL harness with assert_exit /
# assert_contains / assert_not_contains, and the script under test driven
# entirely through its own `--log`/`--state-dir` flags — no real
# `~/.local/state/kyomi/`, no real cron log, ever touched by this suite.
#
# Hermetic: every fixture lives under a single `mktemp -d` cleaned up by a
# `trap ... EXIT`. No network. The one test that needs `notify-send`
# provably absent from PATH (Test 9) builds a minimal stub PATH out of
# symlinks to the real required tools rather than touching the real PATH.
#
# Needs only bash + python3 (python3 for encoding JSON fixtures — the
# script under test is itself bash + python3 only, no `jq`, see its header).
#
# Exit 0 = all pass, exit 1 = any failure.
# ------------------------------------------------------------------------------

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
SCRIPT="$SCRIPT_DIR/check-agent-run-result.sh"
PASS=0
FAIL=0

tmpdir="$(mktemp -d)"
trap 'rm -rf "$tmpdir"' EXIT

# ─── fixture helpers ─────────────────────────────────────────────────────────
# Every helper below emits ONE complete JSON object (one line) via python3,
# never hand-quoted bash string interpolation — the payloads exercise nested
# quotes ("result" text containing the literal phrases this script matches
# on) that bash would get wrong.

# result_json <session_id> <num_turns> <stop_reason> <terminal_reason>
#             <subtype> <is_error: true|false> <result_text>
result_json() {
    python3 -c '
import json, sys
session_id, num_turns, stop_reason, terminal_reason, subtype, is_error, result_text = sys.argv[1:8]
print(json.dumps({
    "type": "result",
    "session_id": session_id,
    "num_turns": int(num_turns),
    "stop_reason": stop_reason,
    "terminal_reason": terminal_reason,
    "subtype": subtype,
    "is_error": is_error == "true",
    "result": result_text,
    "duration_ms": 100,
    "uuid": "00000000-0000-0000-0000-000000000000",
}))
' "$@"
}

# assistant_auth_marker_json <session_id> — the STRUCTURED signal: an
# assistant-typed object with is_api_error_message=true and
# error="authentication_failed", exactly as verified against three real
# 2026-09 incident logs (see check-agent-run-result.sh's own header).
assistant_auth_marker_json() {
    python3 -c '
import json, sys
session_id = sys.argv[1]
print(json.dumps({
    "type": "assistant",
    "session_id": session_id,
    "is_api_error_message": True,
    "error": "authentication_failed",
    "message": {"role": "assistant", "content": [
        {"type": "text", "text": "Failed to authenticate: OAuth session expired and could not be refreshed"}
    ]},
}))
' "$1"
}

# assistant_prose_json <session_id> <text> — an ORDINARY assistant message,
# no structured auth marker at all. Used to prove that prose merely
# containing the auth phrases, outside the final result object's own
# "result" field, does not trip detection (the false-positive this script's
# header specifically claims to avoid).
assistant_prose_json() {
    python3 -c '
import json, sys
session_id, text = sys.argv[1], sys.argv[2]
print(json.dumps({
    "type": "assistant",
    "session_id": session_id,
    "message": {"role": "assistant", "content": [{"type": "text", "text": text}]},
}))
' "$1" "$2"
}

system_init_json() {
    python3 -c 'import json; print(json.dumps({"type": "system", "subtype": "init"}))'
}

# write_log <outfile> <json-line>... — newline-delimited, matching the real
# stream-json shape (though the script under test parses the raw byte
# stream regardless of where the newlines fall).
write_log() {
    local outfile="$1"
    shift
    : >"$outfile"
    for line in "$@"; do
        printf '%s\n' "$line" >>"$outfile"
    done
}

# make_stub_bin <dir> — populate <dir> with symlinks to the real absolute
# path of every external tool check-agent-run-result.sh (and the
# stale-tooling-guard it sources) actually invokes, EXCEPT notify-send.
# Used only by Test 9, so that PATH can be pointed at ONLY this directory —
# genuinely making notify-send absent, not just unlikely to be found —
# while every tool the script under test legitimately needs still resolves.
make_stub_bin() {
    local dir="$1" tool real
    mkdir -p "$dir"
    for tool in python3 git basename dirname mkdir cat mktemp mv date rm grep cut; do
        if ! real="$(command -v "$tool" 2>/dev/null)"; then
            echo "FATAL: required tool '$tool' not found on the real PATH — cannot build stub" >&2
            exit 1
        fi
        ln -sf "$real" "$dir/$tool"
    done
}

# ─── invoke the script under test ───────────────────────────────────────────
CHECK_STATUS=""
CHECK_OUTPUT=""
run_check() {
    # run_check <arg>...
    local out
    if out="$("$SCRIPT" "$@" 2>&1)"; then
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

assert_file_contains() {
    local name="$1" path="$2" needle="$3"
    if [ -f "$path" ] && grep -qF -- "$needle" "$path" 2>/dev/null; then
        printf "  \xe2\x9c\x93 %s\n" "$name"
        PASS=$((PASS + 1))
    else
        printf "  \xe2\x9c\x97 %s \xe2\x80\x94 expected '%s' to contain: %s\n" "$name" "$path" "$needle"
        FAIL=$((FAIL + 1))
    fi
}

assert_file_equals() {
    local name="$1" path="$2" expected="$3" actual
    actual="$(cat "$path" 2>/dev/null || echo "<missing>")"
    if [ "$actual" = "$expected" ]; then
        printf "  \xe2\x9c\x93 %s\n" "$name"
        PASS=$((PASS + 1))
    else
        printf "  \xe2\x9c\x97 %s \xe2\x80\x94 expected '%s' to contain '%s', got '%s'\n" "$name" "$path" "$expected" "$actual"
        FAIL=$((FAIL + 1))
    fi
}

assert_missing() {
    local name="$1" path="$2"
    if [ ! -e "$path" ]; then
        printf "  \xe2\x9c\x93 %s\n" "$name"
        PASS=$((PASS + 1))
    else
        printf "  \xe2\x9c\x97 %s \xe2\x80\x94 expected '%s' to not exist\n" "$name" "$path"
        FAIL=$((FAIL + 1))
    fi
}

echo "Running check-agent-run-result self-tests..."
echo

# ─── Test 1: the real 2026-09 outage shape — is_error true, subtype
# "success", terminal_reason "api_error", OAuth phrase, plus the structured
# assistant marker. Verified against three surviving real logs (one per
# worker); see check-agent-run-result.sh's own header. ─────────────────────
echo "-- Test 1: the real outage shape is detected and classified as an auth failure"
t1="$tmpdir/t1.log"
sd1="$tmpdir/state1"
write_log "$t1" \
    "$(system_init_json)" \
    "$(assistant_auth_marker_json aaaaaaaa-0000-0000-0000-000000000001)" \
    "$(result_json aaaaaaaa-0000-0000-0000-000000000001 1 stop_sequence api_error success true \
        "Failed to authenticate: OAuth session expired and could not be refreshed")"
run_check --log "$t1" --exit-code 0 --worker backlog --state-dir "$sd1"
assert_exit "the outage shape fails the run" 1
assert_contains "reasons include IS_ERROR_TRUE" "IS_ERROR_TRUE"
assert_contains "reasons include TERMINAL_REASON_API_ERROR" "TERMINAL_REASON_API_ERROR"
assert_contains "reasons include AUTH_FAILURE" "AUTH_FAILURE"
assert_contains "explicitly classifies as an auth failure needing /login" "needs an interactive"
echo

# ─── Test 2: a genuinely successful run must not be flagged ────────────────
echo "-- Test 2: a clean run is not flagged"
t2="$tmpdir/t2.log"
sd2="$tmpdir/state2"
write_log "$t2" \
    "$(system_init_json)" \
    "$(result_json bbbbbbbb-0000-0000-0000-000000000002 40 end_turn completed success false "Handed off. PR #500 opened.")"
run_check --log "$t2" --exit-code 0 --worker backlog --state-dir "$sd2"
assert_exit "a clean run exits 0" 0
assert_contains "reasons is none" "REASONS: (none)"
assert_contains "result says SUCCESS" "RESULT: SUCCESS"
assert_file_equals "counter reset to 0" "$sd2/backlog.count" "0"
echo

# ─── Test 3: nonzero --exit-code (the PIPESTATUS[0] half) fails the run
# even against an otherwise-clean log ───────────────────────────────────────
echo "-- Test 3: nonzero --exit-code fails the run even with a clean log"
t3="$tmpdir/t3.log"
sd3="$tmpdir/state3"
write_log "$t3" \
    "$(result_json cccccccc-0000-0000-0000-000000000003 10 end_turn completed success false "all good")"
run_check --log "$t3" --exit-code 137 --worker backlog --state-dir "$sd3"
assert_exit "a nonzero real exit code fails the run" 1
assert_contains "reasons include EXIT_CODE_NONZERO" "EXIT_CODE_NONZERO(137)"
echo

# ─── Test 4: things that must NEVER read as success ─────────────────────────
echo "-- Test 4a: no result object at all"
t4a="$tmpdir/t4a.log"
sd4a="$tmpdir/state4a"
write_log "$t4a" "$(system_init_json)" "$(assistant_prose_json dddddddd-0000-0000-0000-000000000004 "still working")"
run_check --log "$t4a" --exit-code 0 --worker backlog --state-dir "$sd4a"
assert_exit "no result object fails the run" 1
assert_contains "names the reason" "NO_RESULT_OBJECT"
echo

echo "-- Test 4b: an empty log file"
t4b="$tmpdir/t4b.log"
sd4b="$tmpdir/state4b"
: >"$t4b"
run_check --log "$t4b" --exit-code 0 --worker backlog --state-dir "$sd4b"
assert_exit "an empty log fails the run" 1
assert_contains "names the reason" "NO_RESULT_OBJECT"
echo

echo "-- Test 4c: a missing log file"
sd4c="$tmpdir/state4c"
run_check --log "$tmpdir/does-not-exist.log" --exit-code 0 --worker backlog --state-dir "$sd4c"
assert_exit "a missing log file fails the run" 1
assert_contains "names it unreadable" "could not read"
echo

echo "-- Test 4d: a truncated/corrupt JSON tail"
t4d="$tmpdir/t4d.log"
sd4d="$tmpdir/state4d"
good_obj="$(system_init_json)"
# Deliberately cut a well-formed result object off mid-write, simulating a
# process killed while tee was still writing its final line.
fragment='{"type":"result","session_id":"eeeeeeee-0000-0000-0000-000000000005","is_err'
write_log "$t4d" "$good_obj"
printf '%s' "$fragment" >>"$t4d"
run_check --log "$t4d" --exit-code 0 --worker backlog --state-dir "$sd4d"
assert_exit "a truncated tail fails the run" 1
assert_contains "names the reason" "LOG_TAIL_CORRUPT"
echo

echo "-- Test 4e: a literal '{' inside the FINAL result's own text is NOT corruption"
# Regression fixture: earlier drafts of check-agent-run-result.sh found the
# LAST '{' anywhere in the whole log and asked whether it started a
# complete JSON value. A real agent transcript's own "result" text quoting
# code or JSON (e.g. an object literal) fails that test even though the log
# is perfectly well-formed and complete. Confirmed on a real log from this
# box: /tmp/kyomi-backlog-20260911-081700.log's final result field quotes
# `{run_in_background:!0}` verbatim near the very end of the file.
t4e="$tmpdir/t4e.log"
sd4e="$tmpdir/state4e"
write_log "$t4e" \
    "$(result_json ffffffff-0000-0000-0000-000000000006 12 end_turn completed success false \
        'Fixed the gate: `e.omit({run_in_background:!0})` is now applied on every call site.')"
run_check --log "$t4e" --exit-code 0 --worker backlog --state-dir "$sd4e"
assert_exit "a brace inside the result's own text does not trip tail-corruption" 0
assert_not_contains "no LOG_TAIL_CORRUPT reason" "LOG_TAIL_CORRUPT"
echo

# ─── Test 5: subtype:"success" must NEVER rescue an is_error:true run ──────
echo "-- Test 5: subtype success never overrides is_error true"
t5="$tmpdir/t5.log"
sd5="$tmpdir/state5"
write_log "$t5" \
    "$(result_json 11111111-0000-0000-0000-000000000007 5 stop_sequence completed success true \
        "Some other, non-auth error occurred mid-run.")"
run_check --log "$t5" --exit-code 0 --worker backlog --state-dir "$sd5"
assert_exit "is_error true fails the run regardless of subtype" 1
assert_contains "reasons include IS_ERROR_TRUE" "IS_ERROR_TRUE"
assert_not_contains "not misclassified as an auth failure" "MACHINE_AUTH_FAILURE=1"
assert_not_contains "terminal_reason completed is not itself a failure signal" "TERMINAL_REASON_COMPLETED"
echo

# ─── Test 6: counter increments across consecutive failures; a success
# resets it to 0 and clears the escalation marker ───────────────────────────
echo "-- Test 6: consecutive-failure counter and reset"
t6fail="$tmpdir/t6fail.log"
t6ok="$tmpdir/t6ok.log"
sd6="$tmpdir/state6"
write_log "$t6fail" "$(result_json 22222222-0000-0000-0000-000000000008 1 stop_sequence api_error success true "boom")"
write_log "$t6ok" "$(result_json 33333333-0000-0000-0000-000000000009 20 end_turn completed success false "fine")"

run_check --log "$t6fail" --exit-code 0 --worker sixer --state-dir "$sd6" --threshold 5
assert_exit "failure #1" 1
assert_file_equals "counter is 1 after one failure" "$sd6/sixer.count" "1"

run_check --log "$t6fail" --exit-code 0 --worker sixer --state-dir "$sd6" --threshold 5
assert_exit "failure #2" 1
assert_file_equals "counter is 2 after two failures" "$sd6/sixer.count" "2"

run_check --log "$t6fail" --exit-code 0 --worker sixer --state-dir "$sd6" --threshold 5
assert_exit "failure #3" 1
assert_file_equals "counter is 3 after three failures" "$sd6/sixer.count" "3"

run_check --log "$t6ok" --exit-code 0 --worker sixer --state-dir "$sd6" --threshold 5
assert_exit "the next success resets the streak" 0
assert_file_equals "counter reset to 0 by a success" "$sd6/sixer.count" "0"
assert_missing "escalation marker cleared by a success" "$sd6/sixer.escalated"
echo

# ─── Test 7: no escalation below threshold; escalation AT threshold and on
# EVERY subsequent failing run, not just the first crossing ────────────────
echo "-- Test 7: escalation only at/above threshold, and on every run at or above it"
t7fail="$tmpdir/t7fail.log"
sd7="$tmpdir/state7"
write_log "$t7fail" "$(result_json 44444444-0000-0000-0000-00000000000a 1 stop_sequence api_error success true "boom")"

run_check --log "$t7fail" --exit-code 0 --worker sevener --state-dir "$sd7" --threshold 3
assert_exit "failure #1 (below threshold)" 1
assert_not_contains "no escalation below threshold" "ESCALATION:"
assert_missing "no escalation marker below threshold" "$sd7/sevener.escalated"

run_check --log "$t7fail" --exit-code 0 --worker sevener --state-dir "$sd7" --threshold 3
assert_exit "failure #2 (still below threshold)" 1
assert_not_contains "still no escalation" "ESCALATION:"
assert_missing "still no escalation marker" "$sd7/sevener.escalated"

run_check --log "$t7fail" --exit-code 0 --worker sevener --state-dir "$sd7" --threshold 3
assert_exit "failure #3 (AT threshold)" 1
assert_contains "escalates at threshold" "ESCALATION:"
assert_file_contains "escalation marker written at threshold" "$sd7/sevener.escalated" "ESCALATION:"

run_check --log "$t7fail" --exit-code 0 --worker sevener --state-dir "$sd7" --threshold 3
assert_exit "failure #4 (past threshold)" 1
assert_contains "STILL escalates past threshold, not just once" "ESCALATION:"
echo

# ─── Test 8: the failures log gets a line even for a below-threshold
# failure ────────────────────────────────────────────────────────────────────
echo "-- Test 8: failures.log records every failure, even below threshold"
t8fail="$tmpdir/t8fail.log"
sd8="$tmpdir/state8"
write_log "$t8fail" "$(result_json 55555555-0000-0000-0000-00000000000b 1 stop_sequence api_error success true "boom")"
run_check --log "$t8fail" --exit-code 0 --worker eighter --state-dir "$sd8" --threshold 10
assert_exit "a single failure, far below threshold" 1
assert_file_contains "failures.log has a line for it" "$sd8/failures.log" "worker=eighter"
assert_file_contains "failures.log names the log path" "$sd8/failures.log" "$t8fail"
echo

# ─── Test 9: notify-send genuinely absent from PATH must not fail the
# script ─────────────────────────────────────────────────────────────────────
echo "-- Test 9: notify-send absent from PATH does not fail the script"
stubbin="$tmpdir/stubbin"
make_stub_bin "$stubbin"
if PATH="$stubbin" command -v notify-send >/dev/null 2>&1; then
    echo "  ✗ setup failure: notify-send is still resolvable under the stub PATH"
    FAIL=$((FAIL + 1))
else
    echo "  ✓ setup: notify-send is genuinely absent under the stub PATH"
    PASS=$((PASS + 1))
fi
t9fail="$tmpdir/t9fail.log"
sd9="$tmpdir/state9"
write_log "$t9fail" "$(result_json 66666666-0000-0000-0000-00000000000c 1 stop_sequence api_error success true "boom")"
if out9="$(PATH="$stubbin" /usr/bin/bash "$SCRIPT" --log "$t9fail" --exit-code 0 --worker niner --state-dir "$sd9" --threshold 1 2>&1)"; then
    CHECK_STATUS=0
else
    CHECK_STATUS=$?
fi
CHECK_OUTPUT="$out9"
assert_exit "the run still correctly reports failure" 1
assert_contains "it still escalates on stderr" "ESCALATION:"
echo

# ─── Test 10: --help ─────────────────────────────────────────────────────────
echo "-- Test 10: --help"
run_check --help
assert_exit "--help exits 0" 0
assert_contains "usage mentions --log" "--log"
assert_contains "usage mentions --worker" "--worker"
echo

# ─── Test 11: missing required flags ────────────────────────────────────────
echo "-- Test 11: required-flag validation"
run_check --log "$t2"
assert_exit "missing --exit-code and --worker" 2

run_check --log "$t2" --exit-code 0
assert_exit "missing --worker" 2

run_check --exit-code 0 --worker backlog
assert_exit "missing --log" 2
echo

# ─── Test 12: unknown flag ───────────────────────────────────────────────────
echo "-- Test 12: unknown flag"
run_check --bogus-flag
assert_exit "unknown flag" 2
echo

# ─── Test 13: value validation ───────────────────────────────────────────────
echo "-- Test 13: --exit-code / --threshold / --worker validation"
run_check --log "$t2" --exit-code notanumber --worker backlog
assert_exit "--exit-code must be an integer" 2

run_check --log "$t2" --exit-code 0 --worker backlog --threshold 0
assert_exit "--threshold must be positive" 2

run_check --log "$t2" --exit-code 0 --worker backlog --threshold notanumber
assert_exit "--threshold must be an integer" 2

run_check --log "$t2" --exit-code 0 --worker "has a space"
assert_exit "--worker must match the filename-safe pattern" 2

run_check --log "$t2" --exit-code 0 --worker "../etc"
assert_exit "--worker rejects path traversal characters" 2
echo

# ─── Test 14: auth failure via the STRUCTURED marker alone (no phrase in
# the result text) is still detected ─────────────────────────────────────────
echo "-- Test 14: structured marker alone is sufficient"
t14="$tmpdir/t14.log"
sd14="$tmpdir/state14"
write_log "$t14" \
    "$(assistant_auth_marker_json 77777777-0000-0000-0000-00000000000d)" \
    "$(result_json 77777777-0000-0000-0000-00000000000d 1 stop_sequence api_error success true "generic failure text")"
run_check --log "$t14" --exit-code 0 --worker backlog --state-dir "$sd14"
assert_exit "structured marker alone fails the run" 1
assert_contains "classified as an auth failure" "AUTH_FAILURE"
echo

# ─── Test 15: auth failure via the result-field PHRASE alone (no
# structured marker present anywhere) is still detected ────────────────────
echo "-- Test 15: result-field phrase alone is sufficient"
t15="$tmpdir/t15.log"
sd15="$tmpdir/state15"
write_log "$t15" \
    "$(result_json 88888888-0000-0000-0000-00000000000e 1 stop_sequence api_error success true \
        "OAuth session expired and could not be refreshed")"
run_check --log "$t15" --exit-code 0 --worker backlog --state-dir "$sd15"
assert_exit "phrase alone fails the run" 1
assert_contains "classified as an auth failure" "AUTH_FAILURE"
echo

# ─── Test 16: the auth phrases appearing OUTSIDE the final result's own
# field — ordinary prose earlier in the transcript — must NOT trigger
# detection. This is the false positive the whole-log-substring approach
# would produce (an agent discussing or grepping this very incident). ──────
echo "-- Test 16: the phrase in ordinary prose (not the final result field) is not a false positive"
t16="$tmpdir/t16.log"
sd16="$tmpdir/state16"
write_log "$t16" \
    "$(assistant_prose_json 99999999-0000-0000-0000-00000000000f \
        "For context, KYO-710 exists because cron logs showed: Failed to authenticate: OAuth session expired and could not be refreshed. I am now implementing the detector for it.")" \
    "$(result_json 99999999-0000-0000-0000-00000000000f 30 end_turn completed success false "Implemented and shipped. PR #513.")"
run_check --log "$t16" --exit-code 0 --worker backlog --state-dir "$sd16"
assert_exit "a healthy run discussing the incident in prose is not flagged" 0
assert_not_contains "no auth failure reported" "MACHINE_AUTH_FAILURE=1"
echo

# ─── Test 17: an unwritable state directory fails closed ───────────────────
echo "-- Test 17: a state directory that cannot be created fails the run"
t17="$tmpdir/t17.log"
sd17="$tmpdir/state17-is-a-file"
write_log "$t17" "$(result_json aaaaaaaa-1111-0000-0000-000000000010 20 end_turn completed success false "fine")"
: >"$sd17"  # a plain file where a directory is expected — mkdir -p must fail
run_check --log "$t17" --exit-code 0 --worker backlog --state-dir "$sd17"
assert_exit "an unwritable state directory fails the run even on an otherwise-clean log" 1
assert_contains "names the reason" "could not create state directory"
echo

# ─── summary ──────────────────────────────────────────────────────────────
echo "============================================"
echo "Results: $PASS passed, $FAIL failed"
if [ "$FAIL" -gt 0 ]; then
    exit 1
fi
exit 0
