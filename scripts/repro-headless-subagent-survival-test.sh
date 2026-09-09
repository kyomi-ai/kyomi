#!/usr/bin/env bash
# ------------------------------------------------------------------------------
# scripts/repro-headless-subagent-survival-test.sh — self-test for
# repro-headless-subagent-survival.sh (KYO-711)
#
# Follows the shape of audit-agent-run-deaths-test.sh: a fresh mktemp -d,
# synthetic stream-json fixtures instead of a live `claude -p` run, and a
# PASS/FAIL harness with assert_exit / assert_contains. The script under
# test reads fixtures via its own `--from-file <path>` option (added for
# this ticket, mirroring audit-agent-run-deaths.sh's own --from-file), so
# no `claude` binary, subscription credential, network access, or quota is
# needed for any test below — CI must never invoke the real CLI.
#
# Exit 0 = all pass, exit 1 = any failure.
# ------------------------------------------------------------------------------

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
SCRIPT="$SCRIPT_DIR/repro-headless-subagent-survival.sh"
PASS=0
FAIL=0

tmpdir="$(mktemp -d)"
trap 'rm -rf "$tmpdir"' EXIT

# ─── invoke the script under test ───────────────────────────────────────────
CHECK_STATUS=""
CHECK_OUTPUT=""
run_repro() {
    # run_repro <mode> <fixture-file> [extra args...]
    local mode="$1" fixture="$2" out
    shift 2
    if out="$("$SCRIPT" "$mode" --from-file "$fixture" "$@" 2>&1)"; then
        CHECK_STATUS=0
    else
        CHECK_STATUS=$?
    fi
    CHECK_OUTPUT="$out"
}

run_repro_raw() {
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

echo "Running repro-headless-subagent-survival self-tests..."
echo

# ─── Test group 1: usage handling — must exit 2, not merely non-zero ───────
echo "-- Test 1: usage errors"

run_repro_raw
assert_exit "no arguments" 2

run_repro_raw bogus-mode
assert_exit "an unrecognised mode" 2

run_repro_raw schema --model
assert_exit "--model with no value after it" 2

run_repro_raw schema --from-file
assert_exit "--from-file with no value after it" 2

run_repro_raw schema --from-file "$tmpdir/does-not-exist.jsonl"
assert_exit "--from-file pointing at a nonexistent path" 2
echo

unreadable="$tmpdir/unreadable.jsonl"
echo '{"type":"result"}' >"$unreadable"
chmod 000 "$unreadable"
if [ "$(id -u)" -eq 0 ]; then
    echo "-- Test 1b: --from-file pointing at an unreadable path (skipped — running as root, permissions are not enforced)"
else
    run_repro_raw schema --from-file "$unreadable"
    assert_exit "--from-file pointing at an unreadable path" 2
fi
chmod 644 "$unreadable"
echo

# ─── Test group 2: parsing path, driven from fixtures — each must exit 3 ───
# with a message naming what specifically failed.
echo "-- Test 2: parsing failures (fixture-driven, fail-closed)"

# 2.1: output that is not JSON at all.
t21="$tmpdir/t21-not-json.log"
printf 'this is not json, just plain text output\n' >"$t21"
run_repro schema "$t21"
assert_exit "output that is not JSON at all exits 3" 3
assert_contains "names what failed" "no JSON objects found"

# 2.2: valid JSON lines, but no "type":"result" object anywhere.
t22="$tmpdir/t22-no-result.jsonl"
cat >"$t22" <<'EOF'
{"type":"system","subtype":"init"}
{"type":"assistant","message":{"role":"assistant","content":"hi"}}
EOF
run_repro schema "$t22"
assert_exit "valid JSON with no \"type\":\"result\" object exits 3" 3
assert_contains "names what failed" 'no "type":"result" object found'

# 2.3: a result object with no subagent_stats key.
t23="$tmpdir/t23-no-stats.jsonl"
cat >"$t23" <<'EOF'
{"type":"result","session_id":"s23","stop_reason":"end_turn","terminal_reason":"completed","subtype":"success","duration_ms":500,"result":"done"}
EOF
run_repro schema "$t23"
assert_exit "a result object with no subagent_stats key exits 3" 3
assert_contains "names what failed" "has no subagent_stats"

# 2.4: subagent_stats present but missing started_in_background.
t24="$tmpdir/t24-no-started-in-background.jsonl"
cat >"$t24" <<'EOF'
{"type":"result","session_id":"s24","stop_reason":"end_turn","terminal_reason":"completed","subtype":"success","duration_ms":500,"result":"done","subagent_stats":{"completed":1,"failed":0,"killed":{"parent":0,"user":0,"system":0}}}
EOF
run_repro background "$t24"
assert_exit "subagent_stats missing started_in_background exits 3" 3
assert_contains "names what failed" "no started_in_background key"
echo

# ─── Prove the exit-3 assertions are real, not vacuous ─────────────────────
# Directly invoke the script (no test-harness indirection) against two of
# the fixtures above and print the raw observed $? — this is the evidence
# that assert_exit above is checking a real, specific exit code (3) and not
# merely "non-zero", which would also pass on a usage-error 2.
echo "-- Test 2 evidence: raw exit codes observed directly (not via the harness)"
evidence_21="$tmpdir/evidence-2.1.log"
raw_exit_21=0
"$SCRIPT" schema --from-file "$t21" >"$evidence_21" 2>&1 || raw_exit_21=$?
echo "  fixture 2.1 (not JSON at all): observed \$? = $raw_exit_21"
evidence_24="$tmpdir/evidence-2.4.log"
raw_exit_24=0
"$SCRIPT" background --from-file "$t24" >"$evidence_24" 2>&1 || raw_exit_24=$?
echo "  fixture 2.4 (missing started_in_background): observed \$? = $raw_exit_24"
if [ "$raw_exit_21" -eq 3 ] && [ "$raw_exit_24" -eq 3 ]; then
    echo "  ✓ both raw invocations independently observed exit 3"
    PASS=$((PASS + 1))
else
    echo "  ✗ raw invocations did not observe exit 3 (got $raw_exit_21 and $raw_exit_24) — the script is failing open"
    FAIL=$((FAIL + 1))
fi
echo

# ─── Test group 3: happy path — exit 0 ──────────────────────────────────────
# A well-formed stream containing TWO "type":"result" objects (Claude Code
# emits an interim one at each task-notification checkpoint as well as the
# true final one at exit — see the script's own header). The parser must
# report the LAST one. session_id differs between the two objects so this
# test genuinely distinguishes "took the first" from "took the last"
# instead of passing vacuously on an identical fixture.
echo "-- Test 3: happy path, two result objects — the LAST one wins"
t3="$tmpdir/t3-two-results.jsonl"
cat >"$t3" <<'EOF'
{"type":"system","subtype":"task_notification","task_id":"abc","status":"stopped","summary":"still running"}
{"type":"result","session_id":"interim-session-id","stop_reason":"end_turn","terminal_reason":"completed","subtype":"success","duration_ms":1000,"result":"interim","subagent_stats":{"requested":{"background":1,"foreground":0,"unset":0},"started_in_background":1,"completed":0,"failed":0,"killed":{"parent":0,"user":0,"system":0}}}
{"type":"result","session_id":"final-session-id","stop_reason":"end_turn","terminal_reason":"completed","subtype":"success","duration_ms":166000,"result":"final","subagent_stats":{"requested":{"background":1,"foreground":0,"unset":0},"started_in_background":1,"completed":1,"failed":0,"killed":{"parent":0,"user":0,"system":0}}}
EOF
run_repro background "$t3"
assert_exit "a stream with two result objects exits 0" 0
assert_contains "reports seeing both result objects" "result_objects_seen: 2"
assert_contains "the LAST session_id is reported" "session_id: final-session-id"
if printf '%s' "$CHECK_OUTPUT" | grep -qF "session_id: interim-session-id"; then
    echo "  ✗ the FIRST (interim) session_id leaked into the output — parser took the wrong object"
    FAIL=$((FAIL + 1))
else
    echo "  ✓ the FIRST (interim) session_id does not appear anywhere in the output"
    PASS=$((PASS + 1))
fi
assert_contains "the LAST duration_ms is reported" "duration_ms: 166000"
echo

# ─── summary ──────────────────────────────────────────────────────────────
echo "============================================"
echo "Results: $PASS passed, $FAIL failed"
if [ "$FAIL" -gt 0 ]; then
    exit 1
fi
exit 0
