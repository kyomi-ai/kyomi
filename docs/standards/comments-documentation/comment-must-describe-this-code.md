# A comment must describe this code — never mirror another file

A comment of the form `Mirrors POST /x in apps/server/src/routes/y.rs` states a fact about a *different* file. It rots the instant that file changes, and it rots silently when the file is deleted: nothing type-checks a path inside a `///`. It also carries no information a reader of *this* function needs.

**Rule:** Write what the function does, enforces, or returns. If parity with another implementation is genuinely load-bearing, say what the shared property *is* rather than pointing at a path — and only when the other file is still live. When a comment's only content is a comparison, deleting it loses nothing.

```rust
// WRONG — claims a fact about a file deleted in 13e957e1
/// Mirrors POST /api/v1/auth/logout in apps/server/src/routes/auth.rs.
pub async fn logout() -> Result<(), ServerFnError> { /* ... */ }

// RIGHT — states what this function actually does
/// Steps:
/// 1. Revoke the refresh-token family.
/// 2. Clear the session cookies.
/// 3. Return regardless of whether a session existed.
pub async fn logout() -> Result<(), ServerFnError> { /* ... */ }
```

Flagged across all three groups of KYO-239 (+82/-131 of pure comment removal across 18 files, all pointing at routes deleted in `13e957e1`/`60e6f56c`/`0f9390b5`). KYO-302 exists because the informal `matches the REST handler` phrasing escapes that sweep's audit regex.
