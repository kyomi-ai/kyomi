# A value that degrades to empty on failure must not reach a consumer that reads it as a real result

Substituting an empty collection — or a zero derived from one — for a failed fetch does
not lose the error at the point it happens. It loses it one step later, where a consumer
reads the substituted value as an *answer*. `0` and "genuinely zero" become the same
byte, and every decision downstream is made on a fact that was never established.

This is distinct from [never discarding errors without logging](ok-discards-errors-without-logging.md).
A `warn!` next to the `unwrap_or_else` satisfies that rule and does nothing for this one:
the log goes to the operator, while the empty vec goes to the code that acts. The defect
is not the missing trace, it is that the failure stopped being representable.

It is also worse than a plain silent failure, because a substituted value is *actionable*.
A consumer that receives nothing usually retries or aborts. A consumer that receives `0`
proceeds confidently — and in both incidents below, the wrong action was taken precisely
because the value looked legitimate.

**Rule:** Keep the failure representable across the boundary. Where a producer can fail,
hand the consumer an `Option`/`Result` and let it **omit** the entry rather than record a
value that means something. Derive any count from the un-degraded `Result`, before the
fallback flattens it. In shell, check the status you actually depend on — a pipeline's
`PIPESTATUS`, or a capture with `||` — because a process substitution's exit status is
never checked by the parent shell, `set -e` included.

Ask: *if this fetch failed, can the consumer tell?* If the answer is no, the fallback is
the bug, not the missing log.

```rust
// WRONG — the count is `.len()` of a vec that already degraded to empty.
// A transient query failure is now indistinguishable from "no rows".
let sessions = list_sessions_for_sync(db, ws).await
    .unwrap_or_else(|e| { warn!(error = %e, "fetch failed"); vec![] });
counts.insert("chat_sessions".to_string(), sessions.len() as i64);

// RIGHT — derive the count from the Result *before* the fallback flattens it,
// and omit the key entirely when it could not be established.
let sessions_result = list_sessions_for_sync(db, ws).await;
insert_count_if_present(
    &mut counts,
    "chat_sessions",
    sessions_result.as_ref().ok().map(|v| v.len() as i64),
);
let sessions = sessions_result
    .unwrap_or_else(|e| { warn!(error = %e, "fetch failed"); vec![] });
```

```sh
# WRONG — a failure inside the process substitution is never seen. `set -e`
# tests mapfile's own (successful) status, so the script prints a partial or
# empty array and exits 0.
mapfile -t sorted < <(printf '%s\n' "${matches[@]}" | sort)

# RIGHT — the capture puts `sort` directly under `||`, so a failure is fatal.
sorted_str=$(printf '%s\n' "${matches[@]}" | sort) \
  || { echo "ERROR: sort failed unexpectedly" >&2; exit 1; }
mapfile -t sorted <<< "$sorted_str"
```

Mined from the `2026-08-20` and `2026-08-23` review logs, which record this shape twice in
four days, in two languages, at two severities — cited by log date and by the shape of each
claim, since both were logged against in-flight branch state (see
[anchor a citation to a symbol, not a line number](../comments-documentation/anchor-a-citation-to-a-symbol-not-a-line-number.md)):

- **KYO-480 (2026-08-23, 🔴, blocked signing; resolved in cycle 2).** The sync-bootstrap
  path reported each entity type's count as `.len()` of a vec that a pre-existing
  `unwrap_or_else(|e| { warn; vec![] })` had already emptied. The delta path's own
  producer was correct — it omitted a failed type from the map — so the two producers
  disagreed about what `0` meant. Because a cache repair re-fetches *every* reconciled
  type, one transient blip during a repair either made the type under repair converge on
  a false "0 = correct", or fabricated a divergence for an untouched, correctly-cached
  type and wiped it, with no retry until reconnect. It reproduced the exact
  "entities silently vanish" symptom the ticket existed to fix, through the repair
  mechanism itself. Fixed by routing both producers through one
  `insert_count_if_present(counts, key, Option<i64>)` helper that omits `None` rather
  than ever recording `0`, with the `Option` derived from the `Result` before the
  degradation.
- **KYO-386 (2026-08-20, 🟡).** `mapfile -t sorted < <(printf … | sort)`, adopted to
  silence a cosmetic shellcheck warning, traded away fail-fast behaviour: the reviewer
  demonstrated it empirically by making the pipeline `exit 17` — the previous form
  aborted the script, the new one printed an empty array and exited 0. The consumer read
  that empty list as "no recent review logs", which is a legitimate quiet-week result.
  The script in question is the standards-mining script, so a swallowed failure recreated
  in miniature the silent no-op that KYO-386 was written to eliminate.

Both fixes have the same shape, and it is not "add a log": make the failure survive the
boundary, then let the consumer omit rather than assume.
