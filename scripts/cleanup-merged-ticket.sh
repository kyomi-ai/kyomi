#!/usr/bin/env bash
# Explicit, bounded cleanup of one verified merged PR in any repository.
# Preview is default. Safety checks follow retire-worktree.sh: linked trees
# only, 30m quiet window, process scan, clean status, no stranded tombstone.
# PR head equality replaces its unpushed check to support deleted squash heads.
set -euo pipefail
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
source "$SCRIPT_DIR/lib/merged-ticket.sh"
usage() {
    echo "Usage: $0 <TICKET> --repo-path <checkout> --pr <number> --protect-workspace <path> --protect-branch <branch> [--remote <name>] [--apply]" >&2
    echo 'Default: preview. Exit 0 eligible/already absent, 1 retained, 2 usage, 3 incomplete.' >&2
}
[ "$#" -gt 0 ] || { usage; exit 2; }
ticket="${1^^}"; shift
ticket="KYO-${ticket#KYO-}"
[[ "$ticket" =~ ^KYO-[0-9]+$ ]] || { usage; exit 2; }
repo_path='' pr='' protect_workspace='' protect_branch='' remote=origin apply=0
while [ "$#" -gt 0 ]; do
    case "$1" in
        --apply) apply=1; shift ;;
        --repo-path|--pr|--protect-workspace|--protect-branch|--remote)
            [ "$#" -ge 2 ] || { usage; exit 2; }
            case "$1" in
                --repo-path) repo_path="$2" ;;
                --pr) pr="$2" ;;
                --protect-workspace) protect_workspace="$2" ;;
                --protect-branch) protect_branch="$2" ;;
                --remote) remote="$2" ;;
            esac
            shift 2 ;;
        *) usage; exit 2 ;;
    esac
done
[ -n "$repo_path" ] && [ -n "$protect_workspace" ] && [ -n "$protect_branch" ] &&
    [[ "$pr" =~ ^[1-9][0-9]*$ ]] || { usage; exit 2; }
repo_path="$(realpath -e "$repo_path")" || exit 2
protect_workspace="$(realpath -e "$protect_workspace")" || exit 2
# Resolve GitHub identity from the selected remote, not this script's repo.
url="$(git -C "$repo_path" config --get "remote.$remote.url")" || exit 3
case "$url" in
    https://github.com/*) repo="${url#https://github.com/}" ;;
    git@github.com:*) repo="${url#git@github.com:}" ;;
    ssh://git@github.com/*) repo="${url#ssh://git@github.com/}" ;;
    *) echo 'Cannot identify GitHub repository from selected remote' >&2; exit 3 ;;
esac
repo="${repo%.git}"
[[ "$repo" =~ ^[A-Za-z0-9_.-]+/[A-Za-z0-9_.-]+$ ]] || exit 3
# Validate effective URLs: pushurl/insteadOf/pushInsteadOf must never send
# deletion to a repository other than the one whose PR was verified.
validate_transport() {
    local effective push_urls destination
    effective="$(git -C "$repo_path" remote get-url "$remote")" || return 3
    push_urls="$(git -C "$repo_path" remote get-url --push --all "$remote")" || return 3
    case "$effective" in
        "https://github.com/$repo"|"https://github.com/$repo.git"|"git@github.com:$repo"|"git@github.com:$repo.git"|"ssh://git@github.com/$repo"|"ssh://git@github.com/$repo.git") ;;
        *) echo "RETAINED: effective fetch URL redirects repository identity: $effective" >&2; return 1 ;;
    esac
    [ -n "$push_urls" ] || return 3
    while IFS= read -r destination; do
        if [ "$destination" != "$effective" ]; then
            echo "RETAINED: effective push destination differs from verified fetch URL: $destination" >&2
            return 1
        fi
    done <<<"$push_urls"
}
if validate_transport; then :; else exit "$?"; fi
cd "$repo_path"
if verify_merged_ticket_pr "$repo" "$pr" "$remote"; then :; else
    status=$?; echo "RETAINED: PR #$pr completion not verified"; exit "$status"
fi
lower="${MT_BRANCH,,}" slug="${ticket,,}"
case "$lower" in *"$slug-"*|*"$slug") ;; *) echo 'PR branch does not match ticket' >&2; exit 2 ;; esac
# Python handles NUL-separated file paths, /proc errors, lock ownership and
# log copying without shell word splitting. GitHub/Git evidence stays shared.
if merged_ticket_no_open_pr "$repo" "$MT_BRANCH"; then :; else exit "$?"; fi
python3 - "$repo_path" "$ticket" "$pr" "$MT_BRANCH" "$MT_HEAD" "$MT_DEFAULT" "$remote" "$protect_workspace" "$protect_branch" "$apply" "$SCRIPT_DIR/lib/merged-ticket.sh" "$repo" <<'PY'
import contextlib
import fcntl
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile
import time

root, ticket, pr, branch, head, default, remote, protected, protected_branch, apply, evidence_lib, repo = sys.argv[1:]
apply = apply == '1'
QUIET_SECONDS = 30 * 60


def git(*args, cwd=root):
    return subprocess.check_output(['git', '-C', str(cwd), *args], stderr=subprocess.STDOUT).decode()


def retained(reason):
    print('RETAINED: ' + reason)
    raise SystemExit(1)


def safety(tree):
    if tree.resolve() == Path(protected) or branch == protected_branch:
        retained('explicitly protected invoking workspace/branch')
    if tree == primary:
        retained('primary checkout')
    if (tree / 'STRANDED.md').exists() or (tree / 'STRANDED.md').is_symlink():
        retained('STRANDED.md tombstone')
    if git('rev-parse', 'HEAD', cwd=tree).strip() != head or git('symbolic-ref', '--short', 'HEAD', cwd=tree).strip() != branch:
        retained('worktree branch/HEAD changed')
    if git('status', '--porcelain', '--untracked-files=all', cwd=tree):
        retained('dirty or untracked work in ' + str(tree))
    logs = []
    ignored = git('ls-files', '--others', '--ignored', '--exclude-standard', '-z', cwd=tree).split('\0')
    for name in filter(None, ignored):
        path = tree / name
        if path.is_symlink():
            retained('ignored symlink: ' + name)
        if name.startswith('target/'):
            continue  # standard disposable Cargo build output
        if path.parent == tree / 'docs/review-logs' and path.suffix == '.md' and path.is_file():
            logs.append(path)
        else:
            retained('valuable ignored content: ' + name)
    # As in retire-worktree.sh, exclude only target/ and .git from recency.
    cutoff = time.time() - QUIET_SECONDS
    def walk_error(error):
        raise error
    for directory, dirs, files in os.walk(tree, onerror=walk_error):
        if Path(directory) == tree:
            dirs[:] = [d for d in dirs if d not in ('target', '.git')]
        for name in files:
            if Path(directory) == tree and name == '.git':
                continue
            if (Path(directory) / name).lstat().st_mtime > cutoff:
                retained('recent writes within 30 minutes in ' + str(tree))
    if not Path('/proc').is_dir():
        raise RuntimeError('/proc unavailable')
    prefix = str(tree.resolve())
    for proc in Path('/proc').iterdir():
        if not proc.name.isdigit() or int(proc.name) == os.getpid():
            continue
        try:
            if proc.stat().st_uid != os.getuid():
                continue
        except FileNotFoundError:
            continue
        except PermissionError:
            # Cannot identify this PID's owner: best-effort scan, as retirement.
            continue
        # cwd and descriptors are independent evidence. A vanished/unreadable
        # cwd or FD must never suppress another readable writer in this PID.
        try:
            cwd = os.readlink(proc / 'cwd')
        except (FileNotFoundError, PermissionError):
            cwd = None
        if cwd == prefix or (cwd is not None and cwd.startswith(prefix + '/')):
            retained('live process using worktree: pid ' + proc.name + ' cwd ' + cwd)
        try:
            descriptors = list((proc / 'fd').iterdir())
        except (FileNotFoundError, PermissionError):
            continue
        for descriptor in descriptors:
            try:
                destination = os.readlink(descriptor)
            except (FileNotFoundError, PermissionError):
                continue
            # Conservatively retain for any readable FD into the tree,
            # including readonly files/directories and deleted open files.
            if destination == prefix or destination.startswith(prefix + '/'):
                retained('live process open descriptor using worktree: pid ' + proc.name
                         + ' fd ' + descriptor.name + ' -> ' + destination)
    return logs


def transport():
    effective = git('remote', 'get-url', remote).strip()
    destinations = git('remote', 'get-url', '--push', '--all', remote).splitlines()
    valid = {f'https://github.com/{repo}', f'https://github.com/{repo}.git',
             f'git@github.com:{repo}', f'git@github.com:{repo}.git',
             f'ssh://git@github.com/{repo}', f'ssh://git@github.com/{repo}.git'}
    if effective not in valid or not destinations or any(url != effective for url in destinations):
        retained('effective fetch/push destinations changed or redirect repository identity')


def current_remote():
    rows = git('ls-remote', '--heads', remote, 'refs/heads/' + branch).splitlines()
    if not rows:
        return None
    if len(rows) != 1 or rows[0].split('\t')[1] != 'refs/heads/' + branch:
        raise RuntimeError('malformed remote head response')
    sha = rows[0].split('\t')[0]
    if sha != head:
        retained('remote branch has later/different commits')
    return sha


try:
    common = Path(git('rev-parse', '--path-format=absolute', '--git-common-dir').strip())
    lock = common / 'backlog-fast-locks' / ticket
    if lock.exists():
        retained('existing pickup reservation ' + str(lock))
    with contextlib.ExitStack() as stack:
        agent_lock = common / 'ticket-agent' / (ticket + '.lock')
        if agent_lock.exists():
            fd = stack.enter_context(agent_lock.open('r'))
            try:
                fcntl.flock(fd, fcntl.LOCK_EX | fcntl.LOCK_NB)
            except BlockingIOError:
                retained('live ticket-agent claim')
        if apply:
            lock.parent.mkdir(exist_ok=True)
            try:
                lock.mkdir()
            except FileExistsError:
                retained('pickup reservation acquired by another worker')
            stack.callback(lock.rmdir)
        if apply:
            transport()
            # Network evidence is refreshed under our owned pickup reservation.
            verify = subprocess.run(['bash', '-c',
                'source "$1"; verify_merged_ticket_pr "$2" "$3" "$4" || exit $?; '
                '[ "$MT_BRANCH" = "$5" ] && [ "$MT_HEAD" = "$6" ] || exit 1; '
                'merged_ticket_no_open_pr "$2" "$5"',
                'merged-cleanup', evidence_lib, repo, pr, remote, branch, head], cwd=root)
            if verify.returncode == 1:
                retained('PR evidence changed under cleanup reservation')
            if verify.returncode != 0:
                raise RuntimeError('PR verification incomplete under cleanup reservation')
        blocks = git('worktree', 'list', '--porcelain').strip().split('\n\n')
        trees = []
        primary = None
        for block in blocks:
            entry = dict(line.split(' ', 1) for line in block.splitlines() if ' ' in line)
            path = Path(entry['worktree'])
            primary = primary or path
            if entry.get('branch') == 'refs/heads/' + branch:
                if 'locked' in entry or any(line == 'locked' for line in block.splitlines()):
                    retained('locked worktree ' + str(path))
                trees.append(path)
        if branch in (protected_branch, default) or branch.startswith('stranded/'):
            retained('protected/default/stranded branch ' + branch)
        local = git('for-each-ref', '--format=%(objectname)', 'refs/heads/' + branch).strip()
        if local and local != head:
            retained('local branch has later/different commits')
        remote_head = current_remote()
        plans = [(tree, safety(tree)) for tree in trees]
        mode = 'APPLY' if apply else 'PREVIEW'
        print(f'{mode}: PR #{pr} merged and present; exact head {head}; branch {branch}')
        for tree, logs in plans:
            print(f'  remove clean idle linked worktree: {tree}')
            for log in logs:
                print(f'  rescue review log before removal: {log}')
        if local:
            print('  delete exact local branch: ' + branch)
        if remote_head:
            print('  delete remote branch with exact expected-SHA lease: ' + branch)
        if not apply:
            raise SystemExit(0)
        for tree, logs in plans:
            safety(tree)  # re-check immediately before rescue/removal
            if logs:
                archive_root = common / 'merged-ticket-rescues' / ticket
                archive_root.mkdir(parents=True, exist_ok=True)
                archive = Path(tempfile.mkdtemp(prefix='pr-' + pr + '-', dir=archive_root))
                for log in logs:
                    destination = archive / log.name
                    shutil.copy2(log, destination)
                    if destination.read_bytes() != log.read_bytes():
                        raise RuntimeError('review log rescue verification failed')
                print('RESCUED: ' + str(archive))
            # Copying logs can take time: verify files, activity and refs again.
            safety(tree)
            transport()
            current_remote()
            git('worktree', 'remove', str(tree))  # never --force
        if local:
            transport()
            if 'branch refs/heads/' + branch in git('worktree', 'list', '--porcelain').splitlines():
                retained('branch checked out again before deletion')
            # Compare-and-delete supports squash merges without branch -D.
            git('update-ref', '-d', 'refs/heads/' + branch, head)
        if remote_head:
            transport()
            current_remote()
            git('push', '--force-with-lease=refs/heads/' + branch + ':' + head,
                remote, ':refs/heads/' + branch)
        print('CLEANED: verified completed artifacts')
except (OSError, RuntimeError, subprocess.CalledProcessError) as error:
    print('INCOMPLETE: ' + str(error), file=sys.stderr)
    if isinstance(error, subprocess.CalledProcessError):
        print(error.output.decode(), file=sys.stderr)
    sys.exit(3)
PY
