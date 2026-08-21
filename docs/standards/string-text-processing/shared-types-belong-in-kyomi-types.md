# Types that cross the client/server boundary belong in `kyomi-types`

`kyomi-ui` compiles to `wasm32` and can only depend unconditionally on `kyomi-types` — `kyomi-core` and `kyomi-auth` are `ssr`-gated optional dependencies, unavailable to the WASM client. When a server_fn's request or response type needs to exist on both sides, defining it in `kyomi-ui` and hand-writing a `From` conversion from the "real" server type is the failure mode this rule prevents: it silently forks the wire contract, and the two copies drift the moment one side gains a field.

**Rule:** Define the type once in `kyomi-types`. Have the owning server crate (`kyomi-auth`, `kyomi-core`) `pub use` it so existing server-side call sites don't churn, and import it directly in `kyomi-ui` — no local redeclaration, no `From` impl.

```rust
// WRONG — kyomi-ui redeclares the server type and hand-converts
// crates/kyomi-ui/src/server_fns/datasource_oauth.rs
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct GoogleProject { pub project_id: String, pub name: String }

#[cfg(feature = "ssr")]
impl From<kyomi_auth::google_oauth::GoogleProject> for GoogleProject {
    fn from(p: kyomi_auth::google_oauth::GoogleProject) -> Self {
        Self { project_id: p.project_id, name: p.name }
    }
}

// RIGHT — one definition, re-exported on both sides
// crates/kyomi-types/src/datasource_contracts.rs
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct GoogleProject { pub project_id: String, pub name: String }

// crates/kyomi-auth/src/google_oauth.rs
pub use kyomi_types::GoogleProject;

// crates/kyomi-ui/src/server_fns/datasource_oauth.rs
pub use kyomi_types::GoogleProject;
```

The one legitimate exception is a server type that carries server-only derives or dependencies (e.g. `sqlx::FromRow`) — a DB row can't be relocated to a dependency-free crate. In that case, split a plain wire DTO in `kyomi-types` from the DB model, rather than moving the DB model itself.

A same-named type in two crates is not automatically a duplicate of this kind — check the fields and purpose before merging. `kyomi_core::models::QueryCache` is a `sqlx::FromRow` DB row for the `query_cache` table; `kyomi_ui::query_cache::QueryCache` is an unrelated Leptos reactive cache handle. Sharing a name is a naming collision, not a shadow type.

Flagged in KYO-196 review: `GeneratedSshKey`, `GoogleProject`, `GoogleOAuthProjectsResult`, `GoogleOAuthDisconnectResult`, and `DatasourceOAuthDisconnectResult` were each independently redeclared in `kyomi-ui` with a matching `From` impl in `kyomi-auth`, purely to satisfy the wasm32 dependency boundary.
