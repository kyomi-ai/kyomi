#!/usr/bin/env bash
# ------------------------------------------------------------------------------
# scripts/repro-headless-subagent-survival.sh — re-establish, on demand, the
# facts KYO-688 recorded about background sub-agent behaviour under
# `claude -p`, instead of asking anyone to trust a citation of them. (KYO-688)
#
# WHY THIS EXISTS
#
# docs/standards/build-toolchain/a-tool-claim-needs-a-reproduction-not-a-citation.md
# is explicit: a claim about how an external tool (here, the Claude Code
# harness itself) behaves needs a reproduction, not a citation — and that
# rule applies with extra force to a claim ABOUT this harness, because the
# harness is exactly the kind of thing that gets silently upgraded out from
# under a standing belief. scripts/audit-agent-run-deaths.sh (KYO-546)
# detects the failure after the fact, from journal records; this script
# instead RE-CREATES the three conditions live, so "does this still happen"
# is one command away rather than an inference from old journal entries.
#
# OBSERVED RESULTS (2026-09-09, harness Claude Code 2.1.258, installed
# 2026-09-02 11:20 local at
# /home/jason/.local/share/claude/versions/2.1.258, Fedora Linux
# 7.0.9-204.fc44.x86_64) — see KYO-688:
#
#   1. `schema` — under `claude -p`, the Agent tool's input JSON Schema DOES
#      expose a `run_in_background` property. A `claude -p --model haiku`
#      run asked to enumerate its own Agent tool's input schema property
#      names returned: `description, isolation, model, prompt,
#      run_in_background, subagent_type`, and confirmed
#      "run_in_background present: YES". The SAME property is ABSENT from
#      the schema in an INTERACTIVE session on the identical harness version
#      (`description, isolation, model, prompt, subagent_type`) — so the
#      parameter's availability is mode-dependent, not universally present
#      or universally absent.
#
#   2. `background` — a sub-agent dispatched with `run_in_background: true`
#      SURVIVES the parent's `end_turn` under `claude -p`. Cron-shaped run
#      (same flags as ~/.local/bin/kyomi-backlog-cron.sh, ANTHROPIC_API_KEY
#      unset so the subscription login is used): the parent dispatched one
#      background sub-agent running a ~150-second foreground command, then
#      immediately ended its own turn. Measured process wall-clock: 166
#      seconds — the harness RE-INVOKED the session and emitted a SECOND
#      "type":"result" object reporting the background agent's completion.
#      Final accounting: requested={background:1,foreground:0,unset:0},
#      started_in_background=1, completed=1, failed=0,
#      killed={parent:0,user:0,system:0}.
#
#   3. `foreground` — dispatching a sub-agent WITHOUT a `run_in_background`
#      parameter at all blocks the Agent tool call for the sub-agent's whole
#      duration. Same harness, same shape: requested={background:0,
#      foreground:1}, started_in_background=0, completed=1, killed.system=0,
#      duration_ms=167150 for a ~150s sub-agent task.
#
# CONCLUSION AS OF THAT DATE: the KYO-546 failure mode (a background
# sub-agent silently killed by the parent's end_turn, reported in
# subagent_stats.killed.system) did NOT reproduce on harness 2.1.258. It was
# real when scripts/audit-agent-run-deaths.sh was written (KYO-546, harness
# versions predating this one) and appears to have been fixed upstream
# since. This script does not assert that background dispatch is now SAFE
# under claude -p — it only reports that this one failure mode did not
# trigger in these three runs. docs/standards/agent-orchestration/
# no-background-subagents-under-headless-run.md is the standing policy and
# is intentionally NOT touched by this script or by KYO-688; revisiting that
# policy is KYO-692.
#
# Re-run this after any harness upgrade. A result that matches the numbers
# above is not guaranteed to keep matching — that is the entire reason this
# is a script instead of a permanent belief.
#
# USAGE
#
#   repro-headless-subagent-survival.sh <schema|background|foreground> [--model MODEL]
#
#   schema      Ask a claude -p session to enumerate its own Agent tool's
#               input schema property names, and report whether
#               run_in_background is among them (fact 1 above). Cheap and
#               fast — does not dispatch a sub-agent.
#   background  Dispatch ONE sub-agent with run_in_background: true and end
#               the parent turn immediately without waiting, to check
#               whether it survives the parent's end_turn (fact 2 above).
#               Takes ~3 minutes and costs subscription quota for both the
#               parent and the sub-agent.
#   foreground  Dispatch ONE sub-agent WITHOUT a run_in_background parameter,
#               to check whether the Agent tool call blocks for the whole
#               sub-agent duration (fact 3 above). Same cost as `background`.
#
#   --model MODEL   Model for both the parent -p session and the sub-agent it
#                    dispatches (background/foreground modes). Default:
#                    haiku — the question under test is harness behaviour,
#                    not model quality, so use the cheapest model.
#
# OUTPUT
#
# Parses the run's `--output-format stream-json` output (newline-delimited
# JSON) with python3's `json` module via `raw_decode`, exactly the technique
# scripts/audit-agent-run-deaths.sh uses to pull JSON objects out of a byte
# stream that a naive line-based parse could split mid-token. A single run
# can emit MORE THAN ONE "type":"result" object — Claude Code emits an
# interim one at each task-notification checkpoint as well as the true final
# one at exit (this is exactly what happens in `background` mode: the
# harness's re-invocation after the sub-agent finishes emits a second result
# object). This script always takes the LAST one, matching
# scripts/audit-agent-run-deaths.sh's documented handling of the same
# ambiguity. It prints that object's session_id, stop_reason,
# terminal_reason, subtype, subagent_stats, and final result text, plus this
# script's own measured wall-clock seconds for the whole `claude -p`
# invocation.
#
# FAIL CLOSED (KYO-511, and see
# docs/standards/error-handling/empty-on-failure-must-not-look-like-a-real-result.md)
#
# This script never pipes a status-bearing command into `wc -l`/`head`/
# `grep -c` and discards its exit status. If the captured output cannot be
# parsed as JSON at all, contains no "type":"result" object, or that object
# has no subagent_stats, this script prints a clear error to stderr and
# exits 3 — it never prints a zero-looking "clean" answer in that case. It
# does not hardcode a verdict about whether the observed behaviour is safe;
# it reports what it measured, so the header above (not this script's exit
# code) is where "did the KYO-546 mechanism reproduce" gets answered by a
# human reading the output.
#
# EXIT CODES
#
#   0  the run completed and a "type":"result" object with subagent_stats
#      was found and printed. This is NOT a safety verdict — read the
#      printed subagent_stats yourself.
#   2  usage error (unknown mode, unknown flag, --model given without a
#      value).
#   3  could not complete: `claude` or `python3` missing from PATH, the
#      `claude -p` invocation itself failed or timed out, or the captured
#      output could not be parsed / had no result object / had no
#      subagent_stats.
# ------------------------------------------------------------------------------

set -euo pipefail

SCRIPT_NAME="$(basename "$0")"

usage() {
    cat >&2 <<EOF
Usage: $SCRIPT_NAME <schema|background|foreground> [--model MODEL]

  schema      Enumerate the Agent tool's input schema property names under
              claude -p (fact 1) — cheap, no sub-agent dispatched.
  background  Dispatch one sub-agent with run_in_background: true and end
              the parent turn immediately, to check whether it survives the
              parent's end_turn (fact 2).
  foreground  Dispatch one sub-agent with no run_in_background parameter at
              all, to check whether the Agent tool call blocks for the
              sub-agent's whole duration (fact 3).

  --model MODEL   model for the parent -p session and any sub-agent it
                  dispatches (default: haiku)

Exit codes:
  0  a "type":"result" object with subagent_stats was found and printed
     (NOT a safety verdict — read the printed output)
  2  usage error
  3  could not complete (missing tool, claude -p failure/timeout, or the
     output could not be parsed / had no usable result)
EOF
}

if [ "$#" -eq 0 ]; then
    usage
    exit 2
fi

MODE="$1"
shift

case "$MODE" in
    schema|background|foreground) ;;
    -h|--help)
        usage
        exit 0
        ;;
    *)
        echo "ERROR: unknown mode: $MODE" >&2
        usage
        exit 2
        ;;
esac

MODEL="haiku"

while [ "$#" -gt 0 ]; do
    case "$1" in
        --model)
            if [ "$#" -lt 2 ]; then
                echo "ERROR: --model requires a value" >&2
                exit 2
            fi
            MODEL="$2"
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

for tool in claude python3 timeout; do
    if ! command -v "$tool" >/dev/null 2>&1; then
        echo "ERROR: $tool is required and is not on PATH" >&2
        exit 3
    fi
done

# ---- mirror ~/.local/bin/kyomi-backlog-cron.sh's environment EXACTLY ------
# This has to reproduce the actual cron environment, not an interactive
# approximation — the whole point of KYO-688 was that behaviour under
# `claude -p` differs from an interactive session (see fact 1 above), so an
# interactive repro would prove nothing about the cron worker.
export HOME="/home/jason"
export PATH="/home/jason/.cargo/bin:/home/jason/.local/bin:/usr/local/bin:/usr/bin:/bin"

cd /home/jason/repos/kyomi || {
    echo "ERROR: cannot cd to /home/jason/repos/kyomi (cron's own cwd)" >&2
    exit 3
}

# shellcheck disable=SC1091
set -a; source .env; set +a

# .env sets ANTHROPIC_API_KEY for the Rust backend's AI features. Unset it
# so the Claude CLI uses the subscription login: in Claude Code's auth
# precedence ANTHROPIC_API_KEY outranks the stored subscription credential,
# so leaving it set would silently bill the Console org at API rates instead
# — this mirrors kyomi-backlog-cron.sh's own comment verbatim.
unset ANTHROPIC_API_KEY

# ---- build the prompt for the requested mode -------------------------------
#
# NOTE on the sub-agent's task command: a standalone `sleep N` in a Bash
# tool call is BLOCKED by this harness ("Blocked: standalone sleep 60"), so
# the dispatched sub-agent is instructed to run
# `python3 -c "import time; time.sleep(150); print(1)"` instead. Do not
# "simplify" this back to `sleep 150` — it will not run, and the repro will
# silently stop exercising the race it exists to demonstrate.
SLEEP_CMD='python3 -c "import time; time.sleep(150); print(1)"'

case "$MODE" in
    schema)
        TIMEOUT_SECS=120
        PROMPT="Describe your own Agent tool's input JSON Schema. Do NOT call any tool — answer only from the schema definition you were given for the Agent tool. List every top-level property name in its \"properties\" object, alphabetically sorted, comma-separated, on one line prefixed exactly with 'PROPERTIES: '. Then on its own line print exactly 'run_in_background present: YES' if run_in_background is literally one of those property names, or exactly 'run_in_background present: NO' if it is not. Print nothing else, then end your turn."
        ;;
    background)
        TIMEOUT_SECS=600
        PROMPT="Use the Agent tool to dispatch exactly ONE sub-agent. Set subagent_type to general-purpose, model to \"${MODEL}\", and run_in_background to true (explicitly true, not omitted). Its task must be EXACTLY this instruction, verbatim, with no shortening: 'Run this exact command with the Bash tool in the foreground and wait for it to finish, then report its stdout: ${SLEEP_CMD}'. As soon as the Agent tool call returns control to you, do not poll it, do not wait for it, do not call any other tool for any reason — immediately reply with the single word DISPATCHED and end your turn."
        ;;
    foreground)
        TIMEOUT_SECS=600
        PROMPT="Use the Agent tool to dispatch exactly ONE sub-agent. Set subagent_type to general-purpose and model to \"${MODEL}\". Do NOT include a run_in_background parameter in the tool call at all — omit it entirely, do not set it to false, just leave it out. Its task must be EXACTLY this instruction, verbatim, with no shortening: 'Run this exact command with the Bash tool in the foreground and wait for it to finish, then report its stdout: ${SLEEP_CMD}'. After the Agent tool call returns, report what the sub-agent said, then end your turn."
        ;;
esac

RAW_FILE="/tmp/kyo688-repro-${MODE}-$(date +%Y%m%d-%H%M%S).log"

echo "Running: claude -p (mode=$MODE, model=$MODEL) — raw stream-json captured to $RAW_FILE" >&2

# </dev/null is required: without piped stdin, claude -p waits 3s for it and
# logs a warning — same reason the cron wrapper redirects it.
start_ts="$(date +%s)"
claude_exit=0
timeout "$TIMEOUT_SECS" claude -p "$PROMPT" \
    --dangerously-skip-permissions \
    --model "$MODEL" \
    --no-session-persistence \
    --verbose \
    --output-format stream-json \
    < /dev/null > "$RAW_FILE" 2>&1 || claude_exit=$?
end_ts="$(date +%s)"
wall_clock=$((end_ts - start_ts))

if [ "$claude_exit" -ne 0 ]; then
    echo "ERROR: claude -p exited $claude_exit (wall clock: ${wall_clock}s) — raw output follows" >&2
    cat "$RAW_FILE" >&2
    exit 3
fi

echo "claude -p exited 0, wall clock: ${wall_clock}s — parsing $RAW_FILE" >&2
echo >&2

# ---- parse the captured stream-json for the LAST "type":"result" object ---
if python3 - "$RAW_FILE" "$wall_clock" <<'PYEOF'
import json
import sys

RAW_PATH = sys.argv[1]
WALL_CLOCK_SECONDS = sys.argv[2]


def extract_json_objects(blob):
    """Scan `blob` left to right and return every complete top-level JSON
    object found, in order. Uses json.JSONDecoder.raw_decode — a real
    parser, so stray '{'/'}' characters inside quoted string values never
    get mistaken for structure. Same technique
    scripts/audit-agent-run-deaths.sh uses on journal-reconstructed output;
    here the input is a direct capture of claude -p's own stdout, but a real
    parser costs nothing extra and is no less correct for it."""
    decoder = json.JSONDecoder()
    objs = []
    i = 0
    n = len(blob)
    while i < n:
        idx = blob.find("{", i)
        if idx == -1:
            break
        try:
            obj, end = decoder.raw_decode(blob, idx)
            objs.append(obj)
            i = end
        except json.JSONDecodeError:
            i = idx + 1
    return objs


try:
    with open(RAW_PATH, "r", encoding="utf-8", errors="replace") as f:
        blob = f.read()
except OSError as e:
    print(f"ERROR: could not read {RAW_PATH}: {e}", file=sys.stderr)
    sys.exit(3)

objs = extract_json_objects(blob)
if not objs:
    print(
        f"ERROR: no JSON objects found in claude -p output ({RAW_PATH}) — "
        "cannot determine the result. This is reported as a failure, never "
        "as a clean/empty answer.",
        file=sys.stderr,
    )
    sys.exit(3)

results = [o for o in objs if isinstance(o, dict) and o.get("type") == "result"]
if not results:
    print(
        f'ERROR: no "type":"result" object found among {len(objs)} JSON '
        f"object(s) parsed from {RAW_PATH} — cannot determine the result.",
        file=sys.stderr,
    )
    sys.exit(3)

# A run can emit more than one "type":"result" object — an interim one at
# each task-notification checkpoint as well as the true final one at exit
# (this is exactly what `background` mode exercises: the harness
# re-invokes the session after the background sub-agent finishes and emits
# a second result object). The LAST one is the run's actual final state —
# matching scripts/audit-agent-run-deaths.sh's documented handling.
result = results[-1]

stats = result.get("subagent_stats")
if not isinstance(stats, dict):
    print(
        "ERROR: the last \"type\":\"result\" object has no subagent_stats — "
        "cannot determine the sub-agent outcome. Deliberately NOT treated "
        "as \"0 sub-agents, all clean\" — see "
        "docs/standards/error-handling/empty-on-failure-must-not-look-like-a-real-result.md.",
        file=sys.stderr,
    )
    sys.exit(3)

print(f"measured_wall_clock_seconds: {WALL_CLOCK_SECONDS}")
print(f"result_objects_seen: {len(results)}  (last one is authoritative)")
print(f"session_id: {result.get('session_id')}")
print(f"stop_reason: {result.get('stop_reason')}")
print(f"terminal_reason: {result.get('terminal_reason')}")
print(f"subtype: {result.get('subtype')}")
print(f"duration_ms: {result.get('duration_ms')}")
print("subagent_stats:")
print(json.dumps(stats, indent=2))
final_text = result.get("result")
if isinstance(final_text, str):
    print("final_result_text:")
    print(final_text)
sys.exit(0)
PYEOF
then
    parse_exit=0
else
    parse_exit=$?
fi

exit "$parse_exit"
