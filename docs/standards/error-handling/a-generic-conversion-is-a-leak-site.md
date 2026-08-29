# A generic conversion is a leak site — audit the `From` impls and blanket impls, not just the call sites

[Build user-facing error text from `user_message()`](user-message-not-display-for-user-facing-text.md)
is a *per-call-site* discipline: find `e.to_string()` in an `Err` arm that writes text a
human will read, swap it. That technique is structurally blind to two shapes that carry
the identical defect, and both have now been found sitting behind sweeps that were
otherwise complete.

- **A `From` impl that stringifies.** `impl From<kyomi_connect_protocol::Error> for Error`
  (`crates/kyomi-core/src/error.rs`) builds `Error::Internal(e.to_string())`. The foreign
  type's own `Display` tag — `"connection failed: "`, `"not supported: "` — is baked into
  the payload at **construction**. A later `user_message()` strips only the outer
  `"internal: "` and hands the user the inner tag anyway. Nothing at the output boundary
  can undo this; the conversion is the only place it is fixable.
- **A blanket impl at the output boundary.** `IntoServerFnError::into_sfn()`
  (`crates/kyomi-ui/src/server_fns/mod.rs`) is `impl<T, E: std::fmt::Display>`, so it calls
  `e.to_string()` for every error type it is handed, `kyomi_core::Error` included. A sweep
  that correctly fixed every hand-rolled `.map_err` closure in `server_fns/datasources.rs`
  left tagged errors still leaving that same file through `.into_sfn()` — the file the
  ticket's own acceptance criterion named.

What makes both invisible is that the leaking expression exists exactly once, in a file the
ticket never mentions, and the sites that actually leak contain no `to_string()` to grep
for. The bound is the tell: `E: Display` on a conversion that produces user-visible text is
a standing declaration that whatever you pass will be rendered in its **log**
representation. A third shade is worth knowing because it looks safe and is only
conditionally so — `bigquery_projects_discovery_result` in `server_fns/datasources.rs` calls
`to_string()` on a `kyomi_connect_protocol::Error` and is prefix-free only because
BigQuery's `list_projects()` override (delegating to `list_active_projects()`) only ever
constructs `Error::Internal(…)`; the trait's *default* `list_projects()` returns the tagged
`NotSupported(…)`. Correct today, by a fact about which impl is selected rather than about
the type.

**Rule:** when auditing an error-text leak — or any "this value reaches a person" class —
enumerate the *conversions* on the path as well as the call sites. Grep for `impl From<`
into the error type and for blanket impls bounded on `Display`/`ToString`, and say in the
PR which ones you checked. Never build an error payload out of a foreign error's `Display`:
match its variants and map each to clean text, so the tag is never in the payload to begin
with. A generic boundary that must render an error needs a bound that can only produce
user-safe text, or a specialisation — and if Rust's coherence rules block that (they do
here), say so and ticket it rather than letting a green per-file sweep imply the file is
clean.

```rust
// WRONG — the foreign Display tag is baked into the payload at construction.
// user_message() downstream strips "internal: " and still yields
// "connection failed: host unreachable".
impl From<kyomi_connect_protocol::Error> for Error {
    fn from(e: kyomi_connect_protocol::Error) -> Self {
        Error::Internal(e.to_string())
    }
}

// RIGHT — map the foreign variants, so no tag ever enters the payload.
// Exhaustive, no wildcard arm: a new upstream variant becomes a compile error
// rather than a silent re-leak.
impl From<kyomi_connect_protocol::Error> for Error {
    fn from(e: kyomi_connect_protocol::Error) -> Self {
        use kyomi_connect_protocol::Error as P;
        match e {
            P::Provider(m) | P::Internal(m) => Error::Internal(m),
            P::Connection(m) => Error::Internal(m),
            P::NotSupported(m) => Error::BadRequest(m),
            P::SerdeJson(e) => Error::Internal(e.to_string()),
        }
    }
}
```

Two review entries, two days, two different mechanisms, both on KYO-448:

- **2026-08-24, `09:15` entry** (🟢, "Other: latent double-tag via blanket `From`
  conversion") — the `From<kyomi_connect_protocol::Error>` case above. The reviewer
  confirmed it dormant today (`Error::Connection` is never constructed in `kyomi-connect`,
  and `Error::NotSupported` only fires from the driver factory when a provider feature is
  compiled out, which kyomi's `features = ["all"]` never does) and still flagged it, because
  the leak is latent in the type, not in any call site anyone will audit.
- **2026-08-25, `13:32` entry** (🟡, deferred with ticket) — the `into_sfn()` blanket impl.
  The diff under review had just swept the whole of `server_fns/` and the reviewer's own
  verification found `list_datasources_with_status` and `delete_datasource` still routing
  `kyomi_core::Result<T>` through `.into_sfn()` unchanged. Deferred only because KYO-523
  already existed, `agent-ready` and linked both directions, and explains why the fix is not
  small: hundreds of call sites across dozens of files, and Rust's coherence rules forbid a
  second blanket impl specialised on `kyomi_core::Error`.

The standing recommendation predates both. The KYO-397 review (2026-08-21, `04:44`), landing
the sixth per-call-site fix of this species, wrote: *"manual review catches are working but a
structural fix would end the recurrence."* The two findings above are what "structural" turned
out to mean.

Sibling of [user-message-not-display-for-user-facing-text.md](user-message-not-display-for-user-facing-text.md):
that rule is what to write at a call site; this one is why a file full of correct call sites
can still leak. See also
[enumerate-consumers-from-the-type-not-from-the-diff.md](../code-organization/enumerate-consumers-from-the-type-not-from-the-diff.md)
for the same "grep the producer, not the expression" move applied to renames.
