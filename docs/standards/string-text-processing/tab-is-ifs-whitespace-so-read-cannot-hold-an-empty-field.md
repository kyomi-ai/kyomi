# Tab is IFS whitespace, so `read` cannot hold an empty field

`IFS=$'\t' read -r a b c d` looks like the obvious way to split a tab-separated row, and it is
the one thing tab-separated data cannot survive. Tab, space and newline are IFS *whitespace*:
when one of them is the delimiter, the shell folds a run of them into a single separator. A
non-whitespace delimiter does not behave this way, which is why the bug is invisible to anyone
reasoning from the `IFS='|'` version they wrote first.

The consequence is not a truncated field. It is a silent left shift of every field after the
empty one, with the last variable coming out empty — so a guard written as
`[ -n "$last" ] || continue` reads the shifted row as junk and drops the record.

Reproduced directly (bash 5.3):

```sh
line=$'37\tMERGED\t\tbranch'            # a four-column row whose third column is empty

IFS=$'\t' read -r a b c d <<< "$line"
# a=[37] b=[MERGED] c=[branch] d=[]     ← the branch name landed in the date slot
IFS=$'\t' read -r -a arr <<< "$line"
# ${#arr[@]} = 3                        ← `read -a` collapses identically

IFS='|' read -r a b c d <<< '37|MERGED||branch'
# a=[37] b=[MERGED] c=[] d=[branch]     ← a non-whitespace delimiter is exact
```

What makes this dangerous is that it is introduced by *adding a column*. A three-column row is
safe, because only the last field can be empty and an empty tail is indistinguishable from an
absent one. The row grows a fourth column, a middle field becomes emptiable, and a line of code
nobody edited starts discarding records.

**Rule:** Never split a record with `IFS=$'\t' read` (or `read -a`) when any field other than
the last can be empty. Split on something that does not collapse:

- `readarray -t fields <<<"${line//$'\t'/$'\n'}"` — substituting tabs for newlines and reading
  with `readarray -t` yields exactly one element per column, empty ones included. Raises the
  script's floor to bash 4.0.
- `awk -F'\t'` — `NF` and `$3` are both correct for an empty middle field (verified).

Then **count the fields and fail on a wrong count**. A row that did not split into the expected
number of usable fields is a row you *could not read*; it belongs in the same bucket as a
producer that failed outright, not in a `continue`. That guard is what keeps the class
non-silent if someone ever "fixes" the parser back.

State where the delimiter's safety comes from, too. `jq`'s `@tsv` escapes tab, newline,
carriage return and backslash *inside* a field (as the two-character sequences `\t`, `\n`,
`\r`, `\\`), so a literal tab in its output is always a separator and a literal newline never
occurs within one. A producer without that guarantee needs a different delimiter, not a
different reader.

```sh
# WRONG — a reconstruction of the pre-fix form, not a quote. Collapses on an
# empty createdAt: the branch name lands in pr_created, pr_branch comes out
# empty, and the row is skipped entirely.
IFS=$'\t' read -r pr_number pr_state pr_created pr_branch <<< "$pr_line"
[ -n "$pr_branch" ] || continue

# RIGHT — quoted from the `gh pr list` row loop in
# scripts/check-ticket-in-flight.sh. Newline splitting preserves empty fields,
# and a row that does not yield four usable fields becomes a hard failure
# rather than a skipped record.
readarray -t pr_fields <<<"${pr_line//$'\t'/$'\n'}"
if [ "${#pr_fields[@]}" -ne 4 ] || [ -z "${pr_fields[3]}" ]; then
    FAILURES+=("gh pr list: could not read row '$pr_line' as number/state/createdAt/headRefName — the PR check is incomplete, so no verdict can be given")
    continue
fi
```

Real precedent — **KYO-607** (`2026-09-03` review log, the three
`KYO-607: recycled pre-restart ticket keys in check-ticket-in-flight.sh` entries). Adding
`createdAt` to the `gh pr list --jq '… | @tsv'` query turned a safe three-field read into a
four-field one. A PR with an empty `createdAt` collapsed to three fields, `pr_branch` came out
empty, and the pre-existing `[ -n "$pr_branch" ] || continue` dropped the PR — a fail-open in
the one gate that exists to stop two agents from working the same ticket at once. The reviewer
reproduced the collapse rather than reading it (`IFS=$'\t' read -r a b c d <<< $'2\tOPEN\t\tbranch'`
→ `c=branch, d=`) and mutation-tested the replacement by reverting the parser to the old form:
113 passed / 6 failed, including *"an empty createdAt … expected exit 1, got 0"*. Note where it
was caught — the standing comment the fix left above the loop records it as
*"Caught by the KYO-607 self-test, not by review."*

Sibling of
[../error-handling/empty-on-failure-must-not-look-like-a-real-result.md](../error-handling/empty-on-failure-must-not-look-like-a-real-result.md):
both end with a consumer acting on something that was never established. That rule is about an
*exit status* nobody checked, and its remedy is to keep the failure representable across the
boundary. This one is about a delimiter whose semantics re-map fields while every command in
the pipeline succeeds — there is no error to propagate, and the remedy is a parser change plus
a field-count guard. Sibling of
[scan-markdown-for-a-marker-with-code-blocks-excluded.md](scan-markdown-for-a-marker-with-code-blocks-excluded.md)
in this section: the same instinct — model the format before you match — applied to how one
line divides into fields rather than to which lines count as content. And per
[../testing/prove-test-fails-without-fix.md](../testing/prove-test-fails-without-fix.md),
reverting to `IFS=$'\t' read` is a one-line mutation, so a replacement parser that has not been
proven load-bearing that way has not been proven at all.
