// SPDX-License-Identifier: AGPL-3.0-or-later

//! Kyomi-side `DataSourceProvider` for the chartml 5.0 resolver.
//!
//! Wraps the existing `query_datasource_arrow` server function so the
//! chartml resolver can dispatch any chart spec whose `data:` block carries
//! a `datasource` slug + SQL `query` through Kyomi's normal auth + datasource
//! plumbing — matching what the legacy bespoke fetch loop in
//! `markdown_renderer::ChartBlock` used to do, but funneling through the
//! shared resolver so caching, dedup, hooks, and cross-source registration
//! all happen for free.
//!
//! # Wiring
//!
//! Built once at the dashboard root (`DashboardViewerPage` /
//! `DashboardEditorPage`) and provided via Leptos context:
//!
//! ```ignore
//! provide_context(chartml_leptos::ProviderRef::from(
//!     std::sync::Arc::new(KyomiDatasourceProvider::new(workspace_id.clone())),
//! ));
//! ```
//!
//! `chartml_leptos::ChartMLChart` reads the context, registers the provider
//! under the `"datasource"` dispatch key on its inner `ChartML` instance,
//! and the resolver routes every `data: { datasource, query }` shape (and
//! every named-map entry that resolves to that shape) through it.
//!
//! # Cross-workspace isolation
//!
//! Each provider instance carries its `workspace_id` (the workspace UUID).
//! That id is folded into every cache key via the resolver's `namespace`
//! parameter (set on `FetchRequest` by `ChartML::fetch`'s NamedMap walk),
//! so two workspaces sharing a browser cannot see each other's cached query
//! results — same invariant we rely on for `IndexedDbBackend` namespacing.
//!
//! # Testability
//!
//! The actual server-fn call is hidden behind a small [`DatasourceQuerier`]
//! trait so tests can substitute a mock without standing up a Leptos server
//! context (which `query_datasource_arrow` requires). Production callers use
//! [`KyomiDatasourceProvider::new`], which wires the [`ServerFnDatasourceQuerier`]
//! impl that delegates to the real server fn. Tests use
//! [`KyomiDatasourceProvider::with_querier`] to pass a mock that records calls.

use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use chartml_core::data::DataTable;
use chartml_core::{DataSourceProvider, FetchError, FetchRequest, FetchResult};
use chartml_leptos::{HooksRef, ProviderRef};
use leptos::prelude::*;
use leptos::server_fn::ServerFnError;

use crate::server_fns::datasources::{query_datasource_arrow, QueryArrowResult};

/// Abstraction over the `query_datasource_arrow` server function so
/// [`KyomiDatasourceProvider::fetch`] is unit-testable without a Leptos
/// server context.
///
/// The default production impl ([`ServerFnDatasourceQuerier`]) delegates
/// straight through to [`query_datasource_arrow`]; tests substitute a mock
/// via [`KyomiDatasourceProvider::with_querier`] to assert the slug, query,
/// and limit forwarded by the provider.
///
/// `?Send` on WASM matches the `DataSourceProvider` trait — the underlying
/// server-fn future is `!Send` in browser builds.
#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
pub trait DatasourceQuerier: Send + Sync {
    async fn query(
        &self,
        datasource_slug: String,
        sql: String,
        limit: Option<i32>,
    ) -> Result<QueryArrowResult, ServerFnError>;
}

/// Production [`DatasourceQuerier`] impl. Forwards verbatim to
/// [`query_datasource_arrow`] — no logic, just a trait bridge so the provider
/// can be constructed with either the real server fn or a mock.
#[derive(Debug, Default, Clone, Copy)]
pub struct ServerFnDatasourceQuerier;

#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
impl DatasourceQuerier for ServerFnDatasourceQuerier {
    async fn query(
        &self,
        datasource_slug: String,
        sql: String,
        limit: Option<i32>,
    ) -> Result<QueryArrowResult, ServerFnError> {
        query_datasource_arrow(datasource_slug, sql, limit).await
    }
}

/// Shared-ownership alias for the querier behind the provider. `Arc<dyn ...>`
/// because the provider lives behind [`chartml_leptos::ProviderRef`] (also an
/// `Arc<dyn ...>`) and gets cloned every time the resolver dispatches a fetch.
pub type DatasourceQuerierRef = Arc<dyn DatasourceQuerier>;

/// `DataSourceProvider` impl that fans out to Kyomi's datasource server fn.
///
/// Cheap to clone — owns one `String` and one `Arc`. Construction is
/// intentionally cheap so the dashboard root can build it once on mount
/// without coordinating with the async user-context fetch.
///
/// See module docs for wiring.
#[derive(Clone)]
pub struct KyomiDatasourceProvider {
    /// Workspace UUID for the current user. Used to namespace cache entries
    /// across workspaces sharing one browser. Empty string is allowed (the
    /// resolver doesn't fold an empty namespace into anything sensitive),
    /// but downstream `IndexedDbBackend` rejects empty + colon-bearing
    /// namespaces — keep this aligned with whatever the dashboard root
    /// passes to `IndexedDbBackend::new(...)`.
    workspace_id: String,
    /// Indirection over the server-fn call. Production callers get
    /// [`ServerFnDatasourceQuerier`] via [`Self::new`]; tests inject a mock
    /// via [`Self::with_querier`].
    querier: DatasourceQuerierRef,
}

impl std::fmt::Debug for KyomiDatasourceProvider {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Skip `querier` — `dyn DatasourceQuerier` doesn't implement Debug
        // and adding the bound would force every mock impl in tests to also
        // derive it. The workspace id is the only field that varies in logs.
        f.debug_struct("KyomiDatasourceProvider")
            .field("workspace_id", &self.workspace_id)
            .finish_non_exhaustive()
    }
}

impl KyomiDatasourceProvider {
    /// Construct a provider scoped to one workspace, wired to the production
    /// [`ServerFnDatasourceQuerier`]. This is the constructor the dashboard
    /// root uses.
    pub fn new(workspace_id: impl Into<String>) -> Self {
        Self::with_querier(workspace_id, Arc::new(ServerFnDatasourceQuerier))
    }

    /// Construct a provider with a custom [`DatasourceQuerier`]. Tests use
    /// this to inject a mock that records the arguments forwarded by
    /// [`Self::fetch`]; production code should prefer [`Self::new`].
    pub fn with_querier(
        workspace_id: impl Into<String>,
        querier: DatasourceQuerierRef,
    ) -> Self {
        Self {
            workspace_id: workspace_id.into(),
            querier,
        }
    }

    /// Read-only accessor for tests + diagnostics.
    pub fn workspace_id(&self) -> &str {
        &self.workspace_id
    }
}

/// Validate a [`FetchRequest`] and pull out the slug + query the resolver
/// dispatched against. Extracted so the provider's input-validation logic
/// is unit-testable without standing up a fake server fn.
///
/// Returns `Err(FetchError::SlugNotFound)` when `request.spec.datasource`
/// is missing — the slug is empty in that case, which the error message
/// makes explicit. Returns `Err(FetchError::Other)` when the query string
/// is missing.
fn extract_slug_and_query(
    request: &FetchRequest,
) -> Result<(String, String), FetchError> {
    let slug = request.spec.datasource.clone().ok_or_else(|| {
        FetchError::SlugNotFound {
            slug: String::new(),
        }
    })?;
    let query = request.spec.query.clone().ok_or_else(|| {
        FetchError::Other(
            "missing query: KyomiDatasourceProvider requires `query` in the data spec"
                .to_string(),
        )
    })?;
    Ok((slug, query))
}

/// Convert a [`crate::server_fns::datasources::QueryArrowResult`] into a
/// [`FetchResult`]. Extracted so the base64 → IPC → DataTable decode chain
/// is unit-testable in isolation from the (browser-only) server function.
///
/// All errors funnel into `FetchError::DecodeFailed` per the design doc:
/// downstream classification (resolver `on_error` hook, UI fallbacks)
/// distinguishes decode failures from network failures, and lumping them
/// in with `Other` would lose that distinction.
pub(crate) fn build_fetch_result(
    ipc_base64: &str,
    num_rows: usize,
    execution_time_ms: Option<i64>,
) -> Result<FetchResult, FetchError> {
    let ipc_bytes = base64::Engine::decode(
        &base64::engine::general_purpose::STANDARD,
        ipc_base64,
    )
    .map_err(|e| {
        FetchError::DecodeFailed(format!("base64 decode of IPC bytes failed: {e}"))
    })?;

    let data = DataTable::from_ipc_bytes(&ipc_bytes)
        .map_err(|e| FetchError::DecodeFailed(format!("Arrow IPC decode failed: {e}")))?;

    // Surface every datum the server returned. Downstream consumers
    // (`FetchMetadata`, `ResolverHooks`) read these by string key, so
    // picking stable names matters more than picking pretty ones.
    let mut metadata = HashMap::new();
    metadata.insert(
        "rows_returned".to_string(),
        serde_json::Value::from(num_rows),
    );
    if let Some(ms) = execution_time_ms {
        metadata.insert(
            "execution_time_ms".to_string(),
            serde_json::Value::from(ms),
        );
    }

    Ok(FetchResult { data, metadata })
}

// `?Send` on WASM matches the chartml-core trait; the server-fn future
// returned by `query_datasource_arrow` is `!Send` in browser builds.
#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
impl DataSourceProvider for KyomiDatasourceProvider {
    async fn fetch(&self, request: FetchRequest) -> Result<FetchResult, FetchError> {
        // Slug + query validation. The resolver only routes requests that
        // already carry a slug (see `dispatch_provider` in chartml-core's
        // resolver), but we re-validate here so a host that re-registers
        // the provider under a different dispatch key still gets a clear
        // error rather than a panic.
        let (slug, query) = extract_slug_and_query(&request)?;

        // The legacy bespoke path passed `None` for `limit` so the user's
        // SQL is the only thing constraining row count. Preserve that —
        // adding a default cap here would silently truncate dashboards
        // that worked before Phase 6.
        let result = self
            .querier
            .query(slug, query, None)
            .await
            .map_err(|e| FetchError::QueryFailed(e.to_string()))?;

        build_fetch_result(&result.ipc_base64, result.num_rows, result.execution_time_ms)
    }
}

/// IndexedDB database name used by every dashboard. One database per origin
/// (browser scope), namespaced internally per workspace via the IndexedDB
/// backend's `namespace` parameter. Pick a stable string so subsequent page
/// loads attach to the same store rather than creating a fresh database.
pub const KYOMI_CHARTML_CACHE_DB: &str = "kyomi-chartml-cache";

/// Tracing-based [`chartml_core::ResolverHooks`] impl for first-pass
/// observability. Errors flow through `tracing::warn!`; cache hit/miss and
/// progress events are silent for now (they're high-volume and we don't yet
/// have a destination — phase 6.x can wire them to Kyomi's analytics
/// backend or browser-console logging without changing the trait shape).
pub struct TracingHooks;

/// Build a fresh [`HooksRef`] pointing at [`TracingHooks`]. Used by
/// `ChartBlock` to install the hook impl directly on each chart's resolver
/// (rather than via Leptos context — see the docs on
/// [`DashboardChartProviders`] for why).
pub fn tracing_hooks_ref() -> HooksRef {
    #[cfg(not(target_arch = "wasm32"))]
    {
        std::sync::Arc::new(TracingHooks)
    }
    #[cfg(target_arch = "wasm32")]
    {
        std::rc::Rc::new(TracingHooks)
    }
}

#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
impl chartml_core::ResolverHooks for TracingHooks {
    async fn on_error(&self, event: chartml_core::ErrorEvent) {
        // Phase + source name help triage which chart in a multi-chart
        // dashboard hit the failure. The error string itself is the
        // resolver's `FetchError::to_string()` so we don't need to format
        // anything beyond the event fields.
        tracing::warn!(
            phase = ?event.phase,
            source = event.source_name.as_deref().unwrap_or("source"),
            error = %event.error,
            "chartml resolver error",
        );
    }
}

/// Reactive handle to the per-workspace IndexedDB cache backend. `None`
/// while the backend is opening (async `Factory::open`) or after the open
/// failed; `Some` once the database is ready and every chart mounted
/// thereafter picks it up via Leptos context.
///
/// Wrapped in a [`Signal`] (rather than [`Memo`]) because
/// `Arc<dyn CacheBackend>` doesn't implement `PartialEq` — Memo's value
/// caching depends on equality. `Signal::derive_local` reads the underlying
/// `RwSignal` on every access without comparing.
///
/// # Why `LocalStorage`?
///
/// `chartml_leptos::CacheBackendRef` aliases to `Arc<dyn CacheBackend>` on
/// native and `Rc<dyn CacheBackend>` on wasm32. `Rc<T>` is *unconditionally*
/// `!Send + !Sync` regardless of `T`, so the default `SyncStorage` (which
/// requires `T: Send + Sync` for the contained value's accessors) refuses
/// to wrap it on browser builds — every `RwSignal::set` / `Signal::get`
/// site fails to compile with `Rc<(dyn CacheBackend + 'static)> cannot be
/// sent between threads safely`.
///
/// `LocalStorage` lifts that restriction by storing the value inside a
/// [`send_wrapper::SendWrapper`] that pretends to be `Send + Sync` but
/// panics if accessed from any thread other than the one that created it.
/// Sound for our use case because wasm32-unknown-unknown is single-threaded
/// (browser charts never cross workers), and we lose nothing — the
/// `Signal::get()` API is identical regardless of storage.
///
/// On native targets (server-side render) this is always `None` —
/// `IndexedDbBackend` is browser-only — but we still pay the `LocalStorage`
/// machinery there for one reactive type alias across both targets.
pub type CacheBackendSignal =
    Signal<Option<chartml_leptos::CacheBackendRef>, LocalStorage>;

/// Dashboard-wide "refresh all charts" signal. The dashboard viewer's
/// "Refresh All" toolbar button increments this; every descendant
/// `ChartMLChart` (via the `ChartBlock` wrapper in `markdown_renderer`)
/// folds it into the `refresh_trigger` prop on `chartml_leptos::ChartMLChart`,
/// which invalidates each spec source's resolver cache key and re-runs the
/// fetch + transform + render pipeline against the current YAML.
///
/// Provided via Leptos context at `DashboardViewerPage` (above
/// `DashboardChartProviders` so the toolbar button — which sits as a
/// sibling of the providers, not a child — can share the same signal).
/// Surfaces in `ChartBlock` via `use_context::<RefreshAllSignal>()`, which
/// returns `None` in contexts that don't provide it (the dashboard editor's
/// live preview, the chart-builder preview, etc.); per-chart refresh works
/// in those contexts via the chart's own header-bar refresh button.
pub type RefreshAllSignal = RwSignal<u32>;

/// Wrapper component that wires the chartml 5.0 provider, persistent cache
/// backend, and observability hooks into Leptos context for every descendant
/// `chartml_leptos::ChartMLChart`.
///
/// Mounted around `MarkdownRenderer` in both the dashboard viewer and editor.
/// Construction is deferred until the workspace id is known (loaded from
/// the user-context resource) so the IndexedDB backend can namespace cache
/// entries per workspace and `KyomiDatasourceProvider` can fold the id
/// into resolver cache keys for cross-workspace isolation.
///
/// # Why a wrapper component?
///
/// `provide_context` resolves at the component-construction site and
/// affects every descendant. Doing it here (a child of the dashboard's
/// async user-context resolution) means the provider and cache are only
/// installed once we have a real `workspace_id` — never with a placeholder
/// that would either fail (`IndexedDbBackend::new` rejects empty namespaces)
/// or leak data across workspaces.
///
/// # Cache backend
///
/// The IndexedDB tier-2 cache opens asynchronously via `IndexedDbBackend::new`.
/// While it opens, the resolver still has the in-memory tier-1 cache
/// (`MemoryBackend`) so charts render immediately; the persistent cache
/// hydrates as soon as `Factory::open` resolves. If the open fails (private
/// browsing disabled IDB, namespace constraint violated), we log a warning
/// and proceed without a tier-2 cache — degraded but not broken.
#[component]
pub fn DashboardChartProviders(
    /// Workspace UUID — folded into every cache key for cross-workspace
    /// isolation. Must be non-empty and must not contain `:` (the IndexedDB
    /// namespace separator). See [`KyomiDatasourceProvider::new`] and
    /// [`chartml_core::resolver::backends::indexeddb::IndexedDbBackend::new`].
    #[prop(into)]
    workspace_id: String,
    /// `ChildrenFn` (not `Children`) so the wrapper plays nicely with
    /// reactive parent closures that need to be `FnMut` — `Children` is
    /// `FnOnce`, which forces the parent to move every captured variable
    /// into the children body, which in turn forces the parent to also be
    /// `FnOnce`. `ChildrenFn` is `Fn`, so the body can be called many times
    /// (the dashboard re-renders the preview on every editor keystroke).
    children: ChildrenFn,
) -> impl IntoView {
    // Provider — synchronous, cheap, ready immediately.
    let provider: ProviderRef = Arc::new(KyomiDatasourceProvider::new(workspace_id.clone()));
    provide_context(provider);

    // Hooks — `HooksRef` is `Rc<dyn ResolverHooks>` on WASM and therefore
    // `!Send + !Sync`, which makes it incompatible with `provide_context`
    // (Leptos requires `Send + Sync`). Instead of plumbing them through the
    // context the way the provider is plumbed, we expose a constructor on
    // [`KyomiResolverHooks`] and wire them per-chart via the
    // `ChartMLChart`'s `hooks` prop or via the `resolver().set_hooks(...)`
    // call inside `ChartBlock::new`. This keeps the dashboard-root API
    // tier-2-cache-only — same shape as the cache backend signal.

    // IndexedDB cache backend — opens asynchronously, hydrates a reactive
    // signal that descendants can read via Leptos context. The signal is
    // provided immediately (so the context entry exists at child mount) and
    // populated when the open completes. `ChartMLChart` re-resolves its
    // cache_backend prop on every mount, so the persistent cache becomes
    // active as soon as charts re-mount after open completion (e.g. on
    // refresh-button press, navigation back, or any reactive prop change
    // that re-mounts the inner ChartMLChart instance).
    let backend_signal = open_indexeddb_backend(&workspace_id);
    provide_context(backend_signal);

    children()
}

/// Spawn the IndexedDB open and return a [`CacheBackendSignal`] that flips
/// to `Some` when the open succeeds, or stays `None` if it fails.
/// Browser-only — native targets immediately return `None`.
///
/// Returns a `Signal` rather than a `Memo` because `Arc<dyn CacheBackend>`
/// doesn't implement `PartialEq` (Memo's diff-and-skip optimization needs
/// equality). `Signal::derive_local` reads the underlying `RwSignal`
/// unconditionally, which is fine here — the signal flips at most once
/// per workspace.
///
/// # `LocalStorage` everywhere
///
/// Both the inner [`RwSignal`] and the returned [`Signal`] use
/// [`LocalStorage`]. The reason: `chartml_leptos::CacheBackendRef` is
/// `Rc<dyn CacheBackend>` on wasm32 (`Rc` is unconditionally `!Send +
/// !Sync` regardless of `T`), so the default `SyncStorage` rejects it at
/// compile time. `LocalStorage` wraps the value in a [`send_wrapper`] that
/// makes it nominally `Send + Sync` but panics on cross-thread access —
/// which can't happen because wasm32-unknown-unknown is single-threaded.
/// See the [`CacheBackendSignal`] doc comment for the long form.
fn open_indexeddb_backend(workspace_id: &str) -> CacheBackendSignal {
    let backend_state: RwSignal<Option<chartml_leptos::CacheBackendRef>, LocalStorage> =
        RwSignal::new_local(None);

    #[cfg(target_arch = "wasm32")]
    {
        let id = workspace_id.to_string();
        leptos::task::spawn_local(async move {
            use chartml_core::resolver::backends::indexeddb::IndexedDbBackend;
            match IndexedDbBackend::new(KYOMI_CHARTML_CACHE_DB, &id).await {
                Ok(backend) => {
                    let backend: chartml_leptos::CacheBackendRef =
                        std::rc::Rc::new(backend);
                    backend_state.set(Some(backend));
                }
                Err(e) => {
                    tracing::warn!(
                        error = %e,
                        workspace = %id,
                        "IndexedDB cache backend unavailable, falling back to in-memory tier-1 only",
                    );
                }
            }
        });
    }

    // Suppress unused-variable warning on native targets where we don't
    // actually consume `workspace_id`.
    #[cfg(not(target_arch = "wasm32"))]
    let _ = workspace_id;

    Signal::derive_local(move || backend_state.get())
}

#[cfg(test)]
mod tests {
    use super::*;
    use base64::Engine as _;
    use chartml_core::data::Row;

    // ── Constructor / accessor ─────────────────────────────────────────────

    #[test]
    fn provider_stores_workspace_id() {
        let p = KyomiDatasourceProvider::new("acme-corp");
        assert_eq!(p.workspace_id(), "acme-corp");
    }

    #[test]
    fn provider_clone_shares_id() {
        let a = KyomiDatasourceProvider::new("acme");
        let b = a.clone();
        assert_eq!(a.workspace_id(), b.workspace_id());
    }

    #[test]
    fn cache_db_name_is_stable() {
        // Stability matters for IndexedDB attachment across page loads —
        // changing this string would orphan every previously-cached entry
        // in user browsers. Pinned via test so renames go through review.
        assert_eq!(KYOMI_CHARTML_CACHE_DB, "kyomi-chartml-cache");
    }

    // ── Decode pipeline — base64 → Arrow IPC → DataTable ───────────────────
    //
    // Lives in `#[cfg(test)] mod tests` (not in `tests/`) so
    // `build_fetch_result` can stay `pub(crate)` — it's an implementation
    // detail of the provider, not part of the crate's public API.

    /// Build a `DataTable` of two rows {x: "A", y: 1} / {x: "B", y: 2} and
    /// serialize it to base64-encoded Arrow IPC bytes — matching the wire
    /// format `query_datasource_arrow` returns.
    fn known_table_ipc_b64() -> String {
        let rows: Vec<Row> = vec![
            [
                ("x".to_string(), serde_json::json!("A")),
                ("y".to_string(), serde_json::json!(1)),
            ]
            .into_iter()
            .collect(),
            [
                ("x".to_string(), serde_json::json!("B")),
                ("y".to_string(), serde_json::json!(2)),
            ]
            .into_iter()
            .collect(),
        ];
        let table = DataTable::from_rows(&rows).expect("from_rows must succeed for valid rows");
        let bytes = table.to_ipc_bytes().expect("to_ipc_bytes must succeed");
        base64::engine::general_purpose::STANDARD.encode(&bytes)
    }

    #[test]
    fn build_fetch_result_decodes_known_ipc_bytes() {
        let ipc_b64 = known_table_ipc_b64();
        let result = build_fetch_result(&ipc_b64, 2, Some(123)).expect("decode must succeed");

        // Schema: `x` (Utf8) and `y` (Float64) — both columns surface.
        assert_eq!(result.data.num_rows(), 2, "row count survives the round trip");
        let schema = result.data.schema();
        let names: Vec<&str> = schema.fields().iter().map(|f| f.name().as_str()).collect();
        assert!(names.contains(&"x"), "x column survives: {names:?}");
        assert!(names.contains(&"y"), "y column survives: {names:?}");

        // Metadata: `rows_returned` mirrors the server's count, and
        // `execution_time_ms` is propagated so downstream consumers (telemetry,
        // resolver hooks) can see it.
        assert_eq!(
            result.metadata.get("rows_returned"),
            Some(&serde_json::Value::from(2usize)),
            "rows_returned metadata must round-trip",
        );
        assert_eq!(
            result.metadata.get("execution_time_ms"),
            Some(&serde_json::Value::from(123i64)),
            "execution_time_ms metadata must propagate when present",
        );
    }

    #[test]
    fn build_fetch_result_omits_execution_time_when_none() {
        let ipc_b64 = known_table_ipc_b64();
        let result =
            build_fetch_result(&ipc_b64, 2, None).expect("decode must succeed");

        // Absent execution_time_ms = absent metadata key (not a null value).
        // Telemetry consumers can then distinguish "we got 0ms" from "the
        // server didn't measure timing on this one".
        assert!(
            !result.metadata.contains_key("execution_time_ms"),
            "execution_time_ms key must be omitted when source had no timing",
        );
        // rows_returned still present.
        assert!(result.metadata.contains_key("rows_returned"));
    }

    #[test]
    fn build_fetch_result_corrupt_base64_returns_decode_failed() {
        // Anything that isn't valid base64 → `DecodeFailed`, NOT `Other`. This
        // matters for downstream error classification (resolver hooks, UI
        // fallbacks) which dispatch on the error variant.
        let err = build_fetch_result("not-base64-!!!", 0, None)
            .expect_err("corrupt base64 must error");
        assert!(
            matches!(err, FetchError::DecodeFailed(_)),
            "expected DecodeFailed for invalid base64, got: {err:?}",
        );
    }

    #[test]
    fn build_fetch_result_corrupt_ipc_returns_decode_failed() {
        // Valid base64 but garbage IPC payload → still `DecodeFailed` (the
        // `from_ipc_bytes` call surfaces the underlying Arrow error message
        // wrapped in our variant).
        let garbage = base64::engine::general_purpose::STANDARD.encode(b"not-arrow-ipc");
        let err = build_fetch_result(&garbage, 0, None)
            .expect_err("garbage IPC must error");
        assert!(
            matches!(err, FetchError::DecodeFailed(_)),
            "expected DecodeFailed for invalid IPC, got: {err:?}",
        );
    }
}
