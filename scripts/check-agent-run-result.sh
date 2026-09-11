#!/usr/bin/env bash
# ------------------------------------------------------------------------------
# scripts/check-agent-run-result.sh — decide whether ONE `claude -p` cron run
# actually succeeded, and escalate a sustained streak of failures. (KYO-710)
#
# WHY THIS EXISTS
#
# All three kyomi cron workers (kyomi-backlog-cron.sh, kyomi-merge-sweeper-
# cron.sh, kyomi-needs-build-cron.sh) ran 27 consecutive times over roughly
# 70 hours (2026-09-05 through 2026-09-08) without doing anything, while
# every single run reported success. Verified directly against three
# surviving logs from that window, one per worker
# (/tmp/kyomi-backlog-20260908-001701.log,
# /tmp/kyomi-sweeper-20260906-184300.log,
# /tmp/kyomi-needs-build-20260908-033500.log — read, not assumed from the
# ticket), the final `"type":"result"` object in all three was byte-for-byte
# the same shape:
#
#   "is_error":true, "subtype":"success", "terminal_reason":"api_error",
#   "num_turns":1, "stop_reason":"stop_sequence",
#   "result":"Failed to authenticate: OAuth session expired and could not
#   be refreshed"
#
# and the "type":"assistant" object immediately before it carried
# "is_api_error_message":true and "error":"authentication_failed" — a
# structured marker, not just prose, present in all three logs.
#
# That was invisible to the wrappers as they existed before KYO-710 for two
# independent reasons:
#
#   1. `subtype` reads "success" even when `is_error` is `true`. Nothing was
#      reading `is_error` at all.
#   2. Every wrapper ends `claude -p ... | tee "$LOGFILE"`. A bare `$?`
#      after that pipeline is `tee`'s own exit status — `tee` essentially
#      always succeeds — never claude's. The wrappers now capture
#      `${PIPESTATUS[0]}` immediately after the pipeline specifically so
#      this script can be handed the real exit status (see the call sites
#      in ~/.local/bin/kyomi-*-cron.sh, outside this repo).
#
# Renewing the expired session is out of scope for this script and always
# will be: refreshing a subscription OAuth login genuinely requires an
# interactive `claude /login` on this machine, and nothing running headless
# under cron can do that safely. This script is the detection half only —
# turn a success-shaped failure into a loud one, and stop staying quiet once
# it is not a one-off.
#
# RELATIONSHIP TO audit-agent-run-deaths.sh — NOT A DUPLICATE
#
# scripts/audit-agent-run-deaths.sh (KYO-546) answers a retrospective,
# cross-run question by reconstructing `claude -p`'s stream-json output from
# journalctl's chunked CMDOUT/CMDEND records over a time window: "did any
# run in the last N days silently discard a background sub-agent's work
# while reporting success?" This script answers a single-run, real-time
# question about the run that just finished, by reading the wrapper's own
# `tee`d logfile directly off disk — there is no journal reconstruction to
# do, because the wrapper already has the raw byte stream in hand. Both
# scripts parse the same stream-json `"type":"result"` object because they
# are looking at the same underlying artifact, but they solve different
# problems for different callers (a periodic audit anyone can run, vs. the
# per-invocation gate baked into every cron wrapper) and were kept separate
# rather than merged into one interface that would have to serve both a
# journalctl window and a single file path.
#
# FAIL CLOSED
# (docs/standards/error-handling/empty-on-failure-must-not-look-like-a-real-result.md)
#
# A run whose outcome cannot be established — no `"type":"result"` object in
# the log at all, a log that cannot be opened, a truncated/corrupt tail, or
# a state directory/counter file this script cannot write — is NEVER
# reported as success. Unlike audit-agent-run-deaths.sh, which keeps
# "indeterminate" as its own exit code (3) distinct from "a death was
# found" (1), this script collapses "failed" and "could not be determined"
# into the same non-zero exit: a cron wrapper does exactly one of two things
# with this script's verdict (treat the run as fine, or make noise about
# it), and there is no third action it would take for "indeterminate" that
# differs from "failed" — so the wrapper-side contract stays a single
# `if ! check-agent-run-result.sh ...; then <make noise>; fi`.
#
# CLASSIFY, DON'T JUST DETECT
#
# A bare "this run failed" forces whoever reads the escalation to go open
# the log by hand, every time. Where the log names its own cause, this
# script says so: "needs an interactive `claude /login`" and "transient API
# error, retry next cycle" call for different operator responses, and
# folding both into one undifferentiated FAILURE would just relocate the
# manual triage instead of removing it.
#
# The auth-failure check is scoped to the FINAL result object's own designated
# fields — its `"result"` string, plus the structured
# `is_api_error_message`/`error` marker on the assistant message immediately
# preceding it — never a substring match against the raw log as a whole.
# A cron run's own transcript routinely contains these exact words in
# entirely healthy contexts: an agent that reads or greps this very
# script's header (the paragraphs above quote both phrases verbatim)
# produces a tool_result containing them, and an assistant can simply be
# discussing this incident in prose while doing unrelated, successful work.
# Grepping the whole blob would flag both as auth failures. Reading only
# the field the real failure actually appears in does not.
#
# CONSECUTIVE-FAILURE STATE
#
# Lives under a persistent directory
# (${XDG_STATE_HOME:-$HOME/.local/state}/kyomi by default; --state-dir
# overrides it for the self-test) — deliberately never /tmp, which is wiped
# on reboot and would zero the counter exactly when a long outage spanning a
# reboot most needs it to stay nonzero. One counter file per worker. A
# success resets that worker's counter to 0 and clears its escalation
# marker. A failure increments the counter, appends one line to a shared
# failures log (so there is a durable record even below the escalation
# threshold), and — once the counter reaches --threshold
# (default 3), and on EVERY failing run at or above it, not just the run
# that first crosses it, so a sustained outage stays loud instead of
# escalating once and going quiet — escalates via three independent
# channels: a marker file, a message on stderr (cron mails a job's stderr to
# the box owner), and `notify-send` only if it is actually present on PATH
# (checked, never assumed; a runtime notify-send failure, e.g. no D-Bus
# session under cron, is swallowed the same way).
#
# That failures-log record is durable but NOT unconditional, and the
# distinction matters to anyone who later reads the log as evidence. Two
# paths skip the append. If the state directory cannot be written at all,
# the STATE_WRITE_OK branch near the bottom of this script fails the run
# closed without touching the counter, the failures log or the escalation
# channels — all three live inside the directory just established as
# unwritable, so attempting them would only produce a second, noisier
# failure. And if the append itself fails on an otherwise writable
# directory, it degrades to a warning on stderr rather than failing the run,
# because the run's real verdict has already been decided by then and
# discarding it to report a bookkeeping problem would be the worse trade.
# So failures.log can carry a GAP across a state-directory outage. Nothing
# is silently swallowed either way: the primary contract still fires,
# because those paths still exit non-zero and cron mails the run.
#
# WHAT THIS SCRIPT DELIBERATELY DOES NOT DO
#
# It does not retry the run, does not attempt to renew the session, and
# does not read or write ANTHROPIC_API_KEY. It also does not decide what the
# wrapper does with a non-zero exit — that decision (and its reasoning) lives
# at each wrapper's own call site outside this repo.
#
# Only Python's standard-library `json` module parses anything here — no
# `jq`. This matches scripts/audit-agent-run-deaths.sh's own stated
# convention: this repo already ships standalone stdlib-only `.py` helpers
# under scripts/ (e.g. scripts/e2e-regression/seed-test-user.py), so a
# stdlib-only Python dependency alongside bash is an established pattern
# here, not a new one, and introducing a `jq` dependency for a job an
# in-repo idiom already does would be new surface for no benefit.
#
# USAGE
#
#   check-agent-run-result.sh --log <path> --exit-code <n> --worker <name>
#                              [--state-dir <path>] [--threshold <n>]
#
#   --log PATH         the wrapper's own stream-json logfile — the file
#                       `tee` wrote, read directly. Not a journalctl record.
#   --exit-code N       the REAL claude exit status, i.e. `${PIPESTATUS[0]}`
#                       captured immediately after the `| tee` pipeline.
#                       Passing tee's own $? here defeats the entire point.
#   --worker NAME       identifies the cron worker for the per-worker state
#                       files (e.g. "backlog", "merge-sweeper",
#                       "needs-build"). Must match ^[A-Za-z0-9_.-]+$ — it
#                       becomes part of a filename under --state-dir.
#   --state-dir DIR     override the state directory (default:
#                       ${XDG_STATE_HOME:-$HOME/.local/state}/kyomi). Exists
#                       so the self-test never touches real state.
#   --threshold N       consecutive failures before escalating (default: 3).
#   -h, --help          print this usage and exit 0.
#
# EXIT CODES
#
#   0  the run succeeded: no failure signal found, and state was updated.
#   1  the run failed, or its outcome could not be established at all
#      (nonzero --exit-code, is_error true, terminal_reason of api_error,
#      an auth-failure signal, no result object found, a truncated/corrupt
#      log tail, or a state directory/counter that could not be written).
#   2  usage error (missing/unknown flag, a malformed --exit-code/
#      --threshold, or a --worker outside the allowed character set).
# ------------------------------------------------------------------------------

set -euo pipefail

SCRIPT_NAME="$(basename "$0")"
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=lib/stale-tooling-guard.sh
source "${SCRIPT_DIR}/lib/stale-tooling-guard.sh"
stale_tooling_guard "${BASH_SOURCE[0]}"

usage() {
    cat >&2 <<EOF
Usage: $SCRIPT_NAME --log <path> --exit-code <n> --worker <name> [--state-dir <path>] [--threshold <n>]

  --log PATH        path to the wrapper's own stream-json logfile
  --exit-code N      the real claude exit status (e.g. \${PIPESTATUS[0]}), never tee's
  --worker NAME       worker name, used in state filenames (must match ^[A-Za-z0-9_.-]+\$)
  --state-dir DIR     override the state directory (default: \${XDG_STATE_HOME:-\$HOME/.local/state}/kyomi)
  --threshold N       consecutive failures before escalating (default: 3)

Exit codes:
  0  the run succeeded
  1  the run failed, or its outcome could not be established (fail closed)
  2  usage error
EOF
}

LOG_PATH=""
EXIT_CODE=""
WORKER=""
STATE_DIR="${XDG_STATE_HOME:-$HOME/.local/state}/kyomi"
THRESHOLD=3

while [ "$#" -gt 0 ]; do
    case "$1" in
        --log)
            [ "$#" -ge 2 ] || { echo "ERROR: --log requires a value" >&2; exit 2; }
            LOG_PATH="$2"
            shift 2
            ;;
        --exit-code)
            [ "$#" -ge 2 ] || { echo "ERROR: --exit-code requires a value" >&2; exit 2; }
            EXIT_CODE="$2"
            shift 2
            ;;
        --worker)
            [ "$#" -ge 2 ] || { echo "ERROR: --worker requires a value" >&2; exit 2; }
            WORKER="$2"
            shift 2
            ;;
        --state-dir)
            [ "$#" -ge 2 ] || { echo "ERROR: --state-dir requires a value" >&2; exit 2; }
            STATE_DIR="$2"
            shift 2
            ;;
        --threshold)
            [ "$#" -ge 2 ] || { echo "ERROR: --threshold requires a value" >&2; exit 2; }
            THRESHOLD="$2"
            shift 2
            ;;
        -h|--help)
            usage
            exit 0
            ;;
        *)
            echo "ERROR: unknown argument: $1" >&2
            usage
            exit 2
            ;;
    esac
done

if [ -z "$LOG_PATH" ] || [ -z "$EXIT_CODE" ] || [ -z "$WORKER" ]; then
    echo "ERROR: --log, --exit-code, and --worker are all required" >&2
    usage
    exit 2
fi

if ! [[ "$EXIT_CODE" =~ ^[0-9]+$ ]]; then
    echo "ERROR: --exit-code must be a non-negative integer, got: $EXIT_CODE" >&2
    exit 2
fi

if ! [[ "$THRESHOLD" =~ ^[1-9][0-9]*$ ]]; then
    echo "ERROR: --threshold must be a positive integer, got: $THRESHOLD" >&2
    exit 2
fi

if ! [[ "$WORKER" =~ ^[A-Za-z0-9_.-]+$ ]]; then
    echo "ERROR: --worker must match ^[A-Za-z0-9_.-]+\$, got: $WORKER" >&2
    exit 2
fi

if ! command -v python3 >/dev/null 2>&1; then
    echo "ERROR: python3 is required to parse --log and is not on PATH" >&2
    exit 1
fi

echo "check-agent-run-result: worker=$WORKER log=$LOG_PATH exit_code=$EXIT_CODE threshold=$THRESHOLD"
echo

# ---- parse + classify the log --------------------------------------------
# The embedded interpreter owns the "did this run succeed" verdict; the bash
# wrapper around it only turns that verdict into state-file bookkeeping and
# escalation. See FAIL CLOSED above for why "no result object" and "the
# result object says it failed" and "the log is truncated" are all just
# reasons — they collapse to the same non-zero exit here.
if PY_OUTPUT="$(python3 - "$LOG_PATH" "$EXIT_CODE" <<'PYEOF'
import json
import sys

LOG_PATH, EXIT_CODE = sys.argv[1], int(sys.argv[2])

# Both phrases genuinely observed, verbatim, in the "result" string of all
# three real 2026-09 incident logs cited in this script's own header —
# matched ONLY against that one designated field below, never against the
# raw log text as a whole (see the CLASSIFY, DON'T JUST DETECT section of
# the header for why a whole-log substring match is the wrong shape here).
AUTH_FAILURE_PHRASES = ("Failed to authenticate", "OAuth session expired")

# The only terminal_reason value observed across all three real incident
# logs. Deliberately a short, explicit set rather than a broad heuristic —
# widen it only against another verified incident, not a guess.
ERROR_TERMINAL_REASONS = {"api_error"}


def extract_json_objects(blob):
    """Scan `blob` left to right and return every complete top-level JSON
    object found, in order, plus the byte offset immediately after the
    LAST one that parsed successfully (0 if none did). Uses
    json.JSONDecoder.raw_decode — a real parser — so a quoted string
    containing '{' or '}' is never mistaken for structure. Same scanning
    idiom as scripts/audit-agent-run-deaths.sh's own extract_json_objects,
    reused deliberately rather than reinvented for the same input shape."""
    decoder = json.JSONDecoder()
    objs = []
    last_object_end = 0
    i, n = 0, len(blob)
    while i < n:
        idx = blob.find("{", i)
        if idx == -1:
            break
        try:
            obj, end = decoder.raw_decode(blob, idx)
            objs.append(obj)
            last_object_end = end
            i = end
        except json.JSONDecodeError:
            i = idx + 1
    return objs, last_object_end


def tail_is_corrupt(blob, last_object_end):
    """Is there an unparsed '{' anywhere AFTER the last object that parsed
    completely? A log a process finished writing normally ends exactly at
    the close of its final object — nothing follows it (interim
    checkpoints and the true final result are both complete objects; see
    extract_json_objects). A log whose writer died mid-write instead
    leaves an unterminated `{...` fragment trailing after that point.

    Deliberately scoped to the slice AFTER last_object_end, not a
    whole-blob search for the last '{' — a real result's own "result" text
    field routinely contains literal '{' characters (an agent quoting code,
    or discussing this very script's header, which itself quotes example
    JSON). Searching the whole blob for the last '{' finds THAT brace —
    inside a string, already fully consumed as part of a valid object — and
    misreports a completely healthy log as truncated. Restricting the
    search to content the forward scan above never consumed avoids this by
    construction: anything before last_object_end was already accounted
    for, brace and all."""
    return "{" in blob[last_object_end:]


reasons = []

try:
    with open(LOG_PATH, "r", encoding="utf-8", errors="replace") as f:
        blob = f.read()
except OSError as e:
    print(f"ERROR: could not read --log path '{LOG_PATH}': {e}", file=sys.stderr)
    print("MACHINE_REASONS=LOG_UNREADABLE")
    print("MACHINE_AUTH_FAILURE=0")
    sys.exit(1)

if EXIT_CODE != 0:
    reasons.append(f"EXIT_CODE_NONZERO({EXIT_CODE})")

objs, last_object_end = extract_json_objects(blob)

if tail_is_corrupt(blob, last_object_end):
    reasons.append("LOG_TAIL_CORRUPT")

results = [o for o in objs if isinstance(o, dict) and o.get("type") == "result"]

result_obj = None
if results:
    # A run can emit more than one "type":"result" object — an interim one
    # at a task-notification checkpoint as well as the true final one at
    # exit (documented in audit-agent-run-deaths.sh against real KYO-468
    # runs). The LAST one is this run's actual final state.
    result_obj = results[-1]
else:
    reasons.append("NO_RESULT_OBJECT")

is_error = result_obj.get("is_error") if isinstance(result_obj, dict) else None
terminal_reason = result_obj.get("terminal_reason") if isinstance(result_obj, dict) else None
subtype = result_obj.get("subtype") if isinstance(result_obj, dict) else None
num_turns = result_obj.get("num_turns") if isinstance(result_obj, dict) else None
stop_reason = result_obj.get("stop_reason") if isinstance(result_obj, dict) else None
session_id = result_obj.get("session_id") if isinstance(result_obj, dict) else None
result_text = result_obj.get("result") if isinstance(result_obj, dict) else None

if is_error is True:
    reasons.append("IS_ERROR_TRUE")
if terminal_reason in ERROR_TERMINAL_REASONS:
    reasons.append(f"TERMINAL_REASON_{terminal_reason.upper()}")

# subtype is deliberately never consulted as a success/failure signal — see
# the header. It is only ever printed below, for a human reading the log.

result_field_signal = isinstance(result_text, str) and any(
    p in result_text for p in AUTH_FAILURE_PHRASES
)
# The structural counterpart: the assistant-typed message that carries the
# auth failure sets a top-level is_api_error_message=true plus
# error="authentication_failed" — present in all three real incident logs
# cited in the header, on an object of type "assistant", never on a
# tool_result or a message merely discussing the topic in prose.
structured_signal = any(
    isinstance(o, dict)
    and o.get("type") == "assistant"
    and o.get("is_api_error_message") is True
    and o.get("error") == "authentication_failed"
    for o in objs
)
auth_failure = result_field_signal or structured_signal
if auth_failure:
    reasons.append("AUTH_FAILURE")


def fmt(v):
    return "?" if v is None else str(v)


print("Result object: " + ("found" if result_obj is not None else "NOT FOUND"))
if result_obj is not None:
    print(
        f"  session_id={fmt(session_id)} num_turns={fmt(num_turns)} "
        f"stop_reason={fmt(stop_reason)} terminal_reason={fmt(terminal_reason)} "
        f"subtype={fmt(subtype)} is_error={fmt(is_error)}"
    )
print()

if auth_failure:
    print("AUTH FAILURE — this run could not authenticate:")
    if structured_signal:
        print('  - the assistant message carries is_api_error_message=true, error="authentication_failed"')
    if result_field_signal:
        for p in AUTH_FAILURE_PHRASES:
            if p in result_text:
                print(f'  - the final result field contains: "{p}"')
    print("  This needs an interactive `claude /login` on this machine — cron cannot fix it by retrying.")
    print()

print("REASONS: " + (", ".join(reasons) if reasons else "(none)"))
failed = bool(reasons)
print("RESULT: " + ("FAILURE" if failed else "SUCCESS"))
print(f"MACHINE_REASONS={','.join(reasons) if reasons else '(none)'}")
print(f"MACHINE_AUTH_FAILURE={1 if auth_failure else 0}")

sys.exit(1 if failed else 0)
PYEOF
)"; then
    LOG_VERDICT_STATUS=0
else
    LOG_VERDICT_STATUS=$?
fi

echo "$PY_OUTPUT"
echo

# An interpreter crash (a bug in the block above, not a log-content verdict)
# must not read as success either — only exactly 0 or 1 are ever expected.
if [ "$LOG_VERDICT_STATUS" -ne 0 ] && [ "$LOG_VERDICT_STATUS" -ne 1 ]; then
    echo "ERROR: log parsing exited with unexpected status $LOG_VERDICT_STATUS — failing closed." >&2
    LOG_VERDICT_STATUS=1
fi

AUTH_FAILURE_FLAG="$(printf '%s\n' "$PY_OUTPUT" | grep -o '^MACHINE_AUTH_FAILURE=[01]$' | cut -d= -f2 || true)"
[ -n "$AUTH_FAILURE_FLAG" ] || AUTH_FAILURE_FLAG=0

# ---- per-worker consecutive-failure state ---------------------------------
STATE_WRITE_OK=1
if ! mkdir -p "$STATE_DIR" 2>/dev/null; then
    echo "ERROR: could not create state directory '$STATE_DIR' — cannot persist the consecutive-failure counter. Failing closed." >&2
    STATE_WRITE_OK=0
fi

COUNT_FILE="${STATE_DIR}/${WORKER}.count"
ESCALATED_FILE="${STATE_DIR}/${WORKER}.escalated"
FAILURES_LOG="${STATE_DIR}/failures.log"
TIMESTAMP="$(date -u +"%Y-%m-%dT%H:%M:%SZ")"

CURRENT_COUNT=0
if [ "$STATE_WRITE_OK" -eq 1 ] && [ -f "$COUNT_FILE" ]; then
    RAW_COUNT="$(cat "$COUNT_FILE" 2>/dev/null || echo "")"
    if [[ "$RAW_COUNT" =~ ^[0-9]+$ ]]; then
        CURRENT_COUNT="$RAW_COUNT"
    else
        echo "WARNING: state file '$COUNT_FILE' does not hold a plain integer ('$RAW_COUNT') — treating as 0." >&2
    fi
fi

# write_count <value> — atomic: write into a temp file in the SAME
# directory as COUNT_FILE, then rename over it, so a process killed
# mid-write never leaves the next run reading a half-written counter.
write_count() {
    local value="$1" tmp
    if ! tmp="$(mktemp "${STATE_DIR}/.${WORKER}.count.XXXXXX" 2>/dev/null)"; then
        return 1
    fi
    if ! printf '%s\n' "$value" >"$tmp" 2>/dev/null; then
        rm -f "$tmp"
        return 1
    fi
    if ! mv -f "$tmp" "$COUNT_FILE" 2>/dev/null; then
        rm -f "$tmp"
        return 1
    fi
    return 0
}

FINAL_STATUS="$LOG_VERDICT_STATUS"

if [ "$STATE_WRITE_OK" -eq 0 ]; then
    # An unwritable state directory is one of this script's own documented
    # fail-closed triggers, independent of what the log itself said.
    #
    # This branch deliberately returns without touching the counter, the
    # failures log, or the escalation channels: every one of those lives
    # inside the directory we have just established we cannot write to, so
    # attempting them would only produce a second, noisier failure. The
    # consequence is that failures.log can carry a GAP for the duration of a
    # state-directory outage, even for runs whose log content showed a real
    # failure — the durable-record guarantee made above holds only while the
    # state directory is writable. Nothing is silently swallowed: the primary
    # contract still fires, because FINAL_STATUS=1 makes the wrapper exit
    # non-zero and cron mails the run.
    FINAL_STATUS=1
elif [ "$LOG_VERDICT_STATUS" -eq 0 ]; then
    if ! write_count 0; then
        echo "ERROR: could not write '$COUNT_FILE' — failing closed." >&2
        FINAL_STATUS=1
    else
        rm -f "$ESCALATED_FILE" 2>/dev/null || true
        echo "Consecutive failures for '$WORKER': reset to 0."
    fi
else
    NEW_COUNT=$((CURRENT_COUNT + 1))
    if ! write_count "$NEW_COUNT"; then
        echo "ERROR: could not write '$COUNT_FILE' — failing closed." >&2
        FINAL_STATUS=1
    fi

    REASONS_LINE="$(printf '%s\n' "$PY_OUTPUT" | grep -o '^MACHINE_REASONS=.*$' | cut -d= -f2- || true)"
    printf '%s worker=%s log=%s exit_code=%s consecutive=%s reasons=%s\n' \
        "$TIMESTAMP" "$WORKER" "$LOG_PATH" "$EXIT_CODE" "$NEW_COUNT" "${REASONS_LINE:-unknown}" \
        >>"$FAILURES_LOG" 2>/dev/null || echo "WARNING: could not append to '$FAILURES_LOG'." >&2

    echo "Consecutive failures for '$WORKER': $NEW_COUNT (threshold $THRESHOLD)."

    if [ "$NEW_COUNT" -ge "$THRESHOLD" ]; then
        ESCALATION_MSG="ESCALATION: worker '$WORKER' has failed $NEW_COUNT consecutive time(s) (threshold $THRESHOLD). Latest log: $LOG_PATH."
        if [ "$AUTH_FAILURE_FLAG" -eq 1 ]; then
            ESCALATION_MSG="$ESCALATION_MSG This looks like an expired OAuth session — run 'claude /login' on this machine."
        fi
        # Three independent channels — none of which may fail this script:
        # a marker file for anything that later checks state on disk,
        # stderr because cron mails a job's stderr to the box owner, and
        # notify-send only if it is actually present on PATH.
        printf '%s\n' "$ESCALATION_MSG" >"$ESCALATED_FILE" 2>/dev/null || true
        echo "$ESCALATION_MSG" >&2
        if command -v notify-send >/dev/null 2>&1; then
            notify-send -u critical "Kyomi cron: $WORKER failing" "$ESCALATION_MSG" >/dev/null 2>&1 || true
        fi
    fi
fi

echo
echo "RESULT: $([ "$FINAL_STATUS" -eq 0 ] && echo SUCCESS || echo FAILURE)."

exit "$FINAL_STATUS"
