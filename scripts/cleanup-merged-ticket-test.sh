#!/usr/bin/env bash
# Integration tests: real linked trees and bare origins; only GitHub is stubbed.
# Also exercises completed-work classification in check-ticket-in-flight.sh.
set -euo pipefail
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
CLEANUP="$SCRIPT_DIR/cleanup-merged-ticket.sh"
GUARD="$SCRIPT_DIR/check-ticket-in-flight.sh"
tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT
export GIT_CONFIG_GLOBAL=/dev/null GIT_CONFIG_NOSYSTEM=1
export GIT_AUTHOR_NAME=Test GIT_AUTHOR_EMAIL=test@example.com
export GIT_COMMITTER_NAME=Test GIT_COMMITTER_EMAIL=test@example.com
mkdir "$tmp/bin"
cat > "$tmp/bin/gh" <<'STUB'
#!/usr/bin/env bash
set -eu
if [ "${FAIL_GH:-0}" = 1 ]; then exit 1; fi
if [ "${SIBLING_MODE:-0}" = 1 ]; then
    endpoint="$2"; [ "$2" != --paginate ] || endpoint="$3"
    case "$endpoint" in
        repos/kyomi-ai/kyomi-connect/pulls*) ;;
        *) exit 0 ;;
    esac
fi
if [[ "$*" == *'state=open'* ]]; then
    if [ "${FAIL_OPEN:-0}" = 1 ]; then exit 1; fi
    if [ "${OPEN_UNDER_LOCK:-0}" = 1 ] && [ -d "$FIXTURE/primary/.git/backlog-fast-locks/KYO-853" ]; then
        echo jason/kyo-853-publish-tls
    else cat "$FIXTURE/open"; fi
elif [ "$2" = --paginate ]; then
    cat "$FIXTURE/list"
else
    if [ "${REQUIRE_LOCK:-0}" = 1 ] && [ ! -d "$FIXTURE/primary/.git/backlog-fast-locks/KYO-853" ]; then exit 1; fi
    if [ -n "${EVIDENCE_TRACE:-}" ]; then
        if [ -d "$FIXTURE/primary/.git/backlog-fast-locks/KYO-853" ]; then echo locked >> "$EVIDENCE_TRACE"; else echo unlocked >> "$EVIDENCE_TRACE"; fi
    fi
    number="${2##*/}"
    if [ -f "$FIXTURE/detail-$number" ]; then cat "$FIXTURE/detail-$number"; else cat "$FIXTURE/detail"; fi
fi
STUB
chmod +x "$tmp/bin/gh"
real_git="$(command -v git)"
cat > "$tmp/bin/git" <<'STUB'
#!/usr/bin/env bash
set -eu
case "${GIT_FAULT:-}:$*" in
    status:*'status --porcelain'*) echo 'injected status failure' >&2; exit 1 ;;
    fetch:*'fetch --no-tags'*) echo 'injected fetch failure' >&2; exit 1 ;;
    default:*'ls-remote --symref'*) echo 'injected default discovery failure' >&2; exit 1 ;;
esac
# Test transport is local, but production-visible effective URLs stay GitHub.
# Only actual network operations receive a fixture-local insteadOf mapping.
args=("$@")
workdir="$PWD"
if [ "${1:-}" = -C ]; then workdir="$2"; shift 2; fi
if [ -n "${FIXTURE:-}" ] && [[ "${1:-}" =~ ^(push|fetch|ls-remote)$ ]]; then
    configured="$("$REAL_GIT" -C "$workdir" config --get remote.origin.url)"
    bare="$FIXTURE/origin.git"
    case "$workdir" in
        "$FIXTURE/siblings/"*)
            item="${workdir##*/}"
            [ ! -d "$FIXTURE/$item.git" ] || bare="$FIXTURE/$item.git"
            ;;
    esac
    if [ "${STALE_DEFAULT:-0}" = 1 ] && [ "$1" = ls-remote ] && [ "${2:-}" = --symref ]; then
        "$REAL_GIT" -c "url.$bare.insteadOf=$configured" "${args[@]}"
        "$REAL_GIT" -C "$FIXTURE/primary" push -q --force "$bare" "$REVIEW_HEAD:refs/heads/main"
        exit 0
    fi
    exec "$REAL_GIT" -c "url.$bare.insteadOf=$configured" "${args[@]}"
fi
exec "$REAL_GIT" "${args[@]}"
STUB
chmod +x "$tmp/bin/git"
export REAL_GIT="$real_git"
export PATH="$tmp/bin:$PATH"
pass=0 fail=0 counter=0
fixture() {
    counter=$((counter+1)); export FIXTURE="$tmp/case-$counter"
    mkdir "$FIXTURE"
    git init -q --bare -b main "$FIXTURE/origin.git"
    git init -q -b main "$FIXTURE/primary"
    git -C "$FIXTURE/primary" remote add origin https://github.com/test/completed.git
    printf 'target/\ndocs/review-logs/\nvaluable/\nSTRANDED.md\n' > "$FIXTURE/primary/.gitignore"
    echo base > "$FIXTURE/primary/file"
    git -C "$FIXTURE/primary" add .
    git -C "$FIXTURE/primary" commit -qm base
    git -C "$FIXTURE/primary" push -q origin main
    branch=jason/kyo-853-publish-tls
    git -C "$FIXTURE/primary" worktree add -q -b "$branch" "$FIXTURE/old"
    echo implemented > "$FIXTURE/old/file"
    git -C "$FIXTURE/old" commit -qam implementation
    head="$(git -C "$FIXTURE/old" rev-parse HEAD)"
    git -C "$FIXTURE/old" push -q origin "$branch"
    git -C "$FIXTURE/primary" merge -q --squash "$branch" >/dev/null
    git -C "$FIXTURE/primary" commit -qm squash
    merge="$(git -C "$FIXTURE/primary" rev-parse HEAD)"
    git -C "$FIXTURE/primary" push -q origin main
    printf 'closed\t2026-10-03T00:00:00Z\t%s\t%s\t%s\ttest/completed\ttest/completed\n' "$branch" "$head" "$merge" > "$FIXTURE/detail"
    printf '22\tMERGED\t2026-10-01T00:00:00Z\t%s\t0\n' "$branch" > "$FIXTURE/list"
    : > "$FIXTURE/open"
    age
}
age() { find "$FIXTURE/old" -type f ! -name .git -exec touch -d '2 hours ago' {} +; }
run_cleanup() {
    if output="$("$CLEANUP" KYO-853 --repo-path "$FIXTURE/primary" --pr 22 --protect-workspace "$FIXTURE/primary" --protect-branch main "$@" 2>&1)"; then status=0; else status=$?; fi
}
run_guard() {
    if output="$(cd "$FIXTURE/primary" && "$GUARD" KYO-853 "$@" 2>&1)"; then status=0; else status=$?; fi
}
check() {
    local want="$1" needle="$2"
    if [ "$status" = "$want" ] && [[ "$output" == *"$needle"* ]]; then
        echo "PASS: $needle (exit $status)"; pass=$((pass+1))
    else
        echo "FAIL: expected exit $want and '$needle', got $status"; echo "$output"; fail=$((fail+1))
    fi
}
preserved() {
    if [ -d "$FIXTURE/old" ] && [ "$(git -C "$FIXTURE/old" rev-parse HEAD)" = "$head" ] &&
       git -C "$FIXTURE/primary" show-ref --verify --quiet "refs/heads/$branch"; then
        pass=$((pass+1))
    else echo 'FAIL: original artifacts not preserved'; fail=$((fail+1)); fi
}
fixture
run_guard; check 0 'COMPLETED WORK'; preserved
mkdir -p "$FIXTURE/old/docs/review-logs" "$FIXTURE/old/target"
printf 'review one\n' > "$FIXTURE/old/docs/review-logs/one.md"
printf 'review two\n' > "$FIXTURE/old/docs/review-logs/two.md"
echo disposable > "$FIXTURE/old/target/output"
age
run_cleanup; check 0 'PREVIEW'; preserved
run_cleanup --apply; check 0 'CLEANED'
if [ ! -e "$FIXTURE/old" ] && ! git -C "$FIXTURE/primary" show-ref --verify --quiet "refs/heads/$branch" &&
   [ -z "$(git -C "$FIXTURE/primary" ls-remote --heads origin "refs/heads/$branch")" ] &&
   [ "$(find "$FIXTURE/primary/.git/merged-ticket-rescues" -name '*.md' | wc -l)" = 2 ] &&
   [ "$(cat "$FIXTURE/primary/.git/merged-ticket-rescues/KYO-853/"*/one.md)" = 'review one' ]; then
    echo 'PASS: squash cleanup removes exact refs and rescues both logs'; pass=$((pass+1))
else echo 'FAIL: cleanup/rescue state'; fail=$((fail+1)); fi
run_cleanup --apply; check 0 'CLEANED'

fixture
git -C "$FIXTURE/primary" push -q origin ":$branch"
run_cleanup --apply; check 0 'CLEANED' # deleted remote squash head supported

fixture
echo unfinished >> "$FIXTURE/old/file"
run_guard; check 1 'uncommitted/untracked work'
run_cleanup --apply; check 1 'dirty or untracked'; preserved
fixture
echo unfinished > "$FIXTURE/old/new-file"
run_guard; check 1 'uncommitted/untracked work'
run_cleanup --apply; check 1 'dirty or untracked'; preserved

fixture
git -C "$FIXTURE/old" commit -q --allow-empty -m later
run_guard; check 1 'IN FLIGHT'
run_cleanup --apply; check 1 'local branch has later/different commits'
fixture
git -C "$FIXTURE/primary" update-ref refs/heads/remote-later "$head"
git -C "$FIXTURE/primary" switch -q remote-later
git -C "$FIXTURE/primary" commit -q --allow-empty -m remote-later
git -C "$FIXTURE/primary" push -q origin "HEAD:refs/heads/$branch"
git -C "$FIXTURE/primary" switch -q main
run_guard; check 1 'remote branch:'
run_cleanup --apply; check 1 'remote branch has later/different commits'; preserved

fixture
printf '%s\n' "$branch" > "$FIXTURE/open"
printf '23\tOPEN\t2026-10-04T00:00:00Z\t%s\t0\n' "$branch" >> "$FIXTURE/list"
run_guard; check 1 'PR #23 (OPEN)'
run_cleanup --apply; check 1 'open PR reuses branch'; preserved

fixture
mkdir -p "$FIXTURE/primary/.git/backlog-fast-locks/KYO-853"
run_guard; check 1 'local pickup reservation'
run_guard --self "$branch"; check 0 'CLEAR'
run_cleanup --apply; check 1 'existing pickup reservation'; preserved

fixture
echo 'KYO-853 preserved' > "$FIXTURE/old/STRANDED.md"
run_cleanup --apply; check 1 'STRANDED.md tombstone'; preserved
fixture
mkdir "$FIXTURE/old/valuable"; echo treasure > "$FIXTURE/old/valuable/notes"
age
run_cleanup --apply; check 1 'valuable ignored content'; preserved
fixture
ln -s "$FIXTURE/primary/file" "$FIXTURE/old/valuable-link"
# The non-ignored symlink is also correctly refused as untracked.
run_cleanup --apply; check 1 'dirty or untracked'; preserved

fixture
touch "$FIXTURE/old/file"
run_guard; check 0 'COMPLETED WORK'
run_cleanup --apply; check 1 'recent writes'; preserved
fixture
run_cleanup --protect-workspace "$FIXTURE/old" --apply; check 1 'protected invoking workspace'; preserved
fixture
run_cleanup --protect-branch "$branch" --apply; check 1 'protected/default/stranded'; preserved
fixture
git -C "$FIXTURE/primary" worktree lock "$FIXTURE/old"
run_cleanup --apply; check 1 'locked worktree'; preserved

fixture
export FAIL_GH=1
run_guard; check 3 'COULD NOT COMPLETE'
run_cleanup --apply; check 3 'completion not verified'; preserved
unset FAIL_GH
fixture
export FAIL_OPEN=1
run_cleanup --apply; check 3 'Open PR check incomplete'; preserved
unset FAIL_OPEN
fixture
printf 'malformed\n' > "$FIXTURE/detail"
run_guard; check 3 'verification incomplete'
run_cleanup --apply; check 3 'completion not verified'; preserved

fixture
# A real merge commit not present on the default branch must remain blocking.
git -C "$FIXTURE/primary" push -q origin "$head:refs/heads/main" --force
run_guard; check 1 'completion not verified in default branch'
run_cleanup --apply; check 1 'completion not verified'; preserved

fixture
export OPEN_UNDER_LOCK=1
run_cleanup --apply; check 1 'PR evidence changed under cleanup reservation'; preserved
unset OPEN_UNDER_LOCK
fixture
export EVIDENCE_TRACE="$FIXTURE/evidence-trace"
run_cleanup --apply; check 0 'CLEANED'
if [ "$(cat "$EVIDENCE_TRACE")" = $'unlocked\nlocked' ]; then
    echo 'PASS: apply refreshed evidence under owned reservation'; pass=$((pass+1))
else echo 'FAIL: apply reservation evidence'; fail=$((fail+1)); fi
unset EVIDENCE_TRACE
fixture
for fault in status fetch default; do
    export GIT_FAULT="$fault"
    run_guard; check 3 'COULD NOT COMPLETE'
    run_cleanup --apply
    if [ "$fault" = status ]; then check 3 'INCOMPLETE'; else check 3 'completion not verified'; fi
    # Evidence failures happen before Python for fetch/default.
    unset GIT_FAULT
    preserved
 done

fixture
mkdir -p "$FIXTURE/old/valuable"
ln -s "$FIXTURE/primary/file" "$FIXTURE/old/valuable/symlink"
age
run_cleanup --apply; check 1 'ignored symlink'; preserved

fixture
# Discover a non-main default rather than trusting cached origin/main.
git -C "$FIXTURE/primary" branch -m main trunk
git -C "$FIXTURE/primary" push -q origin trunk
git -C "$FIXTURE/origin.git" symbolic-ref HEAD refs/heads/trunk
run_guard; check 0 'fetched origin/trunk'
run_cleanup --apply; check 0 'CLEANED'

fixture
# Validate every push destination BEFORE removing either local or remote work.
git clone -q --bare "$FIXTURE/origin.git" "$FIXTURE/other.git"
git -C "$FIXTURE/primary" config remote.origin.pushurl "$FIXTURE/other.git"
run_cleanup --apply; check 1 'effective push destination differs'; preserved
if git -C "$FIXTURE/origin.git" show-ref --verify --quiet "refs/heads/$branch" &&
   git -C "$FIXTURE/other.git" show-ref --verify --quiet "refs/heads/$branch"; then
    echo 'PASS: fetch and wrong push repositories both retained'; pass=$((pass+1))
else echo 'FAIL: pushurl deleted a branch'; fail=$((fail+1)); fi
git -C "$FIXTURE/primary" config --add remote.origin.pushurl https://github.com/test/completed.git
run_cleanup --apply; check 1 'effective push destination differs'; preserved
fixture
git -C "$FIXTURE/primary" config url.https://github.com/other/wrong.git.pushInsteadOf https://github.com/test/completed.git
run_cleanup --apply; check 1 'effective push destination differs'; preserved
fixture
git -C "$FIXTURE/primary" config url.https://github.com/other/wrong.git.insteadOf https://github.com/test/completed.git
run_cleanup --apply; check 1 'effective fetch URL redirects'; preserved

fixture
export STALE_DEFAULT=1 REVIEW_HEAD="$head"
run_guard; check 1 'completion not verified in default branch'
run_cleanup --apply; check 1 'completion not verified'; preserved
unset STALE_DEFAULT REVIEW_HEAD
if [ -z "$(git -C "$FIXTURE/primary" for-each-ref refs/kyomi-merged-verification)" ]; then
    echo 'PASS: own verification receipt removed'; pass=$((pass+1))
else echo 'FAIL: verification ref leaked'; fail=$((fail+1)); fi

fixture
# Multiple merged PRs reusing a branch retain every verified branch/head pair.
cp "$FIXTURE/detail" "$FIXTURE/detail-21"
echo phase-two > "$FIXTURE/old/phase-two"
git -C "$FIXTURE/old" add phase-two
git -C "$FIXTURE/old" commit -qm phase-two
head="$(git -C "$FIXTURE/old" rev-parse HEAD)"
git -C "$FIXTURE/old" push -q origin "$branch"
git -C "$FIXTURE/primary" merge -q --squash "$branch" >/dev/null
git -C "$FIXTURE/primary" commit -qm squash-phase-two
merge="$(git -C "$FIXTURE/primary" rev-parse HEAD)"
git -C "$FIXTURE/primary" push -q origin main
printf 'closed\t2026-10-05T00:00:00Z\t%s\t%s\t%s\ttest/completed\ttest/completed\n' "$branch" "$head" "$merge" > "$FIXTURE/detail-22"
new_row="$(printf '22\tMERGED\t2026-10-04T00:00:00Z\t%s\t0' "$branch")"
old_row="$(printf '21\tMERGED\t2026-10-01T00:00:00Z\t%s\t0' "$branch")"
printf '%s\n%s\n' "$new_row" "$old_row" > "$FIXTURE/list"
run_guard; check 0 'COMPLETED WORK'
printf '%s\n%s\n' "$old_row" "$new_row" > "$FIXTURE/list"
run_guard; check 0 'COMPLETED WORK'
printf '23\tOPEN\t2026-10-06T00:00:00Z\t%s\t0\n' "$branch" >> "$FIXTURE/list"
run_guard; check 1 'PR #23 (OPEN)'

fixture
# Own assigned tree plus old completed residue in the SAME clone.
assigned=jason/kyo-853-followup
git -C "$FIXTURE/primary" worktree add -q -b "$assigned" "$FIXTURE/assigned"
mkdir -p "$FIXTURE/primary/.git/backlog-fast-locks/KYO-853" "$FIXTURE/primary/.git/ticket-agent"
printf '{"ticket":"KYO-853","session_id":"fixture-session","created_at":"2026-10-10T00:00:00Z","branch":"%s","workspace":"%s"}\n' "$assigned" "$FIXTURE/assigned" > "$FIXTURE/primary/.git/backlog-fast-locks/KYO-853/owner.json"
cp "$FIXTURE/primary/.git/backlog-fast-locks/KYO-853/owner.json" "$FIXTURE/primary/.git/ticket-agent/KYO-853.json"
python3 - "$GUARD" "$FIXTURE" "$assigned" <<'PYOWN'
import fcntl, json, pathlib, subprocess, sys
script, fixture, branch = sys.argv[1:]
with open(fixture+'/primary/.git/ticket-agent/KYO-853.lock', 'w') as fd:
    fcntl.flock(fd, fcntl.LOCK_EX)
    args = [script, 'KYO-853', '--self', branch]
    own = subprocess.run(args, cwd=fixture+'/assigned', capture_output=True, text=True)
    assert own.returncode == 0 and 'COMPLETED WORK' in own.stdout, own.stdout+own.stderr
    owner = pathlib.Path(fixture+'/primary/.git/backlog-fast-locks/KYO-853/owner.json')
    data = json.loads(owner.read_text()); data['branch'] = 'someone-else'; owner.write_text(json.dumps(data))
    other = subprocess.run(args, cwd=fixture+'/assigned', capture_output=True, text=True)
    assert other.returncode == 1 and 'local pickup reservation' in other.stdout, other.stdout+other.stderr
print('PASS: exact owned same-clone reservation/launcher recognized; competing owner retained')
PYOWN
pass=$((pass+1))

fixture
# Exercise live cwd and launcher flock using child fixtures, while the test
# itself runs synchronously to completion (no shell background test command).
python3 - "$CLEANUP" "$FIXTURE" "$branch" "$GUARD" <<'PY'
import fcntl, os, pathlib, subprocess, sys
script, fixture, branch, guard = sys.argv[1:]
args = [script, 'KYO-853', '--repo-path', fixture+'/primary', '--pr', '22', '--protect-workspace', fixture+'/primary', '--protect-branch', 'main', '--apply']
# This orchestrator's real cwd is in the old tree during the first invocation.
os.chdir(fixture+'/old')
run = subprocess.run(args, capture_output=True, text=True)
assert run.returncode == 1 and 'live process' in run.stdout, run.stdout + run.stderr
os.chdir(fixture+'/primary')
lock = pathlib.Path(fixture+'/primary/.git/ticket-agent/KYO-853.lock')
lock.parent.mkdir()
with lock.open('w') as fd:
    fcntl.flock(fd, fcntl.LOCK_EX)
    run = subprocess.run(args, capture_output=True, text=True)
    assert run.returncode == 1 and 'live ticket-agent claim' in run.stdout, run.stdout + run.stderr
    own = subprocess.run([guard, 'KYO-853', '--self', branch], capture_output=True, text=True)
    assert own.returncode == 0, own.stdout + own.stderr
    competing = subprocess.run([guard, 'KYO-853'], capture_output=True, text=True)
    assert competing.returncode == 1 and 'local pickup reservation' in competing.stdout, competing.stdout + competing.stderr
print('PASS: real cwd process and launcher flock preserved')
PY
pass=$((pass+1))
preserved

fixture
# Real outside-cwd descriptor holders and scratch-only permission/vanish faults.
python3 - "$CLEANUP" "$FIXTURE" "$branch" <<'PYFD'
import os
from pathlib import Path
import shutil
import subprocess
import sys

script, fixture, branch = sys.argv[1:]
base = Path(fixture)
os.chdir(base / 'primary')
args = ['KYO-853', '--repo-path', str(base / 'primary'), '--pr', '22',
        '--protect-workspace', str(base / 'primary'), '--protect-branch', 'main', '--apply']

def intact():
    assert (base / 'old').is_dir(), 'worktree lost'
    for repo in [base / 'primary', base / 'origin.git']:
        assert subprocess.run(['git', '-C', str(repo), 'show-ref', '--verify', '--quiet',
                               'refs/heads/' + branch]).returncode == 0, 'branch lost: ' + str(repo)

def refusal(executable, environment=None, status=1, needle='open descriptor'):
    result = subprocess.run([str(executable), *args], capture_output=True, text=True, env=environment)
    assert result.returncode == status and needle in result.stdout + result.stderr, result.stdout + result.stderr
    intact()
    return result

for relative in ['file', 'target/open-output', 'docs/review-logs/open-log.md']:
    path = base / 'old' / relative
    path.parent.mkdir(parents=True, exist_ok=True)
    if not path.exists():
        path.write_text('preserve open data')
        os.utime(path, (1, 1))
    with path.open('r+'):
        refusal(script)
    print('PASS: outside-cwd writable FD retained tree and both refs: ' + relative)
with (base / 'old/file').open('r'):
    refusal(script)
print('PASS: readonly descriptor conservatively retained')

# Fault injection changes a temporary script copy, never production bytes.
scratch = base / 'scratch-tooling'
(scratch / 'lib').mkdir(parents=True)
shutil.copy2(Path(script).parent / 'lib/merged-ticket.sh', scratch / 'lib/merged-ticket.sh')
original = Path(script).read_text()
faults = '''
original_readlink = os.readlink
original_iterdir = Path.iterdir
original_is_dir = Path.is_dir
fixture_pid = os.environ['PROC_FIXTURE_PID']
unrelated_fd = os.environ['PROC_UNRELATED_FD']
scenario = os.environ['PROC_SCENARIO']

def fault_readlink(path, *args, **kwargs):
    name = str(path)
    if name == '/proc/' + fixture_pid + '/cwd' and scenario == 'cwd-permission':
        raise PermissionError('injected cwd permission')
    if name == '/proc/' + fixture_pid + '/fd/' + unrelated_fd:
        if scenario in ('fd-permission', 'cwd-permission'):
            print('INJECTED: unrelated descriptor permission', file=sys.stderr)
            raise PermissionError('injected unrelated descriptor permission')
        if scenario == 'fd-vanished':
            print('INJECTED: vanished descriptor', file=sys.stderr)
            raise FileNotFoundError('injected vanished descriptor')
        if scenario == 'fd-error':
            raise OSError('injected descriptor scan failure')
    return original_readlink(path, *args, **kwargs)

def fault_iterdir(path):
    if str(path) == '/proc/' + fixture_pid + '/fd' and scenario == 'directory-permission':
        raise PermissionError('injected descriptor directory permission')
    if str(path) == '/proc/' + fixture_pid + '/fd':
        return iter(sorted(original_iterdir(path), key=lambda entry: entry.name != unrelated_fd))
    return original_iterdir(path)

def fault_is_dir(path):
    if str(path) == '/proc' and scenario == 'proc-unavailable':
        return False
    return original_is_dir(path)

os.readlink = fault_readlink
Path.iterdir = fault_iterdir
Path.is_dir = fault_is_dir
'''
copy = scratch / 'cleanup-merged-ticket.sh'
copy.write_text(original.replace('def git(*args, cwd=root):', faults + '\ndef git(*args, cwd=root):', 1))
copy.chmod(0o755)
with (base / 'primary/file').open('r') as unrelated, (base / 'old/file').open('r+'):
    for scenario in ['fd-permission', 'cwd-permission', 'fd-vanished']:
        environment = dict(os.environ, PROC_FIXTURE_PID=str(os.getpid()),
                           PROC_UNRELATED_FD=str(unrelated.fileno()), PROC_SCENARIO=scenario)
        result = refusal(copy, environment)
        assert 'INJECTED:' in result.stderr, 'descriptor fault did not execute'
        print('PASS: ' + scenario + ' does not hide another readable writer FD')
    for scenario in ['fd-error', 'proc-unavailable']:
        environment['PROC_SCENARIO'] = scenario
        refusal(copy, environment, status=3, needle='INCOMPLETE')
        print('PASS: ' + scenario + ' fails incomplete with all artifacts retained')
# An unreadable descriptor directory cannot hide positive readable cwd evidence.
os.chdir(base / 'old')
environment['PROC_SCENARIO'] = 'directory-permission'
refusal(copy, environment, needle='live process')
os.chdir(base / 'primary')
assert Path(script).read_text() == original, 'production script was mutated'
print('PASS: inaccessible FD directory still preserves readable active cwd')
PYFD
pass=$((pass+10))
preserved

fixture
# Same ticket branch names across repos must not inherit --self or owned locks.
mkdir "$FIXTURE/siblings"
export KYOMI_REPOS_ROOT="$FIXTURE/siblings"
ln -s "$FIXTURE/primary" "$KYOMI_REPOS_ROOT/kyomi"
git -C "$FIXTURE/primary" remote set-url origin https://github.com/kyomi-ai/kyomi.git
for item in kyomi-connect kyomi-private chartml kode; do
    git clone -q "$FIXTURE/origin.git" "$KYOMI_REPOS_ROOT/$item"
    identity="kyomi-ai/$item"; [ "$item" != chartml ] || identity=chartml/chartml
    git -C "$KYOMI_REPOS_ROOT/$item" remote set-url origin "https://github.com/$identity.git"
    # Non-Connect sibling fixture origins have no ticket head.
    if [ "$item" != kyomi-connect ]; then
        empty="$FIXTURE/$item.git"
        git init -q --bare -b main "$empty"
        git -C "$KYOMI_REPOS_ROOT/$item" push -q origin main
    fi
 done
sed 's@test/completed@kyomi-ai/kyomi-connect@g' "$FIXTURE/detail" > "$FIXTURE/detail-new"
mv "$FIXTURE/detail-new" "$FIXTURE/detail"
export SIBLING_MODE=1
mkdir -p "$FIXTURE/primary/.git/backlog-fast-locks/KYO-853"
run_guard --self "$branch"; check 0 'SIBLING SWEEP RESULT: CLEAR'
mkdir -p "$KYOMI_REPOS_ROOT/kyomi-connect/.git/backlog-fast-locks/KYO-853"
run_guard --self "$branch"; check 1 'local pickup reservation'
check 1 'SIBLING SWEEP RESULT: IN FLIGHT'
unset SIBLING_MODE KYOMI_REPOS_ROOT

echo "$pass passed; $fail failed"
[ "$fail" -eq 0 ]
