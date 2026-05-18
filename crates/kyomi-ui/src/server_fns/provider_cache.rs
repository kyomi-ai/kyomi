// SPDX-License-Identifier: AGPL-3.0-or-later

//! Short-lived provider cache for SQL Editor dry-run calls.
//!
//! Each `dry_run_sql` invocation used to create a brand-new `DatasourceProvider`
//! instance — DB lookup, credential decryption, OAuth token check, HTTP client —
//! all discarded immediately after. This adds 300-500ms overhead per dry run
//! (debounced at 1s).
//!
//! This module introduces a 60-second TTL in-memory cache keyed on
//! `(user_id, workspace_id, datasource_slug)` so that repeated dry-run calls
//! from the same user reuse the same provider. A background eviction task runs
//! every 30 seconds to drop stale entries.
//!
//! Only `dry_run_sql` uses this cache. Other server functions that create
//! providers have different lifetime expectations and are unaffected.

#[cfg(feature = "ssr")]
mod inner {
    use std::sync::{Arc, OnceLock};
    use std::time::{Duration, Instant};

    use dashmap::DashMap;
    use kyomi_datasource_server::DatasourceProvider;

    const TTL: Duration = Duration::from_secs(60);
    const EVICTION_INTERVAL: Duration = Duration::from_secs(30);

    struct CachedEntry {
        provider: Arc<dyn DatasourceProvider>,
        created_at: Instant,
    }

    fn cache() -> &'static DashMap<String, CachedEntry> {
        static CACHE: OnceLock<DashMap<String, CachedEntry>> = OnceLock::new();
        CACHE.get_or_init(|| {
            let map = DashMap::new();
            tokio::spawn(eviction_loop());
            map
        })
    }

    fn cache_key(user_id: &str, workspace_id: &str, datasource_slug: &str) -> String {
        format!("{user_id}\0{workspace_id}\0{datasource_slug}")
    }

    async fn eviction_loop() {
        loop {
            tokio::time::sleep(EVICTION_INTERVAL).await;
            cache().retain(|_, entry| entry.created_at.elapsed() < TTL);
        }
    }

    /// Try to get a cached provider. Returns `None` on miss or TTL expiry.
    pub(crate) fn get_cached(
        user_id: &str,
        workspace_id: &str,
        datasource_slug: &str,
    ) -> Option<Arc<dyn DatasourceProvider>> {
        let key = cache_key(user_id, workspace_id, datasource_slug);
        let entry = cache().get(&key)?;
        if entry.created_at.elapsed() >= TTL {
            drop(entry);
            cache().remove_if(&key, |_, e| e.created_at.elapsed() >= TTL);
            return None;
        }
        Some(Arc::clone(&entry.provider))
    }

    /// Insert a provider into the cache. Converts `Box<dyn DatasourceProvider>`
    /// to `Arc` and returns the shared reference so the caller can use it
    /// immediately without a second lookup.
    pub(crate) fn insert(
        user_id: &str,
        workspace_id: &str,
        datasource_slug: &str,
        provider: Box<dyn DatasourceProvider>,
    ) -> Arc<dyn DatasourceProvider> {
        let key = cache_key(user_id, workspace_id, datasource_slug);
        let arc: Arc<dyn DatasourceProvider> = Arc::from(provider);
        cache().insert(
            key,
            CachedEntry {
                provider: Arc::clone(&arc),
                created_at: Instant::now(),
            },
        );
        arc
    }
}

// `close()` is intentionally not called on eviction. The `Arc` drops the
// provider when refcount reaches zero, and all provider implementations
// clean up resources through `Drop` (sqlx pools close, SSH tunnel tasks
// abort, stateless REST providers are no-ops).
#[cfg(feature = "ssr")]
pub(crate) use inner::{get_cached, insert};
