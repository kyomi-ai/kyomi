// SPDX-License-Identifier: AGPL-3.0-or-later

//! Kyomi-side `DataSourceProvider` for the chartml 5.0 resolver.
//!
//! Routes any chart spec whose `data:` block carries a `datasource` slug +
//! SQL `query` through Kyomi's Arrow streaming endpoint
//! (`POST /api/v1/query-arrow` via [`crate::arrow_fetch::fetch_arrow_stream`]),
//! matching what the legacy bespoke fetch loop in
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
//! The actual browser fetch is hidden behind a small [`DatasourceQuerier`]
//! trait so tests can substitute a mock that returns a `DataTable` directly
//! without standing up browser APIs. Production callers use
//! [`KyomiDatasourceProvider::new`], which wires the [`ServerFnDatasourceQuerier`]
//! impl that delegates to [`crate::arrow_fetch::fetch_arrow_stream`]. Tests use
//! [`KyomiDatasourceProvider::with_querier`] to pass a mock that records calls.

use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use chartml_chart_cartesian::CartesianRenderer;
use chartml_chart_metric::MetricRenderer;
use chartml_chart_pie::PieRenderer;
use chartml_chart_scatter::ScatterRenderer;
use chartml_chart_table::TableRenderer;
use chartml_core::data::DataTable;
use chartml_core::{DataSourceProvider, FetchError, FetchRequest, FetchResult};
use chartml_datafusion::DataFusionTransform;
use chartml_leptos::{use_chartml_configured, ChartMLRef, HooksRef, ProviderRef};
use leptos::prelude::*;

/// Abstraction over the Arrow fetch path so [`KyomiDatasourceProvider::fetch`]
/// is unit-testable without standing up browser APIs.
///
/// The default production impl ([`ServerFnDatasourceQuerier`]) delegates
/// straight through to [`crate::arrow_fetch::fetch_arrow_stream`]; tests
/// substitute a mock via [`KyomiDatasourceProvider::with_querier`] to assert
/// the slug and query forwarded by the provider.
///
/// `?Send` on WASM matches the `DataSourceProvider` trait — the underlying
/// fetch future is `!Send` in browser builds.
#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
pub trait DatasourceQuerier: Send + Sync {
    async fn query(
        &self,
        datasource_slug: String,
        sql: String,
    ) -> Result<DataTable, String>;
}

/// Production [`DatasourceQuerier`] impl. Delegates to
/// [`crate::arrow_fetch::fetch_arrow_stream`] on WASM. On native targets
/// (SSR) this path is never exercised — charts always run in the browser —
/// so we return an error rather than panicking.
#[derive(Debug, Default, Clone, Copy)]
pub struct ServerFnDatasourceQuerier;

#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
impl DatasourceQuerier for ServerFnDatasourceQuerier {
    async fn query(
        &self,
        datasource_slug: String,
        sql: String,
    ) -> Result<DataTable, String> {
        #[cfg(target_arch = "wasm32")]
        {
            crate::arrow_fetch::fetch_arrow_stream(&datasource_slug, &sql).await
        }
        #[cfg(not(target_arch = "wasm32"))]
        {
            let _ = (datasource_slug, sql);
            Err("fetch_arrow_stream is browser-only; SSR does not execute chartml queries".to_string())
        }
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
    /// but `enable_indexeddb_cache` rejects empty + colon-bearing namespaces.
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

// `?Send` on WASM matches the chartml-core trait; the `fetch_arrow_stream`
// future is `!Send` in browser builds.
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

        let data = self
            .querier
            .query(slug, query)
            .await
            .map_err(FetchError::QueryFailed)?;

        let num_rows = data.num_rows();
        let mut metadata = HashMap::new();
        metadata.insert(
            "rows_returned".to_string(),
            serde_json::Value::from(num_rows),
        );

        Ok(FetchResult { data, metadata })
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

/// Create a fully configured [`ChartMLRef`] with all Kyomi chart renderers
/// registered, the named palette applied, the Kyomi editorial theme wired in,
/// tracing-based resolver hooks installed, and (on WASM) the IndexedDB
/// persistent cache enabled for the given workspace.
///
/// This is the **single shared factory** used by both the markdown-renderer
/// path (`ChartBlock` in `markdown_renderer.rs`) and the WYSIWYG extension
/// path (`ChartMLExtension` in `chartml_extension.rs`). All renderer
/// registrations, palette choices, hook installation, and cache setup happen
/// here so the two render paths are guaranteed to be in sync.
///
/// # Arguments
///
/// * `palette_name` — Kyomi palette name (e.g. `"kyomi"`). Passed to
///   `kyomi_palette(palette_name, is_dark)`. Use `"kyomi"` as the default
///   when no user preference exists.
/// * `is_dark` — selects dark-mode palette slots and chrome colors. Read
///   from `use_theme()` at the construction site.
/// * `workspace_id` — workspace UUID used to namespace the IndexedDB cache
///   entries per workspace. Pass an empty string to skip cache setup.
///
/// # What is configured here
///
/// 1. **Renderers**: `bar`, `line`, `area` (Cartesian); `pie`, `donut`,
///    `doughnut` (Pie); `scatter` (Scatter); `metric` (Metric); `table` (Table).
/// 2. **Transform**: [`DataFusionTransform`] for `transform:` pipeline steps.
/// 3. **Palette**: [`kyomi_chart_theme::kyomi_palette`] — per-mode color slots.
/// 4. **Theme**: [`kyomi_chart_theme::kyomi_theme`] — Kyomi editorial chrome.
/// 5. **Hooks**: [`tracing_hooks_ref`] installed on the resolver so every
///    fetch/transform phase is observable via `tracing::`.
/// 6. **Persistent cache** (WASM only, when `workspace_id` is non-empty):
///    [`chartml_core::ChartML::enable_indexeddb_cache`] opens the
///    [`KYOMI_CHARTML_CACHE_DB`] IndexedDB store namespaced by workspace.
///
/// # What is NOT configured here
///
/// Provider (`ProviderRef`) is pulled from Leptos context inside
/// `chartml_leptos::ChartMLChart`, so it is not part of this factory.
/// Hooks are installed here (point 5) because `HooksRef` is
/// `Rc<dyn ResolverHooks>` on wasm32 — it is `!Send + !Sync` and cannot
/// travel through Leptos context.
pub(crate) fn configured_chartml(
    palette_name: &str,
    is_dark: bool,
    workspace_id: &str,
) -> ChartMLRef {
    let colors = kyomi_chart_theme::kyomi_palette(palette_name, is_dark);
    let theme = kyomi_chart_theme::kyomi_theme(is_dark);
    let chartml = use_chartml_configured(|c| {
        c.register_renderer("bar", CartesianRenderer::new());
        c.register_renderer("line", CartesianRenderer::new());
        c.register_renderer("area", CartesianRenderer::new());
        c.register_renderer("pie", PieRenderer::new());
        c.register_renderer("donut", PieRenderer::new());
        c.register_renderer("doughnut", PieRenderer::new());
        c.register_renderer("scatter", ScatterRenderer::new());
        c.register_renderer("metric", MetricRenderer::new());
        c.register_renderer("table", TableRenderer::new());
        c.register_transform(DataFusionTransform);
        c.set_default_palette(colors);
        c.set_theme(theme);
    });
    // Install tracing hooks on the resolver directly — `HooksRef` is
    // `Rc<dyn ResolverHooks>` on wasm32 (`!Send + !Sync`), so it cannot
    // travel through `provide_context`. Allocating a fresh unit-struct
    // `Rc` here is cheap (one allocation per chart construction).
    chartml.resolver().set_hooks(tracing_hooks_ref());
    // Enable the IndexedDB tier-2 cache when a workspace id is provided.
    // The `#[cfg]` gate matches the method's own gate so we don't attempt to
    // call a browser-only API in SSR builds. Non-WASM builds suppress the
    // unused-variable warning via the else branch below.
    #[cfg(target_arch = "wasm32")]
    if !workspace_id.is_empty() {
        chartml.enable_indexeddb_cache(KYOMI_CHARTML_CACHE_DB, workspace_id);
    }
    #[cfg(not(target_arch = "wasm32"))]
    let _ = workspace_id;
    chartml
}

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

/// Wrapper component that wires the chartml 5.0 provider into Leptos context
/// for every descendant `chartml_leptos::ChartMLChart`.
///
/// Mounted around `MarkdownRenderer` in both the dashboard viewer and editor.
/// Construction is deferred until the workspace id is known (loaded from
/// the user-context resource) so `KyomiDatasourceProvider` can fold the id
/// into resolver cache keys for cross-workspace isolation.
///
/// # Why a wrapper component?
///
/// `provide_context` resolves at the component-construction site and
/// affects every descendant. Doing it here (a child of the dashboard's
/// async user-context resolution) means the provider is only installed once
/// we have a real `workspace_id` — never with a placeholder that would
/// leak data across workspaces.
///
/// # Cache backend
///
/// The IndexedDB tier-2 cache is now set up directly on each `ChartMLRef`
/// via [`configured_chartml`]'s `workspace_id` parameter, which calls
/// `chartml.enable_indexeddb_cache`. This component provides only the
/// `ProviderRef` (datasource provider) via context.
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
    provide_chart_context(&workspace_id);
    children()
}

/// Provide the `ProviderRef` (datasource provider) context entry that
/// [`DashboardChartProviders`] would set up — useful when wrapping in the
/// component itself would force a `ChildrenFn` constraint that breaks
/// surrounding `FnOnce` reactive closures (e.g. the visual editor mode in
/// `dashboard_editor.rs` which moves a `Vec<ToolbarItem>` into its children).
/// Call this at the top of a component body to make `ProviderRef` available
/// to descendant `ChartMLChart`s via `use_context`.
///
/// The IndexedDB persistent cache is set up per-chart inside
/// [`configured_chartml`] via `chartml.enable_indexeddb_cache`, not here.
pub fn provide_chart_context(workspace_id: &str) {
    // Provider — synchronous, cheap, ready immediately.
    let provider: ProviderRef = Arc::new(KyomiDatasourceProvider::new(workspace_id.to_string()));
    provide_context(provider);
}

#[cfg(test)]
mod tests {
    use super::*;

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
}
