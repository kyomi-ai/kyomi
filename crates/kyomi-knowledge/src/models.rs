// SPDX-License-Identifier: AGPL-3.0-or-later

//! Node and retrieval result types for the SQL-based knowledge pipeline.
//!
//! These mirror the types from `kyomi-graph/models.rs` but are independent
//! of FalkorDB. The SQL pipeline uses pgvector for embedding search and
//! produces the same context block format for the LLM.

use serde::{Deserialize, Serialize};

// -- Retrieval result types ------------------------------------------------

/// The kind of context entry returned from retrieval.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ContextEntryKind {
    Table,
    Learning,
    Metric,
}

impl ContextEntryKind {
    /// Section header for context block formatting.
    pub fn section_header(&self) -> &'static str {
        match self {
            Self::Table => "## Tables",
            Self::Learning => "## Learnings",
            Self::Metric => "## Metrics",
        }
    }

    /// Sort order -- tables first, then metrics, then learnings.
    pub fn sort_order(&self) -> u8 {
        match self {
            Self::Table => 0,
            Self::Metric => 1,
            Self::Learning => 2,
        }
    }
}

/// How this entry was discovered by the retrieval pipeline.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum RetrievalSource {
    /// Direct vector search hit.
    VectorSearch,
    /// Found via column-to-table score propagation.
    ColumnProxy,
    /// Discovered via graph-style expansion from a previously injected node.
    GraphExpansion,
}

/// A column that matched a vector search and contributed to its parent
/// table's score via the search proxy pattern.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MatchedColumn {
    /// Column name (e.g., "email").
    pub name: String,
    /// Data type (e.g., "VARCHAR").
    pub data_type: String,
    /// The similarity score this column achieved.
    pub score: f64,
}

/// A single entry in the assembled context block.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContextEntry {
    /// What kind of node this came from.
    pub kind: ContextEntryKind,
    /// Unique identifier for deduplication (e.g., table full_name, learning UUID).
    pub id: String,
    /// The display text for this entry.
    pub text: String,
    /// Combined score: max(name_score, desc_score, column_proxy_score).
    pub score: f64,
    /// How this entry was discovered.
    pub source: RetrievalSource,
    /// For Table entries: columns that matched the query (search proxies).
    /// Empty for Learning and Metric entries.
    pub matched_columns: Vec<MatchedColumn>,
}

/// The full context result from a retrieval query.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RetrievalResult {
    /// All context entries, sorted by section then score.
    pub entries: Vec<ContextEntry>,
    /// The formatted context block string.
    pub context_block: String,
    /// Approximate token count of the context block.
    pub token_count: usize,
}
