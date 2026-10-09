# When the format has a field for the fact, read the field — never grep the serialization

A detector that answers "did this run fail to authenticate?", "is this ticket in
flight?", "which release does this build belong to?" is usually handed a *structured*
document — JSONL, a `gh --json` payload, an API response — in which the fact it wants
has a designated field. Writing the check as a substring search over the raw text of
the whole document asks a different and much weaker question: *does this string appear
anywhere in the document*, including in prose, in tool output, in quoted source, and
in the document's own commentary.

The false positive that produces is not a tail case. It is the *most likely* input,
because the place a signal phrase most reliably appears is the documentation of the
signal — and once a detector's own header quotes the phrases it detects, any run that
reads, greps or quotes that file classifies itself as a failure.

Two independent instances reached the same failure by different routes. They are **not**
equally durable, and the difference matters when you cite them: the first was caught in
review and its code was never committed to `origin/main`, so the review log is the only
record; the second is live tooling you can read today.

- **`scripts/check-agent-run-result.sh`, `AUTH_FAILURE` check** — *never committed to
  `origin/main`; the diff survives only in the stranded worktree
  `kyomi-wt-kyo-710-cron-auth`, see Motivating findings below. Line numbers from the
  review are not cited here because they no longer resolve in any extant state of the
  file.* The check did `p in blob` over the entire run log rather than over the final
  result object's own fields. Reproduced live by the reviewer: a fully successful run
  (`is_error:false`,
  `terminal_reason:completed`, exit 0) was misclassified `FAILURE` when its transcript
  merely contained an assistant message discussing OAuth expiry, or a `tool_result`
  that had `cat`'d the detector itself — whose header embeds `Failed to authenticate`
  and `OAuth session expired` verbatim. The harness *does* set structured markers for
  a genuine auth failure (`"error":"authentication_failed"`,
  `"is_api_error_message":true`); the detector's own test fixture demonstrated them and
  the production path never read them. Escalating on this signal would have reproduced
  the exact "cries wolf" failure the ticket set out to avoid.
- **`scripts/check-ticket-in-flight.sh:44-60`** — the same shape one layer up. Matching
  `Closes KYO-NN` against PR *bodies* produced false "in flight" verdicts for KYO-411,
  KYO-413 and KYO-406: every hit was a merged PR that listed the ticket as a *deferral*,
  which this repo's PR convention actively requires. The fix was not a better regex over
  the body but a different field — `headRefName` (`.head.ref` in REST), which carries the
  fact directly.

Note what the two have in common and what they do not. Neither was answerable by
anchoring the pattern or by escaping the input; a perfectly anchored match against the
wrong region is still the wrong question. In both cases the remedy was *changing which
bytes get matched at all* — though only the second remedy is committed and enforced
today; the first was a review recommendation on a diff that never landed.

**Rule:** before writing a text match against a structured document, name the field
that holds the fact and read that field. Parse the document with a parser (`jq`,
`--json`, a JSON load) rather than matching its serialization. If no field holds the
fact, scope the match to the narrowest region that is guaranteed to be machine-written
— a single object's `result` string, one header line — and say in a comment why that
region and not the document. When the phrase you are matching also appears in the
matcher's own source or docs, treat that as proof the whole-document search is wrong,
not as an edge case to special-case away.

```bash
# WRONG — the phrase can appear in prose, in tool output, or in this script's own
# header, which quotes it verbatim. A successful run that greps this file is FAILURE.
if grep -qF "OAuth session expired" "$LOG"; then
    verdict=FAILURE
fi

# RIGHT — read the field the format designates for it.
result_json=$(grep '"type":"result"' "$LOG" | tail -1)
is_error=$(printf '%s' "$result_json" | jq -r '.is_error // false')
err_kind=$(printf '%s' "$result_json" | jq -r '.error // ""')
if [ "$is_error" = true ] || [ "$err_kind" = authentication_failed ]; then
    verdict=FAILURE
fi
```

Direction of failure matters as much as the match. Both detectors above drive an
action with real cost — escalating a false outage, or refusing to claim a ticket — so
each must prefer missing a candidate to inventing one, and must validate against the
real corpus rather than hand-written fixtures. The `check-agent-run-result.sh` defect
was found precisely because the reviewer ran it against a synthetic *successful* run,
not against the failing one it was written for. That the detector was caught in review
rather than in production is the reason it has no landed line number to point at.

Motivating findings: **KYO-710** (`2026-09-10`, *"KYO-710 cron auth-failure
detection"*) — the sole 🔴, reproduced live by the reviewer; the diff it was raised
against was never committed and the branch has since been released as stranded, so the
review log is the durable record, not the code. **KYO-471 / KYO-422** — the
`headRefName` precedent, which *is* on `origin/main` and carries its own
"DO NOT MATCH ON PR BODY OR TITLE" comment at `check-ticket-in-flight.sh:44`.

Nearest siblings, both narrower: `scan-markdown-for-a-marker-with-code-blocks-excluded.md`
in this section covers the case where the document has no field for the fact and the
matcher must instead model the format's own "this is not content" construct;
`error-handling/validate-a-suppression-predicate-anchored-not-by-substring-deletion.md`
covers anchoring a predicate that authorizes *skipping* a check. This rule sits before
both: it asks whether the text match should exist at all.
