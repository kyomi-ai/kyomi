// SPDX-License-Identifier: AGPL-3.0-or-later

//! In-memory KVStore implementation backed by a `HashMap` protected by a
//! `tokio::sync::RwLock`.
//!
//! Suitable for single-instance deployments where Redis is not available.
//! A background task sweeps expired entries every 30 seconds.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::{Duration, Instant};

use tokio::sync::RwLock;

use crate::kv_store::{KVPool, KVStore};

// ---------------------------------------------------------------------------
// Internal entry type
// ---------------------------------------------------------------------------

struct Entry {
    value: String,
    expires_at: Option<Instant>,
}

impl Entry {
    fn new(value: String, ttl_secs: Option<u64>) -> Self {
        Self {
            value,
            expires_at: ttl_secs.map(|s| Instant::now() + Duration::from_secs(s)),
        }
    }

    fn is_expired(&self) -> bool {
        self.expires_at
            .map(|t| Instant::now() > t)
            .unwrap_or(false)
    }
}

// ---------------------------------------------------------------------------
// InMemoryKVStore
// ---------------------------------------------------------------------------

/// In-process key-value store with optional per-key TTLs.
///
/// Construct via [`InMemoryKVStore::new_pool`] rather than directly — the
/// factory function wraps the store in an `Arc` and starts the background
/// expiry sweep task.
pub struct InMemoryKVStore {
    data: RwLock<HashMap<String, Entry>>,
    sets: RwLock<HashMap<String, HashSet<String>>>,
}

impl InMemoryKVStore {
    /// Create a new `Arc<InMemoryKVStore>` and spawn a background task that
    /// removes expired entries every 30 seconds.
    ///
    /// Returns a [`KVPool`] (i.e. `Arc<dyn KVStore>`) ready for use.
    pub fn new_pool() -> KVPool {
        let store = Arc::new(Self {
            data: RwLock::new(HashMap::new()),
            sets: RwLock::new(HashMap::new()),
        });

        // Background expiry sweep — runs every 30 seconds.
        let sweep_handle = Arc::clone(&store);
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(Duration::from_secs(30));
            // The first tick fires immediately — skip it so we don't sweep at
            // startup before anything has had a chance to expire.
            interval.tick().await;
            loop {
                interval.tick().await;
                sweep_handle.sweep_expired().await;
            }
        });

        store
    }

    /// Remove all expired entries from the map.
    async fn sweep_expired(&self) {
        let mut data = self.data.write().await;
        data.retain(|_, entry| !entry.is_expired());
    }
}

// ---------------------------------------------------------------------------
// KVStore implementation
// ---------------------------------------------------------------------------

#[async_trait::async_trait]
impl KVStore for InMemoryKVStore {
    async fn set(&self, key: &str, value: &str, ttl_secs: Option<u64>) -> crate::Result<()> {
        let mut data = self.data.write().await;
        data.insert(key.to_string(), Entry::new(value.to_string(), ttl_secs));
        Ok(())
    }

    async fn get(&self, key: &str) -> crate::Result<Option<String>> {
        let data = self.data.read().await;
        match data.get(key) {
            Some(entry) if !entry.is_expired() => Ok(Some(entry.value.clone())),
            _ => Ok(None),
        }
    }

    async fn del(&self, key: &str) -> crate::Result<()> {
        let mut data = self.data.write().await;
        data.remove(key);
        Ok(())
    }

    async fn getdel(&self, key: &str) -> crate::Result<Option<String>> {
        let mut data = self.data.write().await;
        match data.get(key) {
            Some(entry) if entry.is_expired() => {
                // Key present but expired — remove it and return None.
                data.remove(key);
                Ok(None)
            }
            Some(_) => {
                // Key present and live — remove and return the value.
                let entry = data.remove(key).expect("key was just checked");
                Ok(Some(entry.value))
            }
            None => Ok(None),
        }
    }

    async fn incr(&self, key: &str) -> crate::Result<i64> {
        let mut data = self.data.write().await;

        let (current, existing_ttl) = match data.get(key) {
            Some(entry) if !entry.is_expired() => {
                let n: i64 = entry.value.parse().unwrap_or(0);
                (n, entry.expires_at)
            }
            _ => (0, None),
        };

        let next = current + 1;

        // Preserve the existing TTL (expressed as remaining seconds from now)
        // so that `incr` doesn't inadvertently extend or reset expiry.
        let ttl_secs = existing_ttl.map(|t| {
            let remaining = t.saturating_duration_since(Instant::now());
            remaining.as_secs().max(1) // at least 1s so the entry is useful
        });

        let mut entry = Entry::new(next.to_string(), ttl_secs);
        // Restore the original `expires_at` exactly (avoid rounding from
        // Duration → secs → Duration conversion).
        entry.expires_at = existing_ttl;

        data.insert(key.to_string(), entry);
        Ok(next)
    }

    async fn expire(&self, key: &str, ttl_secs: u64) -> crate::Result<()> {
        let mut data = self.data.write().await;
        if let Some(entry) = data.get_mut(key)
            && !entry.is_expired() {
                entry.expires_at = Some(Instant::now() + Duration::from_secs(ttl_secs));
            }
        Ok(())
    }

    async fn sadd(&self, key: &str, member: &str) -> crate::Result<()> {
        let mut sets = self.sets.write().await;
        sets.entry(key.to_string())
            .or_insert_with(HashSet::new)
            .insert(member.to_string());
        Ok(())
    }

    async fn srem(&self, key: &str, member: &str) -> crate::Result<()> {
        let mut sets = self.sets.write().await;
        if let Some(set) = sets.get_mut(key) {
            set.remove(member);
            if set.is_empty() {
                sets.remove(key);
            }
        }
        Ok(())
    }

    async fn smembers(&self, key: &str) -> crate::Result<Vec<String>> {
        let sets = self.sets.read().await;
        match sets.get(key) {
            Some(set) => Ok(set.iter().cloned().collect()),
            None => Ok(vec![]),
        }
    }

    async fn sdel(&self, key: &str) -> crate::Result<()> {
        let mut sets = self.sets.write().await;
        sets.remove(key);
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    async fn store() -> KVPool {
        InMemoryKVStore::new_pool()
    }

    #[tokio::test]
    async fn set_and_get() {
        let s = store().await;
        s.set("k", "v", None).await.unwrap();
        assert_eq!(s.get("k").await.unwrap(), Some("v".to_string()));
    }

    #[tokio::test]
    async fn missing_key_returns_none() {
        let s = store().await;
        assert_eq!(s.get("missing").await.unwrap(), None);
    }

    #[tokio::test]
    async fn del_removes_key() {
        let s = store().await;
        s.set("k", "v", None).await.unwrap();
        s.del("k").await.unwrap();
        assert_eq!(s.get("k").await.unwrap(), None);
    }

    #[tokio::test]
    async fn getdel_returns_and_removes() {
        let s = store().await;
        s.set("k", "v", None).await.unwrap();
        assert_eq!(s.getdel("k").await.unwrap(), Some("v".to_string()));
        assert_eq!(s.get("k").await.unwrap(), None);
    }

    #[tokio::test]
    async fn getdel_missing_returns_none() {
        let s = store().await;
        assert_eq!(s.getdel("missing").await.unwrap(), None);
    }

    #[tokio::test]
    async fn incr_creates_and_increments() {
        let s = store().await;
        assert_eq!(s.incr("counter").await.unwrap(), 1);
        assert_eq!(s.incr("counter").await.unwrap(), 2);
        assert_eq!(s.incr("counter").await.unwrap(), 3);
    }

    #[tokio::test]
    async fn set_with_ttl_accessible_before_expiry() {
        let s = store().await;
        // A key with a generous TTL should be readable immediately.
        s.set("expiring", "val", Some(60)).await.unwrap();
        assert_eq!(s.get("expiring").await.unwrap(), Some("val".to_string()));
    }

    #[tokio::test]
    async fn expire_sets_ttl() {
        let s = store().await;
        s.set("k", "v", None).await.unwrap();
        s.expire("k", 60).await.unwrap();
        // Key should still be accessible immediately after setting TTL.
        assert_eq!(s.get("k").await.unwrap(), Some("v".to_string()));
    }

    #[tokio::test]
    async fn ping_succeeds() {
        let s = store().await;
        s.ping().await.unwrap();
    }
}
