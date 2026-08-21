# Never reference `kyomi-core` from client-side code — it is an `ssr`-only dependency

`kyomi-ui` declares `kyomi-core` as `dep:kyomi-core` under the `ssr` feature only (`crates/kyomi-ui/Cargo.toml`); the `hydrate` feature does not enable it. Inside a `#[server]` function body this is fine — the body is stripped out of non-ssr builds, so the client never needs the crate. But any code that actually compiles to WASM (a `Memo`/`Signal::derive` closure, a `view!` closure, an event handler, a plain non-`#[server]` helper) that names `kyomi_core::` fails with `E0433: failed to resolve: use of undeclared crate or module` on the `wasm32-unknown-unknown` + `hydrate` build.

This is invisible locally: `cargo check -p kyomi-ui --features ssr` passes, so the break only surfaces in CI or on the trunk build.

**Rule:** `kyomi_core::` may only appear inside `#[server]` function bodies or `#[cfg(feature = "ssr")]` blocks. If client-side code appears to need a `kyomi-core` constant or enum, either restructure the expression so the value isn't needed, or compute the derived value server-side and put a plain type (`bool`, `String`) on the wire DTO. Never widen the `hydrate` feature to pull `kyomi-core` in.

```rust
// WRONG — memo runs on the client; E0433 on wasm32 + hydrate
let seat_capped = Memo::new(move |_| {
    subscription.get()
        .map(|info| info.user_limit.unwrap_or(UNLIMITED_USER_LIMIT) <= 1)
        .unwrap_or(false)
});

// RIGHT — same semantics, no server-only dependency on the client
let seat_capped = Memo::new(move |_| {
    subscription.get()
        .map(|info| info.user_limit.is_some_and(|limit| limit <= 1))
        .unwrap_or(false)
});

// ALSO RIGHT — typed enum stays server-side, client gets a plain bool on the DTO
#[server]
pub async fn catalog_stats() -> Result<CatalogStatsResult, ServerFnError> {
    let row: RefreshRow = /* decodes kyomi_core::enums::CatalogRefreshStatus */;
    Ok(CatalogStatsResult {
        refresh_failed: row.catalog_refresh_status == Some(CatalogRefreshStatus::Failed),
    })
}
```

Verify with `cargo check --target wasm32-unknown-unknown -p kyomi-ui --features hydrate` before pushing. Flagged in KYO-167 (cycle 3 — a `seat_capped` memo referencing `kyomi_core::capability::UNLIMITED_USER_LIMIT` broke CI) and re-checked as an explicit sign-off condition in KYO-169; the same constraint shaped the KYO-126 fix, where the typed `CatalogRefreshStatus` was kept out of the wire struct in favour of a server-computed `bool`.
