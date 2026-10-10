# Shared merged-PR evidence for the read-only guard and explicit cleanup.
# Caller runs in the target repository. No checkout, reset or artifact deletion.
# Returns 0 verified, 1 not merged/present, 3 incomplete. Outputs MT_* globals.
verify_merged_ticket_pr() {
    local repo="$1" number="$2" remote="$3" row refs fetched rc
    local endpoint="repos/{owner}/{repo}/pulls/$number"
    [ -z "$repo" ] || endpoint="repos/$repo/pulls/$number"
    local -a fields=()
    MT_HEAD='' MT_BRANCH='' MT_MERGE='' MT_DEFAULT='' MT_DEFAULT_SHA=''
    if ! row="$(gh api "$endpoint" --jq '[.state, (.merged_at // "-"), .head.ref, .head.sha, (.merge_commit_sha // "-"), .base.repo.full_name, (.head.repo.full_name // "-")] | @tsv')"; then
        echo "Cannot read merged evidence for PR #$number" >&2; return 3
    fi
    readarray -t fields <<<"${row//$'\t'/$'\n'}"
    if [ "${#fields[@]}" -ne 7 ]; then
        echo "Malformed merged evidence for PR #$number" >&2; return 3
    fi
    [ "${fields[0]}" = closed ] && [ "${fields[1]}" != '-' ] || return 1
    if [[ ! "${fields[3]}" =~ ^[0-9a-f]{40}$ || ! "${fields[4]}" =~ ^[0-9a-f]{40}$ ]] ||
        [ -z "${fields[2]}" ] || [ -z "${fields[5]}" ] ||
        { [ -n "$repo" ] && [ "${fields[5]}" != "$repo" ]; }; then
        echo "Invalid merged SHA/repository evidence for PR #$number" >&2; return 3
    fi
    # Fork head names cannot identify branches in this clone.
    [ "${fields[6]}" = "${fields[5]}" ] || return 1
    if ! refs="$(git ls-remote --symref "$remote" HEAD)"; then return 3; fi
    while IFS=$'\t' read -r ref name; do
        if [ "$name" = HEAD ] && [[ "$ref" == 'ref: refs/heads/'* ]]; then
            MT_DEFAULT="${ref#ref: refs/heads/}"
        elif [ "$name" = HEAD ] && [[ "$ref" =~ ^[0-9a-f]{40}$ ]]; then
            MT_DEFAULT_SHA="$ref"
        fi
    done <<<"$refs"
    if [ -z "$MT_DEFAULT" ] || [ -z "$MT_DEFAULT_SHA" ] || ! git check-ref-format "refs/heads/$MT_DEFAULT"; then
        echo "Cannot discover $remote default branch" >&2; return 3
    fi
    # Verify the actual fetch, using a unique owned ref rather than shared
    # FETCH_HEAD or a SHA advertised before the remote default branch moved.
    local receipt verify_ref verify_sha merge_status
    receipt="$(mktemp -d)" || return 3
    verify_ref="refs/kyomi-merged-verification/${receipt##*/}"
    if ! fetched="$(git fetch --no-tags --no-write-fetch-head "$remote" "refs/heads/$MT_DEFAULT:$verify_ref" 2>&1)"; then
        echo "$fetched" >&2
        rmdir "$receipt"
        return 3
    fi
    if ! verify_sha="$(git rev-parse --verify "$verify_ref^{commit}")"; then
        rmdir "$receipt"
        return 3
    fi
    MT_DEFAULT_SHA="$verify_sha"
    if git merge-base --is-ancestor "${fields[4]}" "$verify_sha"; then merge_status=0; else
        rc=$?
        if [ "$rc" -eq 1 ]; then merge_status=1; else merge_status=3; fi
    fi
    # Compare-and-delete only our receipt, never another verifier's ref.
    if ! git update-ref -d "$verify_ref" "$verify_sha"; then merge_status=3; fi
    rmdir "$receipt" || return 3
    [ "$merge_status" = 0 ] || return "$merge_status"
    MT_BRANCH="${fields[2]}" MT_HEAD="${fields[3]}" MT_MERGE="${fields[4]}"
    return 0
}

# Preserve even uncertain protocol reservations; never expire another owner.
# ticket-agent uses flock files, which may remain on disk after release.
merged_ticket_owner_matches() {
    local file="$1" ticket="$2" branch="$3" workspace="$4"
    [ -n "$branch" ] && [ -n "$workspace" ] && [ -f "$file" ] || return 1
    python3 - "$file" "$ticket" "$branch" "$workspace" <<'PYOWNER'
import json
from pathlib import Path
import sys
try:
    file, ticket, branch, workspace = sys.argv[1:]
    owner = json.loads(Path(file).read_text())
    matches = (owner.get('ticket') == ticket and owner.get('branch') == branch
               and isinstance(owner.get('session_id'), str) and bool(owner['session_id'])
               and Path(owner.get('workspace', '')).is_absolute()
               and Path(owner['workspace']).resolve() == Path(workspace).resolve())
    if Path(file).name in ('owner', 'owner.json'):
        matches = matches and isinstance(owner.get('created_at'), str) and bool(owner['created_at'])
    sys.exit(0 if matches else 1)
except (OSError, ValueError, TypeError, AttributeError):
    sys.exit(1)
PYOWNER
}

merged_ticket_claim_status() (
    local ticket="$1" own_branch="${2:-}" own_workspace="${3:-}" common rc
    common="$(git rev-parse --path-format=absolute --git-common-dir)" || return 3
    if [ -e "$common/backlog-fast-locks/$ticket" ]; then
        if merged_ticket_owner_matches "$common/backlog-fast-locks/$ticket/owner.json" "$ticket" "$own_branch" "$own_workspace" ||
           merged_ticket_owner_matches "$common/backlog-fast-locks/$ticket/owner" "$ticket" "$own_branch" "$own_workspace"; then :; else return 1; fi
    fi
    if [ -e "$common/ticket-agent/$ticket.lock" ]; then
        exec {claim_fd}<"$common/ticket-agent/$ticket.lock" || return 3
        if flock --nonblock "$claim_fd"; then return 0; else
            rc=$?
            if [ "$rc" -eq 1 ]; then
                merged_ticket_owner_matches "$common/ticket-agent/$ticket.json" "$ticket" "$own_branch" "$own_workspace" && return 0
                return 1
            fi
            return 3
        fi
    fi
    return 0
)

# A merged PR does not authorize cleanup of a branch reused by an open PR.
merged_ticket_no_open_pr() {
    local repo="$1" branch="$2" rows candidate
    if ! rows="$(gh api --paginate "repos/$repo/pulls?state=open&per_page=100" --jq '.[] | .head.ref')"; then
        echo 'Open PR check incomplete' >&2; return 3
    fi
    while IFS= read -r candidate; do
        if [ "$candidate" = "$branch" ]; then
            echo "RETAINED: open PR reuses branch $branch" >&2; return 1
        fi
    done <<<"$rows"
    return 0
}
