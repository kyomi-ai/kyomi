// SPDX-License-Identifier: AGPL-3.0-or-later

//! Query cache — a minimal react-query-equivalent for Leptos.
//!
//! Part 2 of [KYO-22]. The cache lives at the `Layout` level and survives
//! navigation, so a page that previously re-fetched its list data on every
//! mount now gets the last value **immediately** and shows no skeleton flash.
//!
//! ## Behaviour
//!
//! - **Stale-while-revalidate** — when a cached entry exists for a given
//!   `(name, deps)` pair, the hook returns the existing value signal right
//!   away and kicks off a background refetch. When the new data arrives the
//!   signal updates and the view reconciles (no flash).
//! - **Single in-flight fetch per key** — concurrent mounts or rapid deps
//!   changes can't trigger duplicate network requests for the same key.
//! - **Invalidation by name** — `cache.invalidate("dashboards")` re-runs the
//!   stored fetcher for every cached entry whose name matches, regardless of
//!   deps. The WebSocket refresh path collapses down to a single call.
//!
//! ## Storage shape
//!
//! One flat `HashMap<(name, deps_json), RawEntry>`. Entries are type-erased
//! via `Rc<dyn Any>` so a single generic `use_query<T>` can back every list
//! query instead of a parallel hook per type; `T` is re-asserted via downcast
//! on read. The whole cache is wrapped in `SendWrapper` because Leptos
//! requires context values to be `Send + Sync` even though this crate only
//! ever runs on WASM (single-threaded).
//!
//! ## What this does *not* do
//!
//! - No stale-time, no GC, no max-size. Entries live as long as the Layout.
//!   This is fine for a CSR app with a small, bounded set of list queries.
//! - No singular-fetch caching (`get_dashboard(id)`, etc.). Out of scope for
//!   KYO-22 Part 2 — singular resources remain on plain `Resource`.
//! - No optimistic updates. Callers that mutate server state still call the
//!   mutation, then `cache.invalidate(name)` to trigger a refetch.
//!
//! [KYO-22]: https://linear.app/kyomi/issue/KYO-22

use std::any::Any;
use std::cell::RefCell;
use std::collections::HashMap;
use std::future::Future;
use std::rc::Rc;

use leptos::prelude::*;
use send_wrapper::SendWrapper;
use serde::Serialize;

/// Composite key for a cached query: `(query name, serialized deps)`.
type QueryKey = (&'static str, String);

/// The full cache map type — split into a `type` alias to satisfy clippy's
/// `type_complexity` lint and to keep the `QueryCache` struct readable. The
/// `SendWrapper` is required because Leptos context values must be
/// `Send + Sync` even though this crate is CSR-only and the `Rc<RefCell>`
/// inside is single-threaded.
type CacheMap = SendWrapper<Rc<RefCell<HashMap<QueryKey, RawEntry>>>>;

/// Type-erased entry stored in the cache map.
///
/// `typed` is an `Rc<CacheEntry<T>>` for the actual `T` of this query. It's
/// stored behind `Rc<dyn Any>` so entries of different types can share the
/// same map; the generic [`use_query`] hook downcasts back to the concrete
/// type on read.
///
/// `refetch` is stored alongside so [`QueryCache::invalidate`] can re-run the
/// fetch without needing to know `T`.
struct RawEntry {
    typed: Rc<dyn Any>,
    refetch: Rc<dyn Fn()>,
}

/// A single cached query's live signal, shared across every call site that
/// reads this `(name, deps)` combination.
///
/// Uses [`ArcRwSignal`] — not the regular `RwSignal` — because the signals
/// must outlive the component that first created the entry. A regular
/// `RwSignal` is owned by the current reactive scope; when the originating
/// component unmounts the signal is disposed, and the next remount panics
/// with "you tried to access a reactive value [...] but it has already been
/// disposed". `ArcRwSignal` lives as long as the `Arc`, which in turn lives
/// as long as the cache entry — i.e. as long as the Layout.
struct CacheEntry<T: Send + Sync + 'static> {
    /// Latest fetch result. `None` means "never fetched yet"; `Some(Err(_))`
    /// means "last fetch failed — callers decide whether to show an error".
    data: ArcRwSignal<Option<Result<T, ServerFnError>>>,
    /// Guard against duplicate concurrent fetches for the same key.
    inflight: ArcRwSignal<bool>,
}

/// Layout-level query cache.
///
/// Backed by a `StoredValue` so the handle is `Copy` — callbacks and
/// closures can freely capture `query_cache` without juggling clones at
/// every call site. Consumers grab the handle once via
/// `expect_context::<QueryCache>()` and pass it around like any other
/// `Copy` leptos signal.
#[derive(Clone, Copy)]
pub struct QueryCache {
    inner: StoredValue<CacheMap>,
}

impl QueryCache {
    fn new() -> Self {
        Self {
            inner: StoredValue::new(SendWrapper::new(Rc::new(RefCell::new(
                HashMap::new(),
            )))),
        }
    }

    /// Re-run the stored fetcher for every cached entry whose query name
    /// matches `name`. Used by WebSocket event handlers to fold
    /// `dashboard_update` / `watch_run` / etc. into "dashboards changed,
    /// refresh whatever's cached".
    pub fn invalidate(&self, name: &'static str) {
        // Collect first, drop the borrow, then call — the refetch closures
        // use `leptos::task::spawn_local` internally which can re-enter the
        // cache on the next tick.
        let refetches: Vec<Rc<dyn Fn()>> = self.inner.with_value(|rc| {
            rc.borrow()
                .iter()
                .filter(|((n, _), _)| *n == name)
                .map(|(_, raw)| raw.refetch.clone())
                .collect()
        });
        for r in refetches {
            r();
        }
    }

    /// Like [`invalidate`] but silently no-ops if the cache's `StoredValue` has
    /// been disposed (e.g. the reactive owner unmounted). Use this variant
    /// inside `spawn_local` blocks where the async task may outlive the
    /// component that spawned it.
    pub fn try_invalidate(&self, name: &'static str) {
        let refetches: Vec<Rc<dyn Fn()>> = self
            .inner
            .try_with_value(|rc| {
                rc.borrow()
                    .iter()
                    .filter(|((n, _), _)| *n == name)
                    .map(|(_, raw)| raw.refetch.clone())
                    .collect()
            })
            .unwrap_or_default();
        for r in refetches {
            r();
        }
    }

    /// Borrow the underlying map mutably via the stored `StoredValue`.
    /// Internal helper used by [`lookup_or_create`].
    fn with_map_mut<R>(
        &self,
        f: impl FnOnce(&mut HashMap<QueryKey, RawEntry>) -> R,
    ) -> R {
        self.inner.with_value(|rc| f(&mut rc.borrow_mut()))
    }
}

/// Install a fresh [`QueryCache`] into the current reactive owner via
/// `provide_context`. Call this once inside `Layout` before rendering
/// children — every descendant can then read the cache with
/// `expect_context::<QueryCache>()` (or, more commonly, use the
/// [`use_query`] hook which handles that internally).
pub fn provide_query_cache() {
    provide_context(QueryCache::new());
}

/// Hook: fetch-and-cache a list query with stale-while-revalidate semantics.
///
/// Returns a `Signal` that yields `None` while the first fetch is in flight,
/// then `Some(Ok(data))` or `Some(Err(_))`. When `deps` change reactively,
/// the hook looks up a new cache entry keyed by the new deps — if one
/// exists it's returned immediately; if not, a fresh fetch is spawned.
///
/// The `fetcher` closure is called with a **clone** of `deps`, so pass
/// `move |args| server_fn(args.0, args.1, ...)` and let the server fn take
/// its arguments by value.
///
/// ## Example
///
/// ```ignore
/// let dashboards = use_query(
///     "dashboards",
///     move || (search.get(), sort.get()),
///     |(q, s)| list_dashboards(q, s, None, None, None),
/// );
///
/// view! {
///     <Suspense fallback=|| view! { <Skeleton/> }>
///         {move || dashboards.get().map(|res| match res { /* ... */ })}
///     </Suspense>
/// }
/// ```
pub fn use_query<T, D, F, Fut>(
    name: &'static str,
    deps: impl Fn() -> D + 'static,
    fetcher: F,
) -> Signal<Option<Result<T, ServerFnError>>>
where
    T: Clone + Send + Sync + 'static,
    D: Serialize + Clone + 'static,
    F: Fn(D) -> Fut + Copy + 'static,
    Fut: Future<Output = Result<T, ServerFnError>> + 'static,
{
    let cache = expect_context::<QueryCache>();

    // Holds the currently-active entry for the latest deps value. Changing
    // deps (e.g. user types in the search box) swaps in a different entry —
    // every reader downstream re-derives from the new signal automatically.
    //
    // Uses `new_local` because `Rc<CacheEntry<T>>` is not `Send`; the default
    // `RwSignal<T>` requires `Send + Sync` even though the CSR-only runtime
    // is single-threaded.
    let current = RwSignal::new_local(None::<Rc<CacheEntry<T>>>);

    // Synchronous initial lookup — runs at hook-call time, BEFORE the first
    // render. Critical for the no-flash behaviour: on a cache hit we populate
    // `current` with the existing entry (whose `data` signal already holds
    // the previous fetch result) so the first render shows the grid, not a
    // skeleton. `untrack(deps)` reads the current dep values without
    // subscribing — the Effect below handles reactive tracking for
    // subsequent deps changes.
    let initial_entry =
        lookup_or_create::<T, D, F, Fut>(&cache, name, untrack(&deps), fetcher);
    current.set(Some(initial_entry));

    // Watch for deps changes (e.g. search box edits) and swap the active
    // entry. Skips its first run so we don't do the lookup twice — the
    // initial one above already covers it.
    Effect::new(move |prev: Option<()>| {
        let d = deps();
        if prev.is_none() {
            return;
        }
        let entry = lookup_or_create::<T, D, F, Fut>(&cache, name, d, fetcher);
        current.set(Some(entry));
    });

    Signal::derive(move || current.get().and_then(|e| e.data.get()))
}

/// Look up an existing `CacheEntry` for this `(name, deps)` combination or
/// create a fresh one and kick off the initial fetch. On a cache hit, also
/// spawns a background refetch — that's the "revalidate" half of SWR.
fn lookup_or_create<T, D, F, Fut>(
    cache: &QueryCache,
    name: &'static str,
    deps: D,
    fetcher: F,
) -> Rc<CacheEntry<T>>
where
    T: Clone + Send + Sync + 'static,
    D: Serialize + Clone + 'static,
    F: Fn(D) -> Fut + Copy + 'static,
    Fut: Future<Output = Result<T, ServerFnError>> + 'static,
{
    let key_str = serde_json::to_string(&deps).unwrap_or_else(|e| {
        panic!("use_query: deps for '{name}' must be serde-serializable: {e}")
    });
    let key: QueryKey = (name, key_str);

    // Phase 1: look up or insert under the cache borrow. Don't call refetch
    // here — `refetch()` uses `spawn_local` which can re-enter the borrow.
    enum Action<T: Send + Sync + 'static> {
        Hit(Rc<CacheEntry<T>>, Rc<dyn Fn()>),
        Miss(Rc<CacheEntry<T>>, Rc<dyn Fn()>),
    }

    let action = cache.with_map_mut(|map| {
        if let Some(raw) = map.get(&key) {
            let entry = raw.typed.clone().downcast::<CacheEntry<T>>().unwrap_or_else(
                |_| panic!("use_query: cache key '{name}' reused with different T"),
            );
            Action::Hit(entry, raw.refetch.clone())
        } else {
            let entry = Rc::new(CacheEntry::<T> {
                data: ArcRwSignal::new(None),
                inflight: ArcRwSignal::new(false),
            });
            let refetch = build_refetch(entry.clone(), deps.clone(), fetcher);
            map.insert(
                key,
                RawEntry {
                    typed: entry.clone(),
                    refetch: refetch.clone(),
                },
            );
            Action::Miss(entry, refetch)
        }
    });

    // Phase 2: borrow released, safe to kick off the fetch.
    match action {
        Action::Hit(entry, refetch) => {
            refetch();
            entry
        }
        Action::Miss(entry, refetch) => {
            refetch();
            entry
        }
    }
}

/// Build the refetch closure for a freshly-created cache entry.
///
/// Factored out to keep [`use_query`]'s reactive body readable. The closure
/// owns a strong `Rc` to the entry and a clone of `deps`, and re-runs the
/// fetcher each time it's called — skipping if another fetch for the same
/// entry is already in flight.
fn build_refetch<T, D, F, Fut>(
    entry: Rc<CacheEntry<T>>,
    deps: D,
    fetcher: F,
) -> Rc<dyn Fn()>
where
    T: Send + Sync + 'static,
    D: Clone + 'static,
    F: Fn(D) -> Fut + Copy + 'static,
    Fut: Future<Output = Result<T, ServerFnError>> + 'static,
{
    Rc::new(move || {
        if entry.inflight.get_untracked() {
            return;
        }
        entry.inflight.set(true);
        let entry_for_task = entry.clone();
        let fut = fetcher(deps.clone());
        leptos::task::spawn_local(async move {
            let result = fut.await;
            entry_for_task.data.try_set(Some(result));
            entry_for_task.inflight.try_set(false);
        });
    })
}
