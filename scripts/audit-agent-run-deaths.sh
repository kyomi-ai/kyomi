#!/usr/bin/env bash
# ------------------------------------------------------------------------------
# scripts/audit-agent-run-deaths.sh — detect autonomous runs that silently
# discarded a sub-agent's work while still reporting success. (KYO-546)
#
# WHY THIS EXISTS
#
# Six consecutive `/backlog-fast` cron runs claimed KYO-468 and died 10-16
# minutes later, always before pushing anything. Every one of them exited
# clean: `"is_error":false`, `"stop_reason":"end_turn"`,
# `"terminal_reason":"completed"`, `"subtype":"success"`, and cron logged a
# normal `CMDEND`. `dmesg` is permission-denied on this box, so an empty grep
# against it would have been a FALSE NEGATIVE for OOM; `journalctl -k`
# independently confirmed zero OOM kills across the whole window. It was
# never OOM and never a timeout.
#
# The actual cause: the orchestrator dispatched a sub-agent via the Agent
# tool's `run_in_background` (which defaults to background), then ended its
# own turn to "wait" for it. Under `claude -p` there is no turn after the
# last one — the process exits at `end_turn` and takes every in-flight
# background sub-agent down with it. The harness records this as
# `subagent_stats.killed.system`, and reports the *parent* run as a success
# regardless, because the parent itself really did finish cleanly — it just
# finished having thrown its own child's work away.
#
# Verified across every cron run record on this box: `killed.system > 0`
# NEVER occurred on a run with `started_in_background == 0` — there is not
# one counterexample. The converse does not hold, and that is the
# interesting part: background alone is not always fatal. Several runs (e.g.
# sessions fefb49bb, 249b739a, e5ec864a) started background sub-agents and
# survived, because that particular sub-agent happened to finish before the
# parent's own turn ended — a race, not a guarantee. The long, successful
# KYO-468 sibling runs that actually opened a PR (46, 53, 56 turns) sidestep
# the race entirely by using FOREGROUND sub-agents exclusively, and had zero
# system kills. All six KYO-468 death attempts had `started_in_background
# >= 1` and `killed.system >= 1`:
#
#   Attempt  Session   started_in_background  completed  killed.system
#   1        976062a9  2                      1          1
#   2        1c70589d  2                      1          1
#   3        6cea5fa5  2                      0          2
#   4        811af9fa  2                      1          1
#   5        514d8a54  1                      0          1
#   6        fb3edb6b  2                      2          1
#
# This script makes that failure detectable instead of invisible — it is the
# audit half; the prevention half is the new standard at
# docs/standards/agent-orchestration/no-background-subagents-under-headless-run.md.
#
# WHAT IT READS AND WHY RECONSTRUCTION IS NEEDED, NOT OPTIONAL
#
# The cron wrappers under ~/.local/bin/kyomi-*-cron.sh run
# `claude -p ... --output-format stream-json 2>&1 | tee "$LOGFILE"`, so their
# entire stdout+stderr — one JSON object per line, per Claude Code's
# stream-json contract — passes through cron, which journald records as a
# `CMDOUT (...)` entry per line cron read from the pipe.
#
# Cron does NOT read the child's output one logical line at a time. It reads
# in fixed-size chunks, and a JSON line longer than that chunk gets split
# across multiple consecutive `CMDOUT` entries mid-token — measured on this
# box's real journal, e.g. one entry ending `"maxOutputTokens":640)` followed
# immediately by another beginning `00,"canonicalModel":...)`, i.e. the value
# `64000` split as `640` + `00`. Treating each `CMDOUT` line as one complete
# JSON value would silently corrupt or drop most of a run's final result.
#
# The fix is to concatenate every `CMDOUT` payload for a run, IN ORDER, with
# NO separator inserted — cron's chunking never adds or removes bytes, it
# only decides where to cut, so concatenating the chunks back together
# reproduces the original byte stream exactly, whether or not a cut fell on a
# token boundary. A real JSON parser (Python's `json` module, used via
# `raw_decode` to scan for complete top-level values) is then used to pull
# out every complete JSON object from that reconstructed stream, in order —
# this is immune to stray `{`/`}`/`(`/`)` characters inside quoted string
# values (e.g. "Priority: Normal (3)" inside a result string), because a real
# parser tracks string/escape state and a hand-rolled brace counter does not.
# A single run can contain MORE THAN ONE `"type":"result"` object (Claude
# Code emits an interim one at each task-notification checkpoint as well as
# the true final one at exit) — this script always takes the LAST one, since
# that is the run's actual final state.
#
# FAIL CLOSED — READ THIS BEFORE "FIXING" IT BACK (KYO-511, and see
# docs/standards/error-handling/empty-on-failure-must-not-look-like-a-real-result.md)
#
# A run whose `killed.system` cannot be determined — no `"type":"result"`
# object found at all, or `subagent_stats`/`subagent_stats.killed.system`
# missing from it — is NOT treated as `killed.system == 0`. Silently
# defaulting a missing field to 0 is exactly the failure mode that standard
# describes: an empty/missing value degrading into what looks like a clean
# answer. Such a run is reported as INDETERMINATE and the whole script exits
# 3, the same as "no records parsed at all" — never 0. Likewise this script
# never pipes a status-bearing command (`journalctl`, the embedded `python3`
# parser) into `wc -l`/`grep -c`/`head` and discards its exit status; both
# are invoked directly and their exit status is checked before their output
# is trusted, matching the two KYO-511 bugs this exact workflow already
# produced by doing the opposite.
#
# USAGE
#
#   audit-agent-run-deaths.sh [WINDOW] [--from-file <path>]
#
#   WINDOW           a journalctl --since expression, e.g. "3 days ago" or
#                     "2026-08-25". Default: "7 days ago".
#   --from-file PATH  read journal records from PATH instead of invoking
#                     journalctl. PATH must contain one JSON object per line,
#                     in the exact shape `journalctl -o json
#                     --output-fields=_PID,MESSAGE,__REALTIME_TIMESTAMP`
#                     produces. Used by the self-test
#                     (scripts/audit-agent-run-deaths-test.sh) so it never
#                     depends on the real journal, which will not contain the
#                     test's fixtures on someone else's machine. WINDOW is
#                     ignored (but harmless) when this is given.
#
# EXIT CODES
#
#   0  no run in the window shows killed.system > 0 — nothing silently died.
#   1  at least one run shows killed.system > 0 — a sub-agent's work was
#      silently discarded while the run reported success. Do not treat the
#      window as clean.
#   2  usage error (unknown flag, --from-file given without a value or
#      pointing at an unreadable path).
#   3  a check could not be completed: journalctl failed or is unavailable,
#      zero run records were parsed in the window at all, or a parsed run's
#      kill-stats could not be determined. Treat exactly like exit 1 — never
#      like exit 0. This is deliberately the same fail-closed contract
#      scripts/check-ticket-in-flight.sh uses for its own exit 3.
#  42  this script's own on-disk content is stale relative to origin/main
#      AND KYOMI_STALE_TOOLING_STRICT=1 is set. See
#      scripts/lib/stale-tooling-guard.sh (KYO-632) — by default this is a
#      loud warning on stderr, not a failure.
#
# Only Python's standard-library `json` module does any JSON parsing (no
# `jq`/no third-party package — this repo already ships standalone `.py`
# helpers under scripts/, e.g. scripts/e2e-regression/seed-test-user.py, so a
# stdlib-only Python dependency alongside bash is an established pattern
# here, not a new one). `shellcheck` is not installed on this box (KYO-514),
# so quoting throughout is deliberately defensive rather than lint-verified.
# ------------------------------------------------------------------------------

set -euo pipefail

SCRIPT_NAME="$(basename "$0")"
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=lib/stale-tooling-guard.sh
source "${SCRIPT_DIR}/lib/stale-tooling-guard.sh"
stale_tooling_guard "${BASH_SOURCE[0]}"

usage() {
    cat >&2 <<EOF
Usage: $SCRIPT_NAME [WINDOW] [--from-file <path>]

  WINDOW            journalctl --since expression (default: "7 days ago")
  --from-file PATH  read journal records from PATH instead of running
                     journalctl (see script header for the required shape)

Exit codes:
  0  clean — no run in the window shows killed.system > 0
  1  at least one run silently discarded sub-agent work (killed.system > 0)
  2  usage error
  3  a check could not be completed (journalctl failed, zero records parsed,
     or a run's kill-stats could not be determined) — treat like exit 1
EOF
}

WINDOW="7 days ago"
FROM_FILE=""
WINDOW_SET=0

while [ "$#" -gt 0 ]; do
    case "$1" in
        --from-file)
            if [ "$#" -lt 2 ]; then
                echo "ERROR: --from-file requires a value" >&2
                exit 2
            fi
            FROM_FILE="$2"
            shift 2
            ;;
        --)
            # Real end-of-options: everything after `--` is positional, so a
            # WINDOW that begins with a dash stays reachable. Previously this
            # arm just dropped the token and re-entered the same dispatch,
            # which meant `-- -3 days` still died as an unknown argument.
            shift
            while [ "$#" -gt 0 ]; do
                if [ "$WINDOW_SET" -eq 1 ]; then
                    echo "ERROR: WINDOW given more than once ('$WINDOW' and '$1')" >&2
                    usage
                    exit 2
                fi
                WINDOW="$1"
                WINDOW_SET=1
                shift
            done
            ;;
        -*)
            echo "ERROR: unknown argument: $1" >&2
            usage
            exit 2
            ;;
        *)
            if [ "$WINDOW_SET" -eq 1 ]; then
                echo "ERROR: WINDOW given more than once ('$WINDOW' and '$1')" >&2
                usage
                exit 2
            fi
            WINDOW="$1"
            WINDOW_SET=1
            shift
            ;;
    esac
done

if ! command -v python3 >/dev/null 2>&1; then
    echo "ERROR: python3 is required to parse journal records and is not on PATH" >&2
    exit 3
fi

RAW_FILE=""
CLEANUP_RAW_FILE=0
cleanup() {
    if [ "$CLEANUP_RAW_FILE" -eq 1 ] && [ -n "$RAW_FILE" ]; then
        rm -f "$RAW_FILE"
    fi
}
trap cleanup EXIT

if [ -n "$FROM_FILE" ]; then
    if [ ! -f "$FROM_FILE" ] || [ ! -r "$FROM_FILE" ]; then
        echo "ERROR: --from-file path does not exist or is not readable: $FROM_FILE" >&2
        exit 2
    fi
    RAW_FILE="$FROM_FILE"
else
    RAW_FILE="$(mktemp)"
    CLEANUP_RAW_FILE=1
    journalctl_stderr_file="$(mktemp)"
    if ! journalctl --since "$WINDOW" SYSLOG_IDENTIFIER=CROND -o json \
        --output-fields=_PID,MESSAGE,__REALTIME_TIMESTAMP \
        >"$RAW_FILE" 2>"$journalctl_stderr_file"; then
        echo "ERROR: journalctl failed (window '$WINDOW'):" >&2
        cat "$journalctl_stderr_file" >&2
        rm -f "$journalctl_stderr_file"
        exit 3
    fi
    rm -f "$journalctl_stderr_file"
fi

echo "Auditing agent-run deaths — window: ${FROM_FILE:+(from file $FROM_FILE)}${FROM_FILE:-$WINDOW}"
echo

# ---- hand off to the embedded parser ---------------------------------------
# The parser owns the exit-code decision (0 clean / 1 killed found /
# 3 could not complete) for everything downstream of "we have journal
# records to look at" — see the header above for why. Usage errors (2) and
# journalctl failures (3) are already handled above, before this runs.
if python3 - "$RAW_FILE" <<'PYEOF'
import json
import re
import sys
from datetime import datetime, timezone

RAW_PATH = sys.argv[1]

# Matches the kyomi cron wrappers this audit cares about:
# ~/.local/bin/kyomi-backlog-cron.sh, kyomi-needs-build-cron.sh,
# kyomi-merge-sweeper-cron.sh, kyomi-triage-cron.sh, etc. Matched against the
# CMDEND line's basename only.
CRON_SCRIPT_RE = re.compile(r"^kyomi-[A-Za-z0-9_-]*-cron\.sh$")

# `(<user>) CMD(OUT|END) (<payload>)` — greedy `.*` anchored to end-of-string
# correctly captures the WHOLE payload even when it contains literal ')'
# characters (e.g. "Priority: Normal (3)" inside a result string), because
# cron's own wrapping parens are exactly the first '(' after "CMD(OUT|END) "
# and the very last ')' in the message, nothing more.
LINE_RE = re.compile(r"^\(([^)]*)\) CMD(OUT|END) \((.*)\)$", re.DOTALL)


def format_ts(ts_raw):
    if ts_raw is None:
        return "unknown"
    try:
        micros = int(ts_raw)
    except (TypeError, ValueError):
        return "unknown"
    dt = datetime.fromtimestamp(micros / 1_000_000, tz=timezone.utc)
    return dt.strftime("%Y-%m-%d %H:%M:%SZ")


def extract_json_objects(blob):
    """Scan `blob` left to right and return every complete top-level JSON
    object found, in order. Non-JSON text between objects (e.g. the
    "Background tasks still running..." diagnostic line Claude Code prints
    before its final result) is skipped rather than treated as an error —
    only a real '{...}' JSON value is ever appended. Uses json.JSONDecoder's
    raw_decode, a real parser, so quoted string contents are never
    mistaken for structure."""
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


def build_run_record(script_name, ts_raw, blob):
    rec = {
        "script": script_name,
        "timestamp": format_ts(ts_raw),
        "session_id": None,
        "num_turns": None,
        "stop_reason": None,
        "terminal_reason": None,
        "subtype": None,
        "spawned": None,
        "requested_background": None,
        "requested_foreground": None,
        "started_in_background": None,
        "completed": None,
        "failed": None,
        "killed_system": None,
        "indeterminate": True,
        "indeterminate_reason": "no final result JSON found in this run's CMDOUT stream",
    }

    objs = extract_json_objects(blob)
    results = [o for o in objs if isinstance(o, dict) and o.get("type") == "result"]
    if not results:
        return rec
    # A run can emit more than one "type":"result" object (interim
    # task-notification checkpoints as well as the true final one at exit —
    # observed on real KYO-468 attempts). The LAST one is the run's actual
    # final state.
    result = results[-1]

    rec["session_id"] = result.get("session_id")
    rec["num_turns"] = result.get("num_turns")
    rec["stop_reason"] = result.get("stop_reason")
    rec["terminal_reason"] = result.get("terminal_reason")
    rec["subtype"] = result.get("subtype")

    stats = result.get("subagent_stats")
    if not isinstance(stats, dict):
        rec["indeterminate_reason"] = "result JSON has no subagent_stats"
        return rec

    requested = stats.get("requested")
    killed = stats.get("killed")
    rec["spawned"] = stats.get("spawned")
    rec["requested_background"] = requested.get("background") if isinstance(requested, dict) else None
    rec["requested_foreground"] = requested.get("foreground") if isinstance(requested, dict) else None
    rec["started_in_background"] = stats.get("started_in_background")
    rec["completed"] = stats.get("completed")
    rec["failed"] = stats.get("failed")

    killed_system = killed.get("system") if isinstance(killed, dict) else None
    # Deliberately NOT `killed.get("system", 0)` — a missing/non-integer
    # field must stay indeterminate, not silently become "0 = clean". See
    # docs/standards/error-handling/empty-on-failure-must-not-look-like-a-real-result.md.
    if isinstance(killed_system, int) and not isinstance(killed_system, bool):
        rec["killed_system"] = killed_system
        rec["indeterminate"] = False
        rec["indeterminate_reason"] = None
    else:
        rec["indeterminate_reason"] = "subagent_stats.killed.system missing or not an integer"

    return rec


def parse_runs(raw_path):
    try:
        with open(raw_path, "r", encoding="utf-8", errors="replace") as f:
            lines = f.readlines()
    except OSError as e:
        print(f"ERROR: could not read {raw_path}: {e}", file=sys.stderr)
        sys.exit(3)

    buffers = {}  # pid -> list of accumulated CMDOUT payload chunks
    runs = []

    for raw_line in lines:
        raw_line = raw_line.rstrip("\n")
        if not raw_line.strip():
            continue
        try:
            entry = json.loads(raw_line)
        except json.JSONDecodeError:
            # Not a well-formed journalctl -o json record — skip rather than
            # abort, so one corrupt/foreign line can't take down the whole
            # window's parse. A file with NO valid records at all still
            # yields zero runs, which the caller treats as "could not
            # complete" (exit 3) rather than "clean".
            continue

        pid = entry.get("_PID")
        message = entry.get("MESSAGE")
        ts_raw = entry.get("__REALTIME_TIMESTAMP")
        if pid is None or not isinstance(message, str):
            continue

        m = LINE_RE.match(message)
        if not m:
            continue
        _user, kind, payload = m.group(1), m.group(2), m.group(3)

        if kind == "OUT":
            buffers.setdefault(pid, []).append(payload)
        elif kind == "END":
            blob = "".join(buffers.pop(pid, []))
            script_path = payload
            script_name = script_path.rsplit("/", 1)[-1]
            if CRON_SCRIPT_RE.match(script_name):
                runs.append(build_run_record(script_name, ts_raw, blob))
            # Non-matching scripts (other cron jobs sharing CROND) are
            # dropped here too, so a reused pid always starts a fresh buffer.

    return runs


def fmt(v):
    return "?" if v is None else str(v)


def render(runs):
    print(f"Found {len(runs)} kyomi cron run(s) in the window.")
    print()

    if runs:
        # Width the SCRIPT column to the widest value actually present rather
        # than to a constant. A hardcoded 26 silently misaligned every row for
        # kyomi-merge-sweeper-cron.sh (27 chars), and would do so again for any
        # cron wrapper added later.
        script_w = max([len("SCRIPT")] + [len(r["script"]) for r in runs])
        header = (
            f'{"TIMESTAMP (UTC)":<20} {"SCRIPT":<{script_w}} {"SESSION":<10} '
            f'{"TURNS":>5} {"STOP_REASON":<11} {"TERMINAL":<10} '
            f'{"SPAWN":>5} {"REQ_BG":>6} {"REQ_FG":>6} {"START_BG":>8} '
            f'{"DONE":>4} {"FAIL":>4} {"KILL.SYS":>8}'
        )
        print(header)
        print("-" * len(header))
        for r in runs:
            sid = (r["session_id"] or "?")[:8]
            killed_disp = "N/A" if r["indeterminate"] else str(r["killed_system"])
            print(
                f'{r["timestamp"]:<20} {r["script"]:<{script_w}} {sid:<10} '
                f'{fmt(r["num_turns"]):>5} {fmt(r["stop_reason"]):<11} {fmt(r["terminal_reason"]):<10} '
                f'{fmt(r["spawned"]):>5} {fmt(r["requested_background"]):>6} {fmt(r["requested_foreground"]):>6} '
                f'{fmt(r["started_in_background"]):>8} {fmt(r["completed"]):>4} {fmt(r["failed"]):>4} '
                f'{killed_disp:>8}'
            )

    print()
    print("=" * 78)
    print("VERDICT")
    print("=" * 78)

    killed_runs = [r for r in runs if not r["indeterminate"] and r["killed_system"] > 0]
    indeterminate_runs = [r for r in runs if r["indeterminate"]]

    if killed_runs:
        print()
        print(f"{len(killed_runs)} run(s) killed a background sub-agent and still reported success:")
        for r in killed_runs:
            print(
                f'  - {r["timestamp"]}  session={fmt(r["session_id"])}  '
                f'killed.system={r["killed_system"]}  started_in_background={fmt(r["started_in_background"])}  '
                f'completed={fmt(r["completed"])}  stop_reason={fmt(r["stop_reason"])} '
                f'terminal_reason={fmt(r["terminal_reason"])}'
            )

    if indeterminate_runs:
        print()
        print(f"{len(indeterminate_runs)} run(s) could not be assessed (treated as failures, not as clean):")
        for r in indeterminate_runs:
            print(f'  - {r["timestamp"]}  script={r["script"]}  reason: {r["indeterminate_reason"]}')

    print()
    if not runs:
        print("RESULT: NO RUNS PARSED — cannot tell whether anything died silently or not.")
        sys.exit(3)
    if indeterminate_runs:
        print(
            "RESULT: COULD NOT COMPLETE — at least one run's kill-stats could not be "
            "determined; failing closed rather than assuming it was clean."
        )
        sys.exit(3)
    if killed_runs:
        print(
            f"RESULT: {len(killed_runs)} of {len(runs)} run(s) silently discarded sub-agent "
            "work (killed.system > 0) while reporting success."
        )
        sys.exit(1)
    print(f"RESULT: CLEAN — all {len(runs)} run(s) show killed.system == 0.")
    sys.exit(0)


def main():
    runs = parse_runs(RAW_PATH)
    render(runs)


if __name__ == "__main__":
    try:
        main()
    except SystemExit:
        raise
    except Exception as e:  # noqa: BLE001 - deliberate: an unexpected parse
        # failure must fail closed (exit 3), never be mistaken for "clean".
        print(f"ERROR: unexpected failure while parsing the journal: {e}", file=sys.stderr)
        sys.exit(3)
PYEOF
then
    exit_code=0
else
    exit_code=$?
fi

exit "$exit_code"
