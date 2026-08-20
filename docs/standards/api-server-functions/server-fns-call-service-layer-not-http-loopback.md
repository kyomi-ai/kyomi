# Server functions must call service-layer functions directly — never HTTP loopback

A `#[server]` function runs inside the same process as the axum route handlers. Calling an HTTP endpoint from a server fn (e.g. `reqwest::get("http://localhost:PORT/api/v1/...")`) is a loopback anti-pattern that:
- Creates fragile port/path coupling (wrong prefix = silent 404)
- Bypasses service-layer error typing (HTTP responses must be re-parsed)
- Adds latency and failure modes (TCP connection to self)
- Diverges from every other server fn in the codebase

**Rule:** Extract shared logic into a service function in the appropriate crate (`kyomi-auth`, `kyomi-core`, etc.). Both the REST route handler and the server fn call the same service function directly. If the service function doesn't exist yet, create it — don't shortcut through HTTP.

```rust
// WRONG — HTTP loopback from server fn to own process
#[server]
pub async fn link_google_account(code: String) -> Result<(), ServerFnError> {
    let resp = reqwest::get(format!("http://localhost:8001/api/v1/auth/google/link-callback?code={code}"))
        .await?;
    // fragile: wrong path = 404, must parse HTTP response, port coupling
}

// RIGHT — both callers use the same service function
#[server]
pub async fn link_google_account(code: String) -> Result<LinkResult, ServerFnError> {
    let pool = extract_pool().await?;
    let result = google_link_callback_service(&pool, &params).await
        .map_err(|e| ServerFnError::new(e.to_string()))?;
    Ok(result)
}
```
