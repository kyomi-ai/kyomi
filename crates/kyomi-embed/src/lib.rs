// SPDX-License-Identifier: AGPL-3.0-or-later

//! kyomi-embed — Text embedding via Candle (pure Rust).
//!
//! Wraps the Candle BERT model to provide a simple embedding service.
//!
//! Model: `BGE-small-en-v1.5` (384 dimensions, asymmetric encoding).
//!
//! BGE is an asymmetric model — queries and passages are embedded differently:
//! - **Passages** (stored data: catalog entries, learning insights, descriptions):
//!   embedded as-is via [`EmbeddingService::embed_passage`] / [`EmbeddingService::embed_passages`].
//! - **Queries** (user search terms): embedded with a prefix via [`EmbeddingService::embed_query`].

use candle_core::{Device, Tensor};
use candle_nn::VarBuilder;
use candle_transformers::models::bert::{BertModel, Config, DTYPE};
use std::sync::{Arc, OnceLock};
use tokenizers::{PaddingParams, PaddingStrategy, Tokenizer, TruncationParams};
use tokio::sync::Notify;

/// BGE query prefix — prepended to search queries for asymmetric retrieval.
const BGE_QUERY_PREFIX: &str = "Represent this sentence for searching relevant passages: ";

/// Maximum number of texts embedded in a single `spawn_blocking` call by
/// [`EmbeddingService::embed_passages_chunked`].
///
/// `embed_texts` is a synchronous, CPU-bound Candle forward pass — cost
/// scales with batch size, and an unbounded batch (538 columns in the
/// KYO-644 reproduction) took ~34s. 64 keeps any single blocking-pool
/// occupation short and bounds peak tensor memory, while still being large
/// enough to amortize tokenizer/forward-pass overhead across a meaningful
/// number of passages per chunk.
const EMBED_BATCH_SIZE: usize = 64;

// Embed the model files at compile time (downloaded by build.rs)
const MODEL_SAFETENSORS: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/model.safetensors"));
const TOKENIZER_JSON: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/tokenizer.json"));
const CONFIG_JSON: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/config.json"));

/// Thread-safe embedding service.
///
/// Internally holds an `Arc` so it can be cheaply cloned into axum state.
#[derive(Clone)]
pub struct EmbeddingService {
    inner: Arc<EmbeddingInner>,
}

struct EmbeddingInner {
    model: BertModel,
    tokenizer: Tokenizer,
    device: Device,
}

impl EmbeddingService {
    /// Load the embedding model. This is expensive — do it once at
    /// startup and share via axum `State`.
    ///
    /// The model is embedded in the binary at compile time — no runtime
    /// downloads from HuggingFace are needed.
    pub fn new() -> kyomi_core::Result<Self> {
        let device = Device::Cpu;

        // Load config
        let config: Config = serde_json::from_slice(CONFIG_JSON).map_err(|e| {
            kyomi_core::Error::Internal(format!("failed to parse model config: {e}"))
        })?;

        // Load weights from embedded safetensors
        let safetensors =
            safetensors::SafeTensors::deserialize(MODEL_SAFETENSORS).map_err(|e| {
                kyomi_core::Error::Internal(format!("failed to deserialize safetensors: {e}"))
            })?;
        let vb = VarBuilder::from_buffered_safetensors(
            MODEL_SAFETENSORS.to_vec(),
            DTYPE,
            &device,
        )
        .map_err(|e| {
            kyomi_core::Error::Internal(format!("failed to create var builder: {e}"))
        })?;
        // Verify the safetensors loaded correctly by checking tensor count
        let tensor_count = safetensors.names().len();
        tracing::debug!("Loaded {tensor_count} tensors from safetensors");

        let model = BertModel::load(vb, &config).map_err(|e| {
            kyomi_core::Error::Internal(format!("failed to load BERT model: {e}"))
        })?;

        // Load tokenizer
        let tokenizer_str = std::str::from_utf8(TOKENIZER_JSON).map_err(|e| {
            kyomi_core::Error::Internal(format!("tokenizer.json is not valid UTF-8: {e}"))
        })?;
        let mut tokenizer = Tokenizer::from_bytes(tokenizer_str.as_bytes()).map_err(|e| {
            kyomi_core::Error::Internal(format!("failed to load tokenizer: {e}"))
        })?;

        // Configure truncation (BGE max length = 512)
        tokenizer
            .with_truncation(Some(TruncationParams {
                max_length: 512,
                ..Default::default()
            }))
            .map_err(|e| {
                kyomi_core::Error::Internal(format!("failed to set truncation: {e}"))
            })?;

        tracing::info!("Embedding model loaded (BGE-small-en-v1.5, 384 dims, Candle pure Rust)");

        Ok(Self {
            inner: Arc::new(EmbeddingInner {
                model,
                tokenizer,
                device,
            }),
        })
    }

    // ─── Passage embedding (no prefix) ──────────────────────────────────

    /// Embed a batch of passages (catalog entries, learning insights, descriptions).
    ///
    /// Passages are embedded as-is — no query prefix. Returns one `Vec<f32>`
    /// (384 dimensions) per input text.
    ///
    /// This is a **synchronous, CPU-bound call** (tokenize + one Candle BERT
    /// forward pass, no yield points) — calling it directly from an async
    /// context occupies whatever executor thread runs the call for the
    /// entire batch. From async code embedding catalog-sized batches (tens
    /// to thousands of passages), use [`embed_passages_chunked`] instead —
    /// see its docs for why (KYO-644).
    ///
    /// [`embed_passages_chunked`]: Self::embed_passages_chunked
    pub fn embed_passages(&self, texts: &[&str]) -> kyomi_core::Result<Vec<Vec<f32>>> {
        self.embed_texts(texts)
    }

    /// Embed a batch of passages off the async runtime, in bounded chunks.
    ///
    /// [`embed_passages`](Self::embed_passages) is a synchronous, CPU-bound
    /// Candle forward pass with no yield points. Called directly from async
    /// code on an unbounded batch, it stalls whichever executor thread runs
    /// it for as long as the whole batch takes — in production this was a
    /// single 538-column batch that occupied the calling thread for ~34s,
    /// which correlated with the entire HTTP server going unresponsive for
    /// the same window (a bare `GET /api/health` blocked 34.4s and recovered
    /// to 3ms the instant the embedding call returned). The exact mechanism
    /// linking one occupied thread to whole-pool unresponsiveness was not
    /// conclusively isolated (KYO-644: catalog indexing's post-indexing
    /// embedding pass, called directly on a `tokio::spawn`ed task).
    ///
    /// This method fixes both halves of that defect:
    /// - Each chunk's embedding work runs via `tokio::task::spawn_blocking`,
    ///   so the calling task yields back to the runtime between chunks
    ///   instead of monopolizing an executor thread for the whole batch.
    /// - `texts` is split into groups of at most `EMBED_BATCH_SIZE`, so a
    ///   single forward pass — and its CPU burst and peak tensor memory —
    ///   is bounded regardless of how large the caller's batch is.
    ///
    /// A `spawn_blocking` panic is propagated as an `Err` rather than
    /// discarded: the caller's batch is genuinely incomplete when this
    /// returns an error, and losing that distinction would let a partial
    /// embedding set look identical to a complete one.
    pub async fn embed_passages_chunked(&self, texts: &[&str]) -> kyomi_core::Result<Vec<Vec<f32>>> {
        let mut results = Vec::with_capacity(texts.len());
        for (chunk_index, chunk) in texts.chunks(EMBED_BATCH_SIZE).enumerate() {
            let svc = self.clone();
            let owned: Vec<String> = chunk.iter().map(|s| s.to_string()).collect();
            let chunk_result = tokio::task::spawn_blocking(move || {
                let refs: Vec<&str> = owned.iter().map(String::as_str).collect();
                svc.embed_passages(&refs)
            })
            .await
            .map_err(|e| {
                kyomi_core::Error::Internal(format!("embedding task panicked: {e}"))
            })??;

            tracing::debug!(
                chunk_index,
                chunk_size = chunk_result.len(),
                total = texts.len(),
                "embedded passage chunk off the async runtime"
            );

            results.extend(chunk_result);
        }
        Ok(results)
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
        let mut results = self.embed_texts(&[&prefixed])?;
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

    // ─── Internal ───────────────────────────────────────────────────────

    fn embed_texts(&self, texts: &[&str]) -> kyomi_core::Result<Vec<Vec<f32>>> {
        let inner = &self.inner;

        if texts.is_empty() {
            return Ok(vec![]);
        }

        // Tokenize with padding for batch processing
        let mut tokenizer = inner.tokenizer.clone();
        if texts.len() > 1 {
            tokenizer.with_padding(Some(PaddingParams {
                strategy: PaddingStrategy::BatchLongest,
                ..Default::default()
            }));
        }

        let encodings = tokenizer
            .encode_batch(texts.to_vec(), true)
            .map_err(|e| kyomi_core::Error::Internal(format!("tokenization failed: {e}")))?;

        let batch_size = encodings.len();

        // Build tensors
        let token_ids: Vec<&[u32]> = encodings.iter().map(|e| e.get_ids()).collect();
        let token_type_ids_data: Vec<Vec<u32>> = encodings
            .iter()
            .map(|e| vec![0u32; e.get_ids().len()])
            .collect();
        let attention_masks: Vec<&[u32]> =
            encodings.iter().map(|e| e.get_attention_mask()).collect();

        let token_ids = Tensor::new(token_ids, &inner.device).map_err(candle_err)?;
        let token_type_ids_refs: Vec<&[u32]> =
            token_type_ids_data.iter().map(|v| v.as_slice()).collect();
        let token_type_ids = Tensor::new(token_type_ids_refs, &inner.device).map_err(candle_err)?;
        let attention_mask =
            Tensor::new(attention_masks, &inner.device).map_err(candle_err)?;

        // Forward pass
        let embeddings = inner
            .model
            .forward(&token_ids, &token_type_ids, Some(&attention_mask))
            .map_err(candle_err)?;

        // CLS pooling (index 0) + L2 normalize for each item
        let mut results = Vec::with_capacity(batch_size);
        for i in 0..batch_size {
            let cls = embeddings.get(i).map_err(candle_err)?.get(0).map_err(candle_err)?;
            let norm = cls
                .sqr()
                .map_err(candle_err)?
                .sum_all()
                .map_err(candle_err)?
                .sqrt()
                .map_err(candle_err)?;
            let normalized = cls.broadcast_div(&norm).map_err(candle_err)?;
            results.push(normalized.to_vec1::<f32>().map_err(candle_err)?);
        }

        Ok(results)
    }
}

fn candle_err(e: candle_core::Error) -> kyomi_core::Error {
    kyomi_core::Error::Internal(format!("candle error: {e}"))
}

// ===========================================================================
// LazyEmbedding — deferred model loading for faster startup
// ===========================================================================

/// Lazy-loading wrapper around [`EmbeddingService`].
///
/// The server starts listening immediately while the embedding model loads
/// on a background thread (~65ms). Endpoints that need embeddings get a
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

// Pulls in build.rs's pure curl-argument logic so its tests actually run:
// `build.rs` is never a `cargo test` target on its own (Cargo only ever
// compiles and executes it as the build-script binary), so a `#[cfg(test)]`
// module written directly inside `build.rs` is silently never exercised.
// See `build_support.rs`'s module doc for the full explanation (KYO-510).
#[cfg(test)]
#[path = "../build_support.rs"]
mod build_support;

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

// ─── embed_passages_chunked bounding (KYO-644) ─────────────────────────────
//
// `embed_passages_chunked` is the fix for the runtime-starvation defect
// described on its own doc comment: an unbounded, un-offloaded embedding
// batch stalled the whole HTTP server for ~34s in production. This module
// covers the "bounded batch size" half of that fix directly, at the source
// — `crates/kyomi-knowledge/src/populate.rs`'s regression test covers the
// "moved off the async runtime" half end-to-end, through the real catalog
// population path.
#[cfg(test)]
mod chunking_tests {
    use super::*;

    /// `EMBED_BATCH_SIZE + 1` texts must produce exactly two `spawn_blocking`
    /// chunks (`EMBED_BATCH_SIZE`, then the 1 remainder) rather than one
    /// unbounded batch — the batch-size half of the KYO-644 fix. Every input
    /// must still come back embedded, in order.
    #[tokio::test]
    async fn embed_passages_chunked_splits_into_bounded_batches() {
        let svc = EmbeddingService::new().unwrap();
        let n = EMBED_BATCH_SIZE + 1;
        let texts: Vec<String> = (0..n).map(|i| format!("col_{i}")).collect();
        let refs: Vec<&str> = texts.iter().map(String::as_str).collect();

        let logs = kyomi_test_tracing::capture_tracing();
        let result = svc.embed_passages_chunked(&refs).await.unwrap();

        assert_eq!(result.len(), n, "every input text must come back embedded exactly once");

        let chunk_logs = logs.events_at(tracing::Level::DEBUG);
        assert_eq!(
            chunk_logs.len(),
            2,
            "{n} texts (EMBED_BATCH_SIZE + 1) must split into exactly two spawn_blocking \
             chunks, not one unbounded batch — captured debug events: {chunk_logs:?}"
        );
        assert!(
            chunk_logs[0].1.contains(&format!("chunk_size={EMBED_BATCH_SIZE}")),
            "first chunk must be exactly EMBED_BATCH_SIZE ({EMBED_BATCH_SIZE}): {chunk_logs:?}"
        );
        assert!(
            chunk_logs[1].1.contains("chunk_size=1"),
            "second chunk must be exactly the 1-text remainder: {chunk_logs:?}"
        );
    }
}
