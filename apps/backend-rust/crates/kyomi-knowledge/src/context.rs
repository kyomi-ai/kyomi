// SPDX-License-Identifier: AGPL-3.0-or-later

//! Conversation context state for delta injection.
//!
//! Tracks which entities have been injected into a conversation so that
//! subsequent turns only inject the delta (new entries). Previously injected
//! entities also serve as anchor points for expansion.
//!
//! # Persistence
//!
//! The context is stored in Redis as a JSON blob keyed by session ID:
//! `kg:ctx:{session_id}` with a 7-day TTL. Works correctly across multiple
//! backend replicas.
//!
//! # Lifecycle
//!
//! - Created empty when a new conversation starts (implicit -- `load_context`
//!   returns `Default` when the key is missing).
//! - Updated after each turn's injection via `record_injection`.
//! - Saved to Redis after each injection via `save_context`.
//! - Loaded on conversation resume via `load_context`.

use kyomi_core::KVPool;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;

use crate::expansion::{ExpansionHit, ExpansionHitKind};
use crate::models::{ContextEntry, ContextEntryKind, MatchedColumn, RetrievalResult, RetrievalSource};

/// Redis key prefix for conversation context.
const REDIS_KEY_PREFIX: &str = "kg:ctx";

/// TTL for context in Redis (7 days).
const CONTEXT_TTL_SECS: u64 = 7 * 24 * 60 * 60;

// ---------------------------------------------------------------------------
// ConversationContext
// ---------------------------------------------------------------------------

/// Tracks which entities have been injected into a conversation.
///
/// Used for:
/// 1. **Delta calculation** -- don't re-inject what's already in the conversation.
///    The `all_injected()` method returns a combined set for passing to
///    the retrieval pipeline.
/// 2. **Expansion anchors** -- expand from these entities to find related
///    context on subsequent turns.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ConversationContext {
    /// Table full_names already injected (e.g., "public.customers").
    pub injected_tables: HashSet<String>,
    /// Column identifiers already injected, format: "{table_full_name}#{column_name}"
    /// The '#' separator avoids ambiguity with dot-separated table names.
    pub injected_columns: HashSet<String>,
    /// Learning UUIDs already injected.
    pub injected_learnings: HashSet<String>,
    /// Metric names already injected (e.g., "MRR").
    pub injected_metrics: HashSet<String>,
}

impl ConversationContext {
    /// Create a new empty context (first turn of a conversation).
    pub fn new() -> Self {
        Self::default()
    }

    /// Returns a combined set of all injected IDs.
    ///
    /// This is the set passed to the retrieval pipeline as `already_injected`
    /// so that it skips entries already in the conversation history.
    pub fn all_injected(&self) -> HashSet<String> {
        let capacity = self.injected_tables.len()
            + self.injected_columns.len()
            + self.injected_learnings.len()
            + self.injected_metrics.len();
        let mut set = HashSet::with_capacity(capacity);
        set.extend(self.injected_tables.iter().cloned());
        set.extend(self.injected_columns.iter().cloned());
        set.extend(self.injected_learnings.iter().cloned());
        set.extend(self.injected_metrics.iter().cloned());
        set
    }

    /// Update context with newly injected entries from a retrieval result.
    ///
    /// After retrieval, pass the result here so that the next turn's
    /// retrieval will exclude these entries.
    pub fn record_injection(&mut self, result: &RetrievalResult) {
        for entry in &result.entries {
            match entry.kind {
                ContextEntryKind::Table => {
                    self.injected_tables.insert(entry.id.clone());
                    // Track matched columns for expansion anchors.
                    // Columns accumulate across turns: if the same table is re-retrieved
                    // with different matched columns, all columns become expansion anchors.
                    for col in &entry.matched_columns {
                        self.injected_columns
                            .insert(format!("{}#{}", entry.id, col.name));
                    }
                }
                ContextEntryKind::Learning => {
                    self.injected_learnings.insert(entry.id.clone());
                }
                ContextEntryKind::Metric => {
                    self.injected_metrics.insert(entry.id.clone());
                }
            }
        }
    }

    /// Check if context is empty (first turn of conversation).
    pub fn is_empty(&self) -> bool {
        self.injected_tables.is_empty()
            && self.injected_columns.is_empty()
            && self.injected_learnings.is_empty()
            && self.injected_metrics.is_empty()
    }
}

// -- Redis persistence -------------------------------------------------------

/// Save conversation context to Redis.
///
/// Uses SET with a 7-day TTL. Active sessions refresh the TTL on every turn.
pub async fn save_context(
    kv: &KVPool,
    session_id: &str,
    ctx: &ConversationContext,
) -> anyhow::Result<()> {
    let key = format!("{REDIS_KEY_PREFIX}:{session_id}");
    let json = serde_json::to_string(ctx)
        .map_err(|e| anyhow::anyhow!("failed to serialize ConversationContext: {e}"))?;

    kv.set(&key, &json, Some(CONTEXT_TTL_SECS))
        .await
        .map_err(|e| anyhow::anyhow!("KVStore SET kg:ctx failed: {e}"))?;

    tracing::debug!(
        session_id,
        tables = ctx.injected_tables.len(),
        columns = ctx.injected_columns.len(),
        learnings = ctx.injected_learnings.len(),
        metrics = ctx.injected_metrics.len(),
        "Saved conversation context"
    );

    Ok(())
}

/// Load conversation context from Redis.
///
/// Returns `ConversationContext::default()` if the key does not exist
/// (new conversation or expired session). This is intentional -- the
/// caller does not need to distinguish "new" from "expired".
pub async fn load_context(
    kv: &KVPool,
    session_id: &str,
) -> anyhow::Result<ConversationContext> {
    let key = format!("{REDIS_KEY_PREFIX}:{session_id}");

    let json = kv.get(&key)
        .await
        .map_err(|e| anyhow::anyhow!("KVStore GET kg:ctx failed: {e}"))?;

    match json {
        Some(data) => {
            let ctx: ConversationContext = serde_json::from_str(&data)
                .map_err(|e| anyhow::anyhow!("failed to deserialize ConversationContext: {e}"))?;
            tracing::debug!(
                session_id,
                tables = ctx.injected_tables.len(),
                columns = ctx.injected_columns.len(),
                learnings = ctx.injected_learnings.len(),
                metrics = ctx.injected_metrics.len(),
                "Loaded conversation context"
            );
            Ok(ctx)
        }
        None => {
            tracing::debug!(session_id, "No conversation context found, starting fresh");
            Ok(ConversationContext::default())
        }
    }
}

// -- Delta injection ---------------------------------------------------------

/// Self-contained retrieval turn: load context, retrieve, record injection,
/// save context, return results.
///
/// Loads fresh context from Redis, performs retrieval, records newly injected
/// entries, saves the updated context back to Redis, and returns both the
/// context block and the updated `ConversationContext`.
///
/// Returns an empty string as the context block if no new context was found.
///
/// This function is safe for sequential chat processing (one message at a time
/// per session). It is NOT safe for concurrent calls on the same session -- but
/// chat messages are always processed sequentially per conversation.
pub async fn retrieve_and_inject(
    kv: &KVPool,
    session_id: &str,
    db: &kyomi_core::db::DbPool,
    embed: &kyomi_embed::EmbeddingService,
    workspace_id: &str,
    query: &str,
    budget: usize,
) -> anyhow::Result<(String, ConversationContext)> {
    // 1. Load fresh context from KV store (or default for new conversations)
    let mut context = load_context(kv, session_id).await?;

    // 2. Build the already-injected set from context state
    let already_injected = context.all_injected();

    // 3. Create vector search implementation and run retrieval pipeline
    let vsearch = crate::vector_search::create_vector_search(db);
    let mut result = crate::retrieval::retrieve(
        vsearch.as_ref(), embed, workspace_id, query, &already_injected, Some(budget),
    ).await?;

    // 4. Expand from previously injected anchors to find related context
    if !context.is_empty() {
        let expansion_hits = crate::expansion::expand_from_anchors(
            db,
            workspace_id,
            &context.injected_tables,
            &context.injected_learnings,
            &context.injected_metrics,
            &already_injected,
        )
        .await;

        if !expansion_hits.is_empty() {
            merge_expansion_hits(&mut result, expansion_hits, budget);
        }
    }

    // 5. Record newly injected entries into conversation context
    context.record_injection(&result);

    // 6. Save context to KV store (refreshes TTL even when no new context)
    save_context(kv, session_id, &context).await?;

    // 7. Return the context block (may be empty if no relevant results)
    Ok((result.context_block, context))
}

/// Merge expansion hits into the retrieval result.
///
/// Converts `ExpansionHit` items to `ContextEntry` items, deduplicates by id
/// (keeping the higher score), re-sorts, and re-assembles the context block.
///
/// Column expansion hits contribute matched_columns to their parent table entry
/// rather than appearing as standalone entries.
fn merge_expansion_hits(
    result: &mut RetrievalResult,
    hits: Vec<ExpansionHit>,
    budget: usize,
) {
    let mut new_entries: Vec<ContextEntry> = Vec::new();
    // Columns from expansion: keyed by table_full_name -> Vec<MatchedColumn>
    let mut expansion_columns: std::collections::HashMap<String, Vec<MatchedColumn>> =
        std::collections::HashMap::new();

    for hit in hits {
        match hit.kind {
            ExpansionHitKind::Table {
                full_name,
                datasource_slug,
                description,
            } => {
                let text = crate::retrieval::format_table_entry(
                    &datasource_slug,
                    &full_name,
                    description.as_deref(),
                    &[],
                );
                new_entries.push(ContextEntry {
                    kind: ContextEntryKind::Table,
                    id: full_name,
                    text,
                    score: hit.score,
                    source: RetrievalSource::GraphExpansion,
                    matched_columns: Vec::new(),
                });
            }
            ExpansionHitKind::Learning { id, insight } => {
                new_entries.push(ContextEntry {
                    kind: ContextEntryKind::Learning,
                    id,
                    text: insight,
                    score: hit.score,
                    source: RetrievalSource::GraphExpansion,
                    matched_columns: Vec::new(),
                });
            }
            ExpansionHitKind::Column {
                name,
                table_full_name,
                data_type,
            } => {
                expansion_columns
                    .entry(table_full_name)
                    .or_default()
                    .push(MatchedColumn {
                        name,
                        data_type,
                        score: hit.score,
                    });
            }
        }
    }

    // Attach expansion columns to existing table entries (or create new ones)
    for (table_full_name, columns) in expansion_columns {
        if let Some(existing) = result.entries.iter_mut().find(|e| {
            e.kind == ContextEntryKind::Table && e.id == table_full_name
        }) {
            // Add columns to the existing table entry (dedup handled below)
            existing.matched_columns.extend(columns);
        } else if let Some(new_entry) = new_entries.iter_mut().find(|e| {
            e.kind == ContextEntryKind::Table && e.id == table_full_name
        }) {
            new_entry.matched_columns.extend(columns);
        }
        // If no table entry exists for these columns, they are orphaned
        // (the table was not retrieved or expanded). This is expected --
        // the table may have been filtered by the quality gate.
    }

    // Merge new entries into existing, dedup by id (higher score wins)
    for new_entry in new_entries {
        if let Some(existing) = result
            .entries
            .iter_mut()
            .find(|e| e.kind == new_entry.kind && e.id == new_entry.id)
        {
            if new_entry.score > existing.score {
                existing.score = new_entry.score;
                existing.source = new_entry.source;
            }
        } else {
            result.entries.push(new_entry);
        }
    }

    // Dedup matched_columns by name within each table entry
    for entry in result.entries.iter_mut().filter(|e| e.kind == ContextEntryKind::Table) {
        entry.matched_columns.sort_by(|a, b| {
            a.name.cmp(&b.name).then(
                b.score
                    .partial_cmp(&a.score)
                    .unwrap_or(std::cmp::Ordering::Equal),
            )
        });
        entry.matched_columns.dedup_by(|a, b| a.name == b.name);
        entry.matched_columns.sort_by(|a, b| {
            b.score
                .partial_cmp(&a.score)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
    }

    // Re-sort by kind then score
    result.entries.sort_by(|a, b| {
        a.kind
            .sort_order()
            .cmp(&b.kind.sort_order())
            .then(b.score.partial_cmp(&a.score).unwrap_or(std::cmp::Ordering::Equal))
    });

    // Re-assemble context block
    result.context_block = crate::retrieval::assemble_context_block(&result.entries, budget);
    result.token_count = (result.context_block.len() as f64 / crate::retrieval::CHARS_PER_TOKEN) as usize;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_context_is_empty() {
        let ctx = ConversationContext::new();
        assert!(ctx.is_empty());
        assert!(ctx.all_injected().is_empty());
    }

    #[test]
    fn record_injection_tracks_tables_and_columns() {
        let mut ctx = ConversationContext::new();
        let result = RetrievalResult {
            entries: vec![ContextEntry {
                kind: ContextEntryKind::Table,
                id: "public.users".to_string(),
                text: "prod / public.users: email (VARCHAR)".to_string(),
                score: 0.9,
                source: RetrievalSource::VectorSearch,
                matched_columns: vec![MatchedColumn {
                    name: "email".to_string(),
                    data_type: "VARCHAR".to_string(),
                    score: 0.85,
                }],
            }],
            context_block: "<knowledge_context>...</knowledge_context>".to_string(),
            token_count: 50,
        };

        ctx.record_injection(&result);

        assert!(!ctx.is_empty());
        assert!(ctx.injected_tables.contains("public.users"));
        assert!(ctx.injected_columns.contains("public.users#email"));

        let all = ctx.all_injected();
        assert!(all.contains("public.users"));
        assert!(all.contains("public.users#email"));
    }

    #[test]
    fn record_injection_tracks_learnings_and_metrics() {
        let mut ctx = ConversationContext::new();
        let result = RetrievalResult {
            entries: vec![
                ContextEntry {
                    kind: ContextEntryKind::Learning,
                    id: "550e8400-e29b-41d4-a716-446655440000".to_string(),
                    text: "Exclude cancelled subs from MRR".to_string(),
                    score: 0.8,
                    source: RetrievalSource::VectorSearch,
                    matched_columns: vec![],
                },
                ContextEntry {
                    kind: ContextEntryKind::Metric,
                    id: "MRR".to_string(),
                    text: "MRR: Monthly Recurring Revenue".to_string(),
                    score: 0.95,
                    source: RetrievalSource::VectorSearch,
                    matched_columns: vec![],
                },
            ],
            context_block: "<knowledge_context>...</knowledge_context>".to_string(),
            token_count: 80,
        };

        ctx.record_injection(&result);

        assert!(ctx
            .injected_learnings
            .contains("550e8400-e29b-41d4-a716-446655440000"));
        assert!(ctx.injected_metrics.contains("MRR"));
        assert!(ctx.injected_tables.is_empty());
    }

    #[test]
    fn all_injected_combines_all_sets() {
        let mut ctx = ConversationContext::new();
        ctx.injected_tables.insert("public.users".to_string());
        ctx.injected_columns
            .insert("public.users#email".to_string());
        ctx.injected_learnings.insert("learn-abc".to_string());
        ctx.injected_metrics.insert("MRR".to_string());

        let all = ctx.all_injected();
        assert_eq!(all.len(), 4);
        assert!(all.contains("public.users"));
        assert!(all.contains("public.users#email"));
        assert!(all.contains("learn-abc"));
        assert!(all.contains("MRR"));
    }

    #[test]
    fn record_injection_is_idempotent() {
        let mut ctx = ConversationContext::new();
        let result = RetrievalResult {
            entries: vec![ContextEntry {
                kind: ContextEntryKind::Table,
                id: "public.users".to_string(),
                text: "public.users".to_string(),
                score: 0.9,
                source: RetrievalSource::VectorSearch,
                matched_columns: vec![],
            }],
            context_block: String::new(),
            token_count: 0,
        };

        ctx.record_injection(&result);
        ctx.record_injection(&result);

        assert_eq!(ctx.injected_tables.len(), 1);
    }

    #[test]
    fn serialization_roundtrip() {
        let mut ctx = ConversationContext::new();
        ctx.injected_tables.insert("public.users".to_string());
        ctx.injected_columns
            .insert("public.users#email".to_string());
        ctx.injected_learnings.insert("learn-abc".to_string());
        ctx.injected_metrics.insert("MRR".to_string());

        let json = serde_json::to_string(&ctx).unwrap();
        let deserialized: ConversationContext = serde_json::from_str(&json).unwrap();

        assert_eq!(ctx.injected_tables, deserialized.injected_tables);
        assert_eq!(ctx.injected_columns, deserialized.injected_columns);
        assert_eq!(ctx.injected_learnings, deserialized.injected_learnings);
        assert_eq!(ctx.injected_metrics, deserialized.injected_metrics);
    }

    #[test]
    fn record_injection_accumulates_columns_across_calls() {
        let mut ctx = ConversationContext::new();

        // First injection: table with "email" column
        let result1 = RetrievalResult {
            entries: vec![ContextEntry {
                kind: ContextEntryKind::Table,
                id: "public.users".to_string(),
                text: "public.users".to_string(),
                score: 0.9,
                source: RetrievalSource::VectorSearch,
                matched_columns: vec![MatchedColumn {
                    name: "email".to_string(),
                    data_type: "VARCHAR".to_string(),
                    score: 0.85,
                }],
            }],
            context_block: String::new(),
            token_count: 0,
        };
        ctx.record_injection(&result1);

        // Second injection: same table with "name" column
        let result2 = RetrievalResult {
            entries: vec![ContextEntry {
                kind: ContextEntryKind::Table,
                id: "public.users".to_string(),
                text: "public.users".to_string(),
                score: 0.8,
                source: RetrievalSource::GraphExpansion,
                matched_columns: vec![MatchedColumn {
                    name: "name".to_string(),
                    data_type: "VARCHAR".to_string(),
                    score: 0.7,
                }],
            }],
            context_block: String::new(),
            token_count: 0,
        };
        ctx.record_injection(&result2);

        // Both columns should be tracked (accumulation, not replacement)
        assert_eq!(ctx.injected_tables.len(), 1);
        assert_eq!(ctx.injected_columns.len(), 2);
        assert!(ctx.injected_columns.contains("public.users#email"));
        assert!(ctx.injected_columns.contains("public.users#name"));
    }

    #[test]
    fn default_context_is_empty() {
        let ctx = ConversationContext::default();
        assert!(ctx.is_empty());
        assert_eq!(ctx.all_injected().len(), 0);
    }
}
