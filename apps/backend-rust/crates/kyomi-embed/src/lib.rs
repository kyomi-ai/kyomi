// SPDX-License-Identifier: AGPL-3.0-or-later

//! kyomi-embed — Text embedding via fastembed (ONNX Runtime).
//!
//! Wraps the `fastembed` crate to provide a simple embedding service.
//!
//! Model: `BGE-small-en-v1.5` (384 dimensions, asymmetric encoding).
//!
//! BGE is an asymmetric model — queries and passages are embedded differently:
//! - **Passages** (stored data: catalog entries, learning insights, descriptions):
//!   embedded as-is via [`EmbeddingService::embed_passage`] / [`EmbeddingService::embed_passages`].
//! - **Queries** (user search terms): embedded with a prefix via [`EmbeddingService::embed_query`].

use fastembed::{
    InitOptionsUserDefined, Pooling, TextEmbedding, TokenizerFiles, UserDefinedEmbeddingModel,
};
use std::sync::{Arc, OnceLock};
use tokio::sync::Notify;

/// BGE query prefix — prepended to search queries for asymmetric retrieval.
const BGE_QUERY_PREFIX: &str = "Represent this sentence for searching relevant passages: ";

// Embed the model files at compile time (downloaded by build.rs)
const MODEL_ONNX: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/model.onnx"));
const TOKENIZER_JSON: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/tokenizer.json"));
const CONFIG_JSON: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/config.json"));
const SPECIAL_TOKENS_MAP: &[u8] =
    include_bytes!(concat!(env!("OUT_DIR"), "/special_tokens_map.json"));
const TOKENIZER_CONFIG: &[u8] =
    include_bytes!(concat!(env!("OUT_DIR"), "/tokenizer_config.json"));

/// Thread-safe embedding service.
///
/// Internally holds an `Arc` so it can be cheaply cloned into axum state.
#[derive(Clone)]
pub struct EmbeddingService {
    model: Arc<TextEmbedding>,
}

impl EmbeddingService {
    /// Load the embedding model. This is expensive (~500ms) — do it once at
    /// startup and share via axum `State`.
    ///
    /// The model is embedded in the binary at compile time — no runtime
    /// downloads from HuggingFace are needed.
    pub fn new() -> kyomi_core::Result<Self> {
        let user_model = UserDefinedEmbeddingModel::new(
            MODEL_ONNX.to_vec(),
            TokenizerFiles {
                tokenizer_file: TOKENIZER_JSON.to_vec(),
                config_file: CONFIG_JSON.to_vec(),
                special_tokens_map_file: SPECIAL_TOKENS_MAP.to_vec(),
                tokenizer_config_file: TOKENIZER_CONFIG.to_vec(),
            },
        )
        .with_pooling(Pooling::Cls); // CRITICAL: BGE-small-en-v1.5 uses CLS pooling

        let model = TextEmbedding::try_new_from_user_defined(
            user_model,
            InitOptionsUserDefined::default(),
        )
        .map_err(|e| {
            kyomi_core::Error::Internal(format!("failed to load embedding model: {e}"))
        })?;

        tracing::info!("Embedding model loaded (BGE-small-en-v1.5, 384 dims, embedded)");
        Ok(Self {
            model: Arc::new(model),
        })
    }

    // ─── Passage embedding (no prefix) ──────────────────────────────────

    /// Embed a batch of passages (catalog entries, learning insights, descriptions).
    ///
    /// Passages are embedded as-is — no query prefix. Returns one `Vec<f32>`
    /// (384 dimensions) per input text.
    pub fn embed_passages(&self, texts: &[&str]) -> kyomi_core::Result<Vec<Vec<f32>>> {
        let texts: Vec<String> = texts.iter().map(|s| s.to_string()).collect();
        self.model
            .embed(texts, None)
            .map_err(|e| kyomi_core::Error::Internal(format!("embedding failed: {e}")))
    }

    /// Embed a single passage. Convenience wrapper around [`embed_passages`].
    pub fn embed_passage(&self, text: &str) -> kyomi_core::Result<Vec<f32>> {
        let mut results = self.embed_passages(&[text])?;
        results
            .pop()
            .ok_or_else(|| kyomi_core::Error::Internal("embedding returned empty result".into()))
    }

    // ─── Query embedding (with BGE prefix) ──────────────────────────────

    /// Embed a search query with the BGE query prefix for asymmetric retrieval.
    ///
    /// The prefix `"Represent this sentence for searching relevant passages: "`
    /// is prepended automatically.
    pub fn embed_query(&self, query: &str) -> kyomi_core::Result<Vec<f32>> {
        let prefixed = format!("{BGE_QUERY_PREFIX}{query}");
        let mut results = self.model
            .embed(vec![prefixed], None)
            .map_err(|e| kyomi_core::Error::Internal(format!("embedding failed: {e}")))?;
        results
            .pop()
            .ok_or_else(|| kyomi_core::Error::Internal("embedding returned empty result".into()))
    }

    // ─── Backward-compatible aliases ────────────────────────────────────
    //
    // DEPRECATED: These aliases exist for backward compatibility with code that
    // was written before asymmetric encoding was introduced (BGE model switch).
    // Callers should use `embed_passage()` / `embed_passages()` for stored data
    // and `embed_query()` for search queries. These aliases will be removed
    // once all callers are migrated.
    // These aliases can be removed once all callers are migrated.

    /// DEPRECATED: Use [`embed_passages`] instead.
    /// Alias kept for backward compatibility during graph migration.
    pub fn embed(&self, texts: &[&str]) -> kyomi_core::Result<Vec<Vec<f32>>> {
        self.embed_passages(texts)
    }

    /// DEPRECATED: Use [`embed_passage`] instead.
    /// Alias kept for backward compatibility during graph migration.
    pub fn embed_one(&self, text: &str) -> kyomi_core::Result<Vec<f32>> {
        self.embed_passage(text)
    }

    /// The dimensionality of embeddings produced by this model.
    pub const DIMENSIONS: usize = 384;
}

// ===========================================================================
// LazyEmbedding — deferred model loading for faster startup
// ===========================================================================

/// Lazy-loading wrapper around [`EmbeddingService`].
///
/// The server starts listening immediately while the embedding model loads
/// on a background thread (~440ms). Endpoints that need embeddings get a
/// 503 Service Unavailable during the brief warmup window.
///
/// # Usage
/// ```text
/// // For endpoints (fail fast if not loaded)
/// let embedding = lazy_embedding.get()?.embed_query(query)?;
///
/// // For background tasks (wait for load)
/// let embedding = lazy_embedding.wait_ready().await?.embed_query(query)?;
/// ```
#[derive(Clone)]
pub struct LazyEmbedding {
    inner: Arc<OnceLock<EmbeddingService>>,
    ready: Arc<Notify>,
}

impl LazyEmbedding {
    /// Create an empty `LazyEmbedding`. Call [`set`] once the model is loaded.
    pub fn new() -> Self {
        Self {
            inner: Arc::new(OnceLock::new()),
            ready: Arc::new(Notify::new()),
        }
    }

    /// Create a `LazyEmbedding` with the model already loaded.
    /// Useful for tests and contexts where blocking startup is acceptable.
    pub fn loaded(svc: EmbeddingService) -> Self {
        let lock = OnceLock::new();
        lock.set(svc).ok();
        Self {
            inner: Arc::new(lock),
            ready: Arc::new(Notify::new()),
        }
    }

    /// Set the loaded embedding service. Called once from the background loader.
    pub fn set(&self, svc: EmbeddingService) {
        self.inner.set(svc).ok();
        self.ready.notify_waiters();
    }

    /// Get a reference to the inner service, or `None` if still loading.
    pub fn try_get(&self) -> Option<&EmbeddingService> {
        self.inner.get()
    }

    /// Get a reference to the inner service, or a 503 error if still loading.
    pub fn get(&self) -> kyomi_core::Result<&EmbeddingService> {
        self.inner.get().ok_or_else(|| {
            kyomi_core::Error::ServiceUnavailable(
                "Embedding model still loading, try again shortly".into(),
            )
        })
    }

    /// Wait until the model is loaded. For background tasks (schedulers)
    /// that can afford to wait at first use rather than failing.
    pub async fn wait_ready(&self) -> kyomi_core::Result<&EmbeddingService> {
        // Fast path: already loaded
        if let Some(svc) = self.inner.get() {
            return Ok(svc);
        }
        // Register for notification *before* re-checking to avoid race
        let notified = self.ready.notified();
        if let Some(svc) = self.inner.get() {
            return Ok(svc);
        }
        notified.await;
        self.inner.get().ok_or_else(|| {
            kyomi_core::Error::Internal(
                "Embedding service not available after initialization notification".into(),
            )
        })
    }
}

impl Default for LazyEmbedding {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn passage_embedding_generates_correct_dimensions() {
        let svc = EmbeddingService::new().unwrap();
        let embedding = svc.embed_passage("hello world").unwrap();
        assert_eq!(embedding.len(), EmbeddingService::DIMENSIONS);
    }

    #[test]
    fn passage_batch_embedding() {
        let svc = EmbeddingService::new().unwrap();
        let results = svc.embed_passages(&["hello", "world"]).unwrap();
        assert_eq!(results.len(), 2);
        assert_eq!(results[0].len(), EmbeddingService::DIMENSIONS);
        assert_eq!(results[1].len(), EmbeddingService::DIMENSIONS);
    }

    #[test]
    fn query_embedding_correct_dimensions() {
        let svc = EmbeddingService::new().unwrap();
        let embedding = svc.embed_query("hello world").unwrap();
        assert_eq!(embedding.len(), EmbeddingService::DIMENSIONS);
    }

    #[test]
    fn asymmetric_encoding_query_vs_passage() {
        let svc = EmbeddingService::new().unwrap();

        // A query embedding should differ from a passage embedding of the same text
        // because the query has the BGE prefix prepended.
        let query_emb = svc.embed_query("email").unwrap();
        let passage_emb = svc.embed_passage("email").unwrap();

        // They should NOT be identical (prefix changes the embedding)
        let sim = cosine_similarity(&query_emb, &passage_emb);
        assert!(
            sim < 0.999,
            "query and passage embeddings of same text should differ due to prefix, similarity: {sim}"
        );
    }

    #[test]
    fn similar_texts_closer() {
        let svc = EmbeddingService::new().unwrap();

        // Use embed_query for the search query, embed_passages for the passages
        let query_emb = svc.embed_query("revenue by region").unwrap();
        let passages = svc
            .embed_passages(&["sales data per area", "chocolate cake recipe"])
            .unwrap();

        let sim_related = cosine_similarity(&query_emb, &passages[0]);
        let sim_unrelated = cosine_similarity(&query_emb, &passages[1]);

        assert!(
            sim_related > sim_unrelated,
            "related texts should be more similar: {sim_related} vs {sim_unrelated}"
        );
    }

    #[test]
    fn backward_compat_aliases() {
        let svc = EmbeddingService::new().unwrap();

        // embed_one should produce same result as embed_passage
        let one = svc.embed_one("test text").unwrap();
        let passage = svc.embed_passage("test text").unwrap();
        assert_eq!(one, passage);

        // embed should produce same result as embed_passages
        let batch = svc.embed(&["hello", "world"]).unwrap();
        let passages = svc.embed_passages(&["hello", "world"]).unwrap();
        assert_eq!(batch, passages);
    }

    fn cosine_similarity(a: &[f32], b: &[f32]) -> f32 {
        let dot: f32 = a.iter().zip(b.iter()).map(|(x, y)| x * y).sum();
        let norm_a: f32 = a.iter().map(|x| x * x).sum::<f32>().sqrt();
        let norm_b: f32 = b.iter().map(|x| x * x).sum::<f32>().sqrt();
        dot / (norm_a * norm_b)
    }

    // --- LazyEmbedding tests ---

    #[test]
    fn lazy_embedding_unloaded_returns_error() {
        let lazy = LazyEmbedding::new();
        assert!(lazy.try_get().is_none());
        assert!(lazy.get().is_err());
    }

    #[test]
    fn lazy_embedding_loaded_works() {
        let svc = EmbeddingService::new().unwrap();
        let lazy = LazyEmbedding::loaded(svc);
        assert!(lazy.try_get().is_some());
        let result = lazy.get().unwrap().embed_passage("test").unwrap();
        assert_eq!(result.len(), EmbeddingService::DIMENSIONS);
    }

    #[test]
    fn lazy_embedding_set_then_get() {
        let lazy = LazyEmbedding::new();
        assert!(lazy.try_get().is_none());
        let svc = EmbeddingService::new().unwrap();
        lazy.set(svc);
        assert!(lazy.try_get().is_some());
        assert!(lazy.get().is_ok());
    }

    #[tokio::test]
    async fn lazy_embedding_wait_ready() {
        let lazy = LazyEmbedding::new();
        let lazy2 = lazy.clone();
        tokio::task::spawn_blocking(move || {
            let svc = EmbeddingService::new().unwrap();
            lazy2.set(svc);
        });
        let svc = lazy.wait_ready().await.unwrap();
        let result = svc.embed_passage("test").unwrap();
        assert_eq!(result.len(), EmbeddingService::DIMENSIONS);
    }
}
