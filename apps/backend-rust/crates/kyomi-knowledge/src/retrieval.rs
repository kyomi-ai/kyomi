// SPDX-License-Identifier: AGPL-3.0-or-later

//! Retrieval pipeline -- vector search + SQL anchor expansion
//! + column-to-table propagation.
//!
//! Entry point: `retrieve(vsearch, embed, workspace_id, query, already_injected, token_budget)`
//!
//! Pipeline:
//! 1. Embed query using `embed_query()` (BGE asymmetric)
//! 2. Search all vector indexes in parallel via VectorSearch trait
//! 3. SQL anchor expansion from previously injected nodes
//! 4. Merge results: deduplicate by node ID, best score wins per node
//! 5. Column->Table score propagation (BEFORE score floor)
//! 6. Quality gates: MIN_SIMILARITY floor, then budget ceiling
//! 7. Return ranked results

use crate::models::{
    ContextEntry, ContextEntryKind, MatchedColumn, RetrievalResult, RetrievalSource,
};
use crate::vector_search::VectorSearch;
use kyomi_embed::EmbeddingService;
use std::collections::{HashMap, HashSet};

/// Default top-K for each vector index search.
const MAX_RESULTS_PER_INDEX: usize = 10;

/// Minimum cosine similarity to include a result.
/// Below this, results are noise -- not relevant to the query.
const MIN_SIMILARITY: f64 = 0.25;

/// Per-turn token budget for new context injection.
pub const PER_TURN_TOKEN_BUDGET: usize = 512;

/// Approximate characters per token (conservative for English text).
pub(crate) const CHARS_PER_TOKEN: f64 = 4.0;

/// Maximum number of matched columns to include per table entry.
const MAX_COLUMNS_PER_TABLE: usize = 10;

/// Compute a recency weight for a metric based on its age.
///
/// - < 7 days old: 1.0 (full weight)
/// - < 30 days old: 0.8
/// - < 90 days old: 0.5
/// - >= 90 days old: 0.3
fn recency_weight(created_at: &chrono::DateTime<chrono::Utc>) -> f64 {
    let days = (chrono::Utc::now() - *created_at).num_days().max(0);
    if days < 7 {
        1.0
    } else if days < 30 {
        0.8
    } else if days < 90 {
        0.5
    } else {
        0.3
    }
}

/// A table candidate being assembled from multiple index hits.
#[derive(Debug)]
struct TableCandidate {
    full_name: String,
    datasource_slug: String,
    description: Option<String>,
    /// Best score from table name/desc embedding or column propagation.
    own_score: f64,
    /// Source of the own score.
    own_source: RetrievalSource,
    /// Columns that matched the query (from column index searches).
    matched_columns: Vec<ColumnHit>,
}

/// A column hit from vector search, used as a search proxy.
#[derive(Debug)]
struct ColumnHit {
    name: String,
    table_full_name: String,
    data_type: String,
    score: f64,
}

/// Retrieve relevant context for a user message using vector search.
///
/// Returns scored, budget-constrained context entries ready for injection.
pub async fn retrieve(
    vsearch: &dyn VectorSearch,
    embed: &EmbeddingService,
    workspace_id: &str,
    query: &str,
    already_injected: &HashSet<String>,
    token_budget: Option<usize>,
) -> anyhow::Result<RetrievalResult> {
    let budget = token_budget.unwrap_or(PER_TURN_TOKEN_BUDGET);

    // 1. Embed the query with BGE prefix
    let embed_start = std::time::Instant::now();
    let query_vec = embed
        .embed_query(query)
        .map_err(|e| anyhow::anyhow!("embedding failed: {e}"))?;
    tracing::debug!(elapsed_ms = embed_start.elapsed().as_millis(), "Query embedding");

    // 2. Search all vector indexes in parallel
    let search_start = std::time::Instant::now();
    let (table_result, column_result, learning_result, metric_result) = tokio::join!(
        vsearch.search_tables(workspace_id, &query_vec, MAX_RESULTS_PER_INDEX),
        vsearch.search_columns(workspace_id, &query_vec, MAX_RESULTS_PER_INDEX),
        vsearch.search_learnings(workspace_id, &query_vec, MAX_RESULTS_PER_INDEX),
        vsearch.search_metrics(workspace_id, &query_vec, MAX_RESULTS_PER_INDEX),
    );
    tracing::debug!(elapsed_ms = search_start.elapsed().as_millis(), "Vector search (4 indexes)");

    let table_hits = table_result.unwrap_or_default();
    let column_hits = column_result.unwrap_or_default();
    let learning_hits = learning_result.unwrap_or_default();
    let metric_hits = metric_result.unwrap_or_default();

    // 3. Merge table hits from vector search (best score wins per node)
    let propagation_start = std::time::Instant::now();
    let mut table_candidates: HashMap<String, TableCandidate> = HashMap::new();

    for row in &table_hits {
        let full_name = kyomi_core::build_full_table_name(&row.project_id, &row.dataset_id, &row.table_id);
        let description = extract_description(&row.table_metadata);
        let entry = table_candidates.entry(full_name.clone()).or_insert_with(|| {
            TableCandidate {
                full_name: full_name.clone(),
                datasource_slug: row.datasource_slug.clone(),
                description: description.clone(),
                own_score: 0.0,
                own_source: RetrievalSource::VectorSearch,
                matched_columns: Vec::new(),
            }
        });
        if row.score > entry.own_score {
            entry.own_score = row.score;
            entry.own_source = RetrievalSource::VectorSearch;
        }
        // Update metadata if we have it and entry doesn't
        if entry.datasource_slug.is_empty() && !row.datasource_slug.is_empty() {
            entry.datasource_slug = row.datasource_slug.clone();
        }
        if entry.description.is_none() {
            entry.description = description;
        }
    }

    // 4. Column->Table score propagation (BEFORE score floor)
    let column_hit_items: Vec<ColumnHit> = column_hits
        .into_iter()
        .map(|c| {
            let full_name = kyomi_core::build_full_table_name(&c.project_id, &c.dataset_id, &c.table_id);
            ColumnHit {
                name: c.column_name,
                table_full_name: full_name,
                data_type: c.data_type,
                score: c.score,
            }
        })
        .collect();

    // Deduplicate column hits by (table_full_name, column_name), best score wins
    let mut deduped_columns: HashMap<(String, String), ColumnHit> = HashMap::new();
    for col in column_hit_items {
        let key = (col.table_full_name.clone(), col.name.clone());
        let entry = deduped_columns.entry(key);
        match entry {
            std::collections::hash_map::Entry::Occupied(mut e) => {
                if col.score > e.get().score {
                    e.insert(col);
                }
            }
            std::collections::hash_map::Entry::Vacant(e) => {
                e.insert(col);
            }
        }
    }

    // Propagate vector search column scores to parent tables
    for ((_table, _col_name), col) in &deduped_columns {
        let entry = table_candidates
            .entry(col.table_full_name.clone())
            .or_insert_with(|| TableCandidate {
                full_name: col.table_full_name.clone(),
                datasource_slug: String::new(),
                description: None,
                own_score: 0.0,
                own_source: RetrievalSource::ColumnProxy,
                matched_columns: Vec::new(),
            });

        // max(table_score, column_score)
        if col.score > entry.own_score {
            entry.own_score = col.score;
            entry.own_source = RetrievalSource::ColumnProxy;
        }

        entry.matched_columns.push(ColumnHit {
            name: col.name.clone(),
            table_full_name: col.table_full_name.clone(),
            data_type: col.data_type.clone(),
            score: col.score,
        });
    }
    tracing::debug!(elapsed_ms = propagation_start.elapsed().as_millis(), "Score propagation");

    // 5. Apply quality gates and build final entries
    let assembly_start = std::time::Instant::now();
    let mut entries: Vec<ContextEntry> = Vec::new();

    // Tables (with column information)
    for candidate in table_candidates.values() {
        if candidate.own_score < MIN_SIMILARITY {
            continue;
        }
        if already_injected.contains(&candidate.full_name) {
            continue;
        }

        // Sort matched columns by score descending, take relevant ones.
        let mut matched: Vec<MatchedColumn> = candidate
            .matched_columns
            .iter()
            .filter(|c| c.score >= MIN_SIMILARITY)
            .map(|c| MatchedColumn {
                name: c.name.clone(),
                data_type: c.data_type.clone(),
                score: c.score,
            })
            .collect();

        // Deduplicate matched columns by name (expansion + vector may find the same column)
        matched.sort_by(|a, b| {
            a.name.cmp(&b.name).then(
                b.score
                    .partial_cmp(&a.score)
                    .unwrap_or(std::cmp::Ordering::Equal),
            )
        });
        matched.dedup_by(|a, b| a.name == b.name);

        // Re-sort by score descending for display
        matched.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap_or(std::cmp::Ordering::Equal));

        // Limit to top N columns per table to avoid oversized context
        if matched.len() > MAX_COLUMNS_PER_TABLE {
            matched.truncate(MAX_COLUMNS_PER_TABLE);
        }

        let text = format_table_entry(
            &candidate.datasource_slug,
            &candidate.full_name,
            candidate.description.as_deref(),
            &matched,
        );

        entries.push(ContextEntry {
            kind: ContextEntryKind::Table,
            id: candidate.full_name.clone(),
            text,
            score: candidate.own_score,
            source: candidate.own_source,
            matched_columns: matched,
        });
    }

    // Learnings (no recency penalty -- human-curated knowledge is always relevant)
    for row in &learning_hits {
        if row.score < MIN_SIMILARITY {
            continue;
        }
        if already_injected.contains(&row.learning_id) {
            continue;
        }
        entries.push(ContextEntry {
            kind: ContextEntryKind::Learning,
            id: row.learning_id.clone(),
            text: row.insight.clone(),
            score: row.score,
            source: RetrievalSource::VectorSearch,
            matched_columns: Vec::new(),
        });
    }

    // Metrics -- apply recency weighting
    for row in &metric_hits {
        let weighted_score = row.score * recency_weight(&row.created_at);
        if weighted_score < MIN_SIMILARITY {
            continue;
        }
        let id = row.learning_id.clone();
        if already_injected.contains(&id) {
            continue;
        }
        let name = row.title.as_deref().unwrap_or("Metric");
        entries.push(ContextEntry {
            kind: ContextEntryKind::Metric,
            id,
            text: format!("{name}: {}", row.insight),
            score: weighted_score,
            source: RetrievalSource::VectorSearch,
            matched_columns: Vec::new(),
        });
    }

    // Sort by score descending (across all kinds) for budget allocation
    entries.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap_or(std::cmp::Ordering::Equal));

    // Apply token budget ceiling
    let context_block = assemble_context_block(&entries, budget);
    let token_count = (context_block.len() as f64 / CHARS_PER_TOKEN) as usize;

    if token_count > budget {
        tracing::warn!(token_count, budget, "Context block may exceed token budget");
    }

    tracing::debug!(elapsed_ms = assembly_start.elapsed().as_millis(), entries = entries.len(), token_count, "Context assembly");

    Ok(RetrievalResult {
        entries,
        context_block,
        token_count,
    })
}

// -- Helper to extract description from table_metadata JSON ------------------

fn extract_description(metadata: &Option<serde_json::Value>) -> Option<String> {
    metadata
        .as_ref()
        .and_then(|m| m.get("description"))
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string())
}

// -- Context block assembly ------------------------------------------------

/// Format a table entry for the context block.
///
/// Format: `datasource_slug / full_name -- description: col1 (type), col2 (type), ...`
pub(crate) fn format_table_entry(
    datasource_slug: &str,
    full_name: &str,
    description: Option<&str>,
    columns: &[MatchedColumn],
) -> String {
    let mut text = if datasource_slug.is_empty() {
        full_name.to_string()
    } else {
        format!("{datasource_slug} / {full_name}")
    };

    if let Some(desc) = description {
        text.push_str(&format!(" -- {desc}"));
    }

    if !columns.is_empty() {
        let col_list: Vec<String> = columns
            .iter()
            .map(|c| format!("{} ({})", c.name, c.data_type))
            .collect();
        text.push_str(&format!(": {}", col_list.join(", ")));
    }

    text
}

/// Assemble the context block string from sorted entries, respecting token budget.
///
/// Entries are grouped by kind (section headers), and capped by the budget.
/// The budget is a ceiling, not a target -- we don't scrape the bottom of the
/// barrel to fill it.
pub(crate) fn assemble_context_block(entries: &[ContextEntry], token_budget: usize) -> String {
    if entries.is_empty() {
        return String::new();
    }

    let char_budget = (token_budget as f64 * CHARS_PER_TOKEN) as usize;

    // Select entries by score (highest first, regardless of kind) until budget is filled.
    // This ensures high-relevance learnings aren't starved by lower-relevance tables.
    let mut by_score: Vec<&ContextEntry> = entries.iter().collect();
    by_score.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap_or(std::cmp::Ordering::Equal));

    let overhead = "<knowledge_context>\n</knowledge_context>".len();
    let mut budget_remaining = char_budget.saturating_sub(overhead);
    let mut selected: Vec<&ContextEntry> = Vec::new();

    for entry in &by_score {
        let line_len = format!("- {}\n", entry.text).len();
        if line_len > budget_remaining {
            continue; // skip this entry but try smaller ones
        }
        budget_remaining -= line_len;
        selected.push(entry);
    }

    if selected.is_empty() {
        return String::new();
    }

    // Group selected entries by kind for readable output (section headers).
    selected.sort_by(|a, b| {
        a.kind
            .sort_order()
            .cmp(&b.kind.sort_order())
            .then(b.score.partial_cmp(&a.score).unwrap_or(std::cmp::Ordering::Equal))
    });

    let mut block = String::from("<knowledge_context>\n");
    let mut current_kind: Option<ContextEntryKind> = None;

    for entry in &selected {
        if current_kind != Some(entry.kind) {
            block.push_str(&format!("\n{}\n", entry.kind.section_header()));
            current_kind = Some(entry.kind);
        }
        block.push_str(&format!("- {}\n", entry.text));
    }

    block.push_str("</knowledge_context>");
    block
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_recency_weight_recent() {
        // 1-day-old metric: full weight
        let yesterday = chrono::Utc::now() - chrono::Duration::days(1);
        assert!((recency_weight(&yesterday) - 1.0).abs() < f64::EPSILON);
    }

    #[test]
    fn test_recency_weight_two_weeks() {
        // 15-day-old metric: 0.8
        let dt = chrono::Utc::now() - chrono::Duration::days(15);
        assert!((recency_weight(&dt) - 0.8).abs() < f64::EPSILON);
    }

    #[test]
    fn test_recency_weight_two_months() {
        // 60-day-old metric: 0.5
        let dt = chrono::Utc::now() - chrono::Duration::days(60);
        assert!((recency_weight(&dt) - 0.5).abs() < f64::EPSILON);
    }

    #[test]
    fn test_recency_weight_old() {
        // 120-day-old metric: 0.3
        let dt = chrono::Utc::now() - chrono::Duration::days(120);
        assert!((recency_weight(&dt) - 0.3).abs() < f64::EPSILON);
    }

    #[test]
    fn test_format_table_entry_with_columns() {
        let columns = vec![
            MatchedColumn {
                name: "email".to_string(),
                data_type: "VARCHAR".to_string(),
                score: 0.85,
            },
            MatchedColumn {
                name: "created_at".to_string(),
                data_type: "TIMESTAMP".to_string(),
                score: 0.60,
            },
        ];
        let text = format_table_entry(
            "production-postgres",
            "public.users",
            None,
            &columns,
        );
        assert_eq!(
            text,
            "production-postgres / public.users: email (VARCHAR), created_at (TIMESTAMP)"
        );
    }

    #[test]
    fn test_format_table_entry_no_columns() {
        let text = format_table_entry(
            "prod-bq",
            "analytics.events",
            Some("Event tracking table"),
            &[],
        );
        assert_eq!(
            text,
            "prod-bq / analytics.events -- Event tracking table"
        );
    }

    #[test]
    fn test_format_table_entry_no_slug() {
        let text = format_table_entry("", "public.users", None, &[]);
        assert_eq!(text, "public.users");
    }

    #[test]
    fn test_assemble_context_block_basic() {
        let entries = vec![
            ContextEntry {
                kind: ContextEntryKind::Table,
                id: "public.users".to_string(),
                text: "prod / public.users: email (VARCHAR)".to_string(),
                score: 0.90,
                source: RetrievalSource::VectorSearch,
                matched_columns: vec![],
            },
            ContextEntry {
                kind: ContextEntryKind::Learning,
                id: "learn-123".to_string(),
                text: "Exclude cancelled subs from MRR".to_string(),
                score: 0.85,
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
        ];

        let block = assemble_context_block(&entries, 2048);
        assert!(block.contains("<knowledge_context>"));
        assert!(block.contains("</knowledge_context>"));
        assert!(block.contains("## Tables"));
        assert!(block.contains("## Metrics"));
        assert!(block.contains("## Learnings"));
        assert!(block.contains("prod / public.users"));
        assert!(block.contains("MRR: Monthly Recurring Revenue"));
        assert!(block.contains("Exclude cancelled subs from MRR"));
    }

    #[test]
    fn test_assemble_context_block_empty() {
        let block = assemble_context_block(&[], 2048);
        assert!(block.is_empty());
    }

    #[test]
    fn test_assemble_context_block_respects_budget() {
        let entries = vec![
            ContextEntry {
                kind: ContextEntryKind::Learning,
                id: "a".to_string(),
                text: "A".repeat(1000),
                score: 0.9,
                source: RetrievalSource::VectorSearch,
                matched_columns: vec![],
            },
            ContextEntry {
                kind: ContextEntryKind::Learning,
                id: "b".to_string(),
                text: "B".repeat(1000),
                score: 0.8,
                source: RetrievalSource::VectorSearch,
                matched_columns: vec![],
            },
        ];

        // Budget of 300 tokens ~ 1200 chars -- should only fit the first entry
        let block = assemble_context_block(&entries, 300);
        assert!(block.contains(&"A".repeat(100)));
        assert!(!block.contains(&"B".repeat(100)));
    }

    #[test]
    fn test_assemble_high_score_learning_not_starved_by_tables() {
        let entries = vec![
            ContextEntry {
                kind: ContextEntryKind::Table,
                id: "table1".to_string(),
                text: "T".repeat(400),
                score: 0.50,
                source: RetrievalSource::VectorSearch,
                matched_columns: vec![],
            },
            ContextEntry {
                kind: ContextEntryKind::Table,
                id: "table2".to_string(),
                text: "U".repeat(400),
                score: 0.45,
                source: RetrievalSource::VectorSearch,
                matched_columns: vec![],
            },
            ContextEntry {
                kind: ContextEntryKind::Learning,
                id: "learning1".to_string(),
                text: "Important learning".to_string(),
                score: 0.90,
                source: RetrievalSource::VectorSearch,
                matched_columns: vec![],
            },
        ];

        let block = assemble_context_block(&entries, 300);
        assert!(block.contains("Important learning"), "High-score learning must be included");
        assert!(block.contains(&"T".repeat(100)), "Highest-score table must be included");
    }

    #[test]
    fn test_extract_description() {
        let meta = Some(serde_json::json!({"description": "User accounts table"}));
        assert_eq!(extract_description(&meta), Some("User accounts table".to_string()));

        let empty_desc = Some(serde_json::json!({"description": ""}));
        assert_eq!(extract_description(&empty_desc), None);

        let no_desc = Some(serde_json::json!({"other_field": "value"}));
        assert_eq!(extract_description(&no_desc), None);

        assert_eq!(extract_description(&None), None);
    }
}
