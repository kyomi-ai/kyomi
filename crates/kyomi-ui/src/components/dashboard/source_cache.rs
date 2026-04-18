// SPDX-License-Identifier: AGPL-3.0-or-later

//! Dashboard-scoped cache for remote data sources used by ChartML charts.
//!
//! Multiple chart blocks on the same dashboard often reference the same
//! datasource + SQL query (e.g. a metric card and a chart derived from the
//! same underlying table). Fetching independently per block wastes a round
//! trip and can trigger N parallel requests for the same data on first load.
//!
//! `DashboardSourceCache` solves this in two ways:
//!
//! 1. **Deduplication** — a fetch keyed by `(slug, query)` is shared across
//!    all concurrent callers. Later arrivals attach to the in-flight Future
//!    rather than issuing a second request.
//! 2. **TTL-based caching** — successful fetches are retained for an optional
//!    TTL. Cache hits skip the network entirely. Expired entries are evicted
//!    lazily on the next fetch.
//!
//! The cache is single-threaded on purpose — it lives in the WASM main thread
//! under `Rc<RefCell<_>>`. Leptos context values must be `Send + Sync`, so the
//! shared state is wrapped in `SendWrapper` (same pattern as `QueryCache`) —
//! the wrapper panics if ever accessed from a different thread, which WASM's
//! single-threaded runtime guarantees never happens.

use std::cell::RefCell;
use std::collections::HashMap;
use std::future::Future;
use std::hash::{Hash, Hasher};
use std::pin::Pin;
use std::rc::Rc;
use std::task::{Context, Poll, Waker};
use std::time::Duration;

use chartml_core::data::DataTable;
use send_wrapper::SendWrapper;

/// Key derived from `(slug, query)`. We store the hash directly so the caller's
/// strings don't need to hang around for the lifetime of the cache.
fn compute_key(slug: &str, query: &str) -> u64 {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    slug.hash(&mut hasher);
    0u8.hash(&mut hasher); // separator — avoid collisions like ("ab","c") vs ("a","bc")
    query.hash(&mut hasher);
    hasher.finish()
}

/// Shared state for a single in-flight fetch. `result` stays `None` until the
/// fetcher completes, then every waker is notified.
struct InFlightShared {
    result: Option<Result<DataTable, String>>,
    wakers: Vec<Waker>,
}

/// A cached fetch entry.
enum CacheEntry {
    /// Fetch in progress — shared state is polled by all waiting futures.
    InFlight(Rc<RefCell<InFlightShared>>),
    /// Successful result, cached with its fetched-at timestamp (`js_sys::Date::now()`
    /// ms since Unix epoch) and optional TTL.
    Ready {
        table: DataTable,
        fetched_at_ms: f64,
        ttl: Option<Duration>,
    },
}

impl CacheEntry {
    /// True if `Ready` and its TTL has elapsed. `None` TTL = never expires.
    fn is_expired(&self, now_ms: f64) -> bool {
        match self {
            CacheEntry::Ready {
                fetched_at_ms, ttl, ..
            } => match ttl {
                Some(d) => (now_ms - fetched_at_ms) >= (d.as_secs_f64() * 1000.0),
                None => false,
            },
            CacheEntry::InFlight(_) => false,
        }
    }
}

/// Dashboard-scoped source cache. Cheaply `Clone`-able (Rc inside).
///
/// Wrapped in `SendWrapper` so it can be stored in Leptos context
/// (which requires `Send + Sync`) despite holding non-Send `Rc`/`RefCell`.
/// Safe in practice because WASM is single-threaded.
#[derive(Clone)]
pub struct DashboardSourceCache {
    inner: SendWrapper<Rc<RefCell<HashMap<u64, CacheEntry>>>>,
}

impl Default for DashboardSourceCache {
    fn default() -> Self {
        Self::new()
    }
}

/// Future returned to waiters on an in-flight fetch. Polling registers a waker
/// and resolves once the owner of the fetch writes `result`.
struct WaitForInFlight {
    shared: Rc<RefCell<InFlightShared>>,
}

impl Future for WaitForInFlight {
    type Output = Result<DataTable, String>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let mut state = self.shared.borrow_mut();
        if let Some(result) = state.result.as_ref() {
            return Poll::Ready(result.clone());
        }
        // Only register the current waker if it isn't already in the list.
        let waker = cx.waker();
        if !state.wakers.iter().any(|w| w.will_wake(waker)) {
            state.wakers.push(waker.clone());
        }
        Poll::Pending
    }
}

impl DashboardSourceCache {
    /// Create a fresh empty cache.
    pub fn new() -> Self {
        Self {
            inner: SendWrapper::new(Rc::new(RefCell::new(HashMap::new()))),
        }
    }

    /// Fetch `(slug, query)` via `fetcher`, with deduplication and TTL caching.
    ///
    /// - Cache hit (Ready + not expired) → returns cloned `DataTable` immediately.
    /// - In-flight → awaits the existing fetch, no new network call.
    /// - Miss / expired → invokes `fetcher`, stores the result on success, fans
    ///   out to any queued waiters. Failures are NOT cached — the next caller
    ///   will retry.
    pub async fn fetch<F, Fut>(
        &self,
        slug: &str,
        query: &str,
        ttl: Option<Duration>,
        fetcher: F,
    ) -> Result<DataTable, String>
    where
        F: FnOnce() -> Fut,
        Fut: Future<Output = Result<DataTable, String>>,
    {
        let key = compute_key(slug, query);
        let now_ms = js_sys::Date::now();

        // Phase 1: classify the current state under one short-lived borrow.
        enum Action {
            ReturnReady(DataTable),
            Wait(Rc<RefCell<InFlightShared>>),
            Fetch(Rc<RefCell<InFlightShared>>),
        }

        let action = {
            let mut map = (*self.inner).borrow_mut();

            // Evict expired Ready entries lazily.
            let expired = map
                .get(&key)
                .map(|e| e.is_expired(now_ms))
                .unwrap_or(false);
            if expired {
                map.remove(&key);
            }

            match map.get(&key) {
                Some(CacheEntry::Ready { table, .. }) => Action::ReturnReady(table.clone()),
                Some(CacheEntry::InFlight(shared)) => Action::Wait(shared.clone()),
                None => {
                    let shared = Rc::new(RefCell::new(InFlightShared {
                        result: None,
                        wakers: Vec::new(),
                    }));
                    map.insert(key, CacheEntry::InFlight(shared.clone()));
                    Action::Fetch(shared)
                }
            }
        };

        match action {
            Action::ReturnReady(table) => Ok(table),
            Action::Wait(shared) => WaitForInFlight { shared }.await,
            Action::Fetch(shared) => {
                // Run the fetcher without holding the cache borrow.
                let result = fetcher().await;
                let fetched_at_ms = js_sys::Date::now();

                // Phase 2: install the Ready entry on success, or drop the
                // InFlight placeholder on failure. Then wake all waiters so
                // they observe the finalized state.
                {
                    let mut map = (*self.inner).borrow_mut();
                    match &result {
                        Ok(table) => {
                            map.insert(
                                key,
                                CacheEntry::Ready {
                                    table: table.clone(),
                                    fetched_at_ms,
                                    ttl,
                                },
                            );
                        }
                        Err(_) => {
                            // Failures are not cached — remove the InFlight
                            // placeholder so the next caller retries.
                            map.remove(&key);
                        }
                    }
                }

                let wakers = {
                    let mut state = shared.borrow_mut();
                    state.result = Some(result.clone());
                    std::mem::take(&mut state.wakers)
                };
                for w in wakers {
                    w.wake();
                }

                result
            }
        }
    }
}
