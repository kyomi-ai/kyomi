# Build user-facing error text from `user_message()`, never from `Display`

`kyomi_core::Error`'s `Display` impl is the **log** representation: every variant's
`#[error(...)]` attribute prepends a tag (`"internal: {0}"`, `"bad request: {0}"`,
`"not found: {0}"`, …) so a log line identifies which branch fired without a source
location. `user_message()` is the **user** representation: the inner message alone,
plus a fixed `"internal server error"` for the four `#[error(transparent)]` variants
whose payload is raw sqlx/redis/serde detail. The type's own doc says the two must
never be swapped.

They get swapped anyway, in one direction, over and over: `e.to_string()` (or `{e}`,
or `%e` inside a `format!`) at a site that writes text a person or the model then
reads. The result is a user handed `"internal: connection refused"` or a copilot
retry-guidance string that opens `"bad request: "` — the message is *correct*, and
the leaked tag is pure noise that reads like a bug. It also passes review easily:
`to_string()` on an error is idiomatic everywhere else, and the tag only appears at
runtime, in copy nobody unit-tested.

**Rule:** at any site that persists or returns error text destined for a human or a
model — a tool result, a toast, a `*.error_message` column, a rendered status line —
call `Error::user_message()`. Keep `Display` for `tracing` (`error = %e`) so the
variant survives in the log; the two calls sit side by side in the same `Err` arm and
that is correct, not duplication. When more than one call site in a file needs the
same payload shape, extract one named helper and route all of them through it, so the
next site added to that file cannot reintroduce the leak. Check the fallback arm too:
the error handler *for the error handler* must reuse the already-sanitized string,
not re-derive it from `e`.

```rust
// WRONG — Display's variant tag reaches the stored, user-rendered message
Err(e) => {
    error!(watch_id = %watch_id, error = %e, "Watch execution failed");
    let error_msg = sanitize_null_bytes(&e.to_string()); // "internal: …"
    watch_service::complete_execution(
        db,
        execution_id,
        kyomi_core::WatchExecutionStatus::Error,
        None,
        Some(&error_msg),
    )
    .await
}

// RIGHT — log keeps the tag, the user-facing write site does not
Err(e) => {
    error!(watch_id = %watch_id, error = %e, "Watch execution failed");
    // watch_execution_error_message == sanitize_null_bytes(e.user_message())
    let error_msg = watch_execution_error_message(&e);
    watch_service::complete_execution(
        db,
        execution_id,
        kyomi_core::WatchExecutionStatus::Error,
        None,
        Some(&error_msg),
    )
    .await
}
```

Six instances of this one species have been fixed across four tickets, three of those
tickets inside two days. KYO-350 established `user_message()` and fixed the first three
sites.
KYO-380 (2026-08-20 review log) fixed the persisted `watch_executions.error_message`
write, extracting `watch_execution_error_message` in `crates/kyomi-agent/src/watch_execution.rs`.
KYO-389 (2026-08-21) fixed three call sites in `crates/kyomi-agent/src/tools/copilot.rs`
behind `validation_failure_result`. KYO-397 (2026-08-21) fixed `preview_watch`'s
`parse_schedule` arm behind `schedule_validation_failure` in
`crates/kyomi-agent/src/tools/watch.rs` — the review that landed it noted this was the
sixth occurrence and that "manual review catches are working but a structural fix would
end the recurrence."

Two things make the recurrence cheap to keep repeating, and both are worth knowing
when you touch a new tool or write path. First, each instance is a single expression
in an otherwise-correct `Err` arm, so nothing about the surrounding code looks wrong.
Second, the *sanitizing* wrapper is often already present (`sanitize_null_bytes`,
`sanitize_error`) and reads like the concern is handled — it is not; those strip
different things. Prefer a per-file helper with a name that says what it produces over
inlining `e.user_message()` at each site, so the pattern is visible to the next reader.

Distinct from [never show the user a claim the branch doesn't establish](no-user-facing-claim-the-branch-doesnt-establish.md):
there the text is wrong about the outcome; here the text is right and only an internal
variant prefix leaks.
