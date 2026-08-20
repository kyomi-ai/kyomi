# Never reimplement server-owned URL/routing logic on the client

OAuth connect URLs, callback URLs, and API endpoint paths are owned by the server. Client-side functions that reconstruct these URLs by pattern-matching on provider strings duplicate logic that `get_oauth_connect_url` (or equivalent server_fn) already handles correctly — and inevitably miss edge cases (e.g. BigQuery `enterprise_oauth` needing a different endpoint than `kyomi_oauth`).

**Rule:** Call the server_fn that owns the URL logic. If no server_fn exists for the use case, create one. Never build API URLs client-side from provider/mode strings.

```rust
// WRONG — client reimplements URL routing, misses enterprise_oauth branch
fn oauth_url_for_datasource(provider: &str, slug: &str) -> String {
    match provider {
        "bigquery" => "/api/v1/auth/google-oauth/connect".to_string(),
        // missing: enterprise_oauth needs /api/v1/auth/oauth/bigquery-enterprise/connect?datasource_slug=...
        "snowflake" => format!("/api/v1/auth/oauth/snowflake/connect?datasource_slug={slug}"),
        _ => String::new(),
    }
}

// RIGHT — server_fn owns the routing
let url = get_oauth_connect_url(provider.clone(), slug.clone(), auth_mode).await?;
```

Flagged in KYO-12 review — `oauth_url_for_datasource` silently routed enterprise BigQuery users to the wrong OAuth endpoint.
