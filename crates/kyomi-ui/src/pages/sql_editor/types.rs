// SPDX-License-Identifier: AGPL-3.0-or-later

//! Shared types for the SQL Editor that cross the server/client boundary.
//!
//! All types here must be `Clone + Serialize + Deserialize` since they are
//! sent over the wire between server functions and WASM client code.
//!
//! These mirror the TypeScript definitions in
//! `apps/frontend/src/features/sql-editor/types.ts`.

use serde::de::{SeqAccess, Visitor};
use serde::{Deserialize, Deserializer, Serialize};

// ─────────────────────────────────────────────────────────────────────────────
// Query result types
// ─────────────────────────────────────────────────────────────────────────────

/// Column metadata from query execution.
///
/// Mirrors `ColumnMetadata` in the React types.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct ColumnMetadata {
    pub name: String,
    /// Simplified type (string, number, boolean, datetime).
    #[serde(rename = "type", default, skip_serializing_if = "Option::is_none")]
    pub col_type: Option<String>,
    /// NULLABLE, REQUIRED, REPEATED.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mode: Option<String>,
}

/// Deserialize `columns` from either `["col1", "col2"]` (plain strings) or
/// `[{name: "col1", type: "string", ...}]` (full metadata objects).
///
/// Plain strings are normalized to `ColumnMetadata { name: s, col_type: None, mode: None }`.
fn deserialize_columns<'de, D>(deserializer: D) -> Result<Vec<ColumnMetadata>, D::Error>
where
    D: Deserializer<'de>,
{
    /// A single element that is either a plain string or a full metadata object.
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum ColumnOrString {
        Meta(ColumnMetadata),
        Plain(String),
    }

    struct ColumnsVisitor;

    impl<'de> Visitor<'de> for ColumnsVisitor {
        type Value = Vec<ColumnMetadata>;

        fn expecting(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
            formatter.write_str("an array of strings or ColumnMetadata objects")
        }

        fn visit_seq<A>(self, mut seq: A) -> Result<Self::Value, A::Error>
        where
            A: SeqAccess<'de>,
        {
            let mut columns = Vec::with_capacity(seq.size_hint().unwrap_or(0));
            while let Some(item) = seq.next_element::<ColumnOrString>()? {
                columns.push(match item {
                    ColumnOrString::Meta(m) => m,
                    ColumnOrString::Plain(s) => ColumnMetadata {
                        name: s,
                        col_type: None,
                        mode: None,
                    },
                });
            }
            Ok(columns)
        }
    }

    deserializer.deserialize_seq(ColumnsVisitor)
}

/// Opaque handle for pagination — used by query service to fetch additional
/// pages from any datasource type.
///
/// Mirrors `QueryHandle` in the React types.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct QueryHandle {
    /// e.g. "bigquery", "postgres", "clickhouse"
    pub datasource_type: String,
    /// e.g. "production-postgres"
    pub datasource_slug: String,
    /// Original query (for re-execution).
    pub sql: String,
    /// BigQuery-specific: job ID for random page access.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub job_id: Option<String>,
}

/// Represents the result of a query execution.
///
/// ## Two data paths
///
/// - **Arrow path** (new): `data` is populated from `fetch_arrow_buffered`.
///   `rows` and `columns` are empty.  `ResultsTable` renders from `data`.
/// - **JSON path** (legacy server functions): `data` is `None`, `rows` and
///   `columns` are populated.  `ResultsTable` falls back to `rows`.
///
/// `data` is skipped during (de)serialization because `DataTable` is not
/// serde-serializable.  After restoring from localStorage, `data` is `None`
/// and the tab shows "Results expired — click to re-run".
///
/// Mirrors `QueryResult` in the React types.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct QueryResult {
    /// Arrow-backed columnar data for the current page.
    ///
    /// Populated by the Arrow path (`fetch_arrow_buffered`).
    /// `None` after deserialization from localStorage — the tab shows an
    /// expiry message and a re-run prompt.
    #[serde(skip)]
    pub data: Option<chartml_core::data::DataTable>,
    /// Column metadata (populated by the legacy JSON path only).
    #[serde(default, deserialize_with = "deserialize_columns")]
    pub columns: Vec<ColumnMetadata>,
    /// JSON rows (populated by the legacy JSON path only; empty on Arrow path).
    #[serde(default)]
    pub rows: Vec<Vec<serde_json::Value>>,
    /// Number of rows in the current page.
    pub row_count: usize,
    /// Total rows available (for server-side pagination).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub total_rows: Option<usize>,
    /// Unified pagination handle (works for all datasource types).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub query_handle: Option<QueryHandle>,
    /// Execution time in milliseconds.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub execution_time: Option<u64>,
    /// Bytes processed by the query engine.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bytes_processed: Option<u64>,
    /// Whether more pages are available beyond the current result set.
    #[serde(default)]
    pub has_more: bool,
}

/// `DataTable` does not implement `PartialEq`, so we compare all other fields
/// and treat two results as equal if everything except `data` matches.  This
/// is sufficient for Leptos `Memo` change-detection — the memo fires when any
/// metadata (rows, columns, pagination state) changes, which is what matters
/// for re-renders.
impl PartialEq for QueryResult {
    fn eq(&self, other: &Self) -> bool {
        // `data` is intentionally excluded — DataTable is not PartialEq.
        self.columns == other.columns
            && self.rows == other.rows
            && self.row_count == other.row_count
            && self.total_rows == other.total_rows
            && self.query_handle == other.query_handle
            && self.execution_time == other.execution_time
            && self.bytes_processed == other.bytes_processed
            && self.has_more == other.has_more
    }
}

/// Represents an error from query execution.
///
/// Mirrors `QueryError` in the React types.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct QueryError {
    pub message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub code: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub line: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub column: Option<u32>,
}

// ─────────────────────────────────────────────────────────────────────────────
// Query status
// ─────────────────────────────────────────────────────────────────────────────

/// Status of a query execution.
///
/// Mirrors `QueryStatus` in the React types.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum QueryStatus {
    #[default]
    Idle,
    Running,
    Streaming,
    Success,
    Error,
}

// ─────────────────────────────────────────────────────────────────────────────
// Visualization (ChartML)
// ─────────────────────────────────────────────────────────────────────────────

/// A ChartML visualization configuration attached to a result tab.
///
/// Mirrors `Visualization` in the React types.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Visualization {
    pub id: String,
    /// ChartML object (YAML parsed).
    pub chart_ml: serde_json::Value,
    /// YAML string representation.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub chart_ml_text: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    pub created_at: f64,
}

// ─────────────────────────────────────────────────────────────────────────────
// Tab types
// ─────────────────────────────────────────────────────────────────────────────

/// A single result tab containing query results and optional visualization.
///
/// Mirrors `ResultTab` in the React types.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ResultTab {
    pub id: String,
    pub label: String,
    pub query: String,
    pub status: QueryStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result: Option<QueryResult>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<QueryError>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub visualization: Option<Visualization>,
    /// Pinned tabs are kept beyond the 5-unpinned limit.
    pub pinned: bool,
    /// Stable color index for this tab (0-7, doesn't change when other tabs
    /// are removed).
    pub color_index: u8,
    pub created_at: f64,
    pub updated_at: f64,
    /// Tab was loaded from localStorage and needs data refresh.
    #[serde(default)]
    pub needs_refresh: bool,
    /// Datasource slug (e.g. "production-postgres").
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub datasource_slug: Option<String>,
    /// Datasource type (e.g. "bigquery", "postgres").
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub datasource_type: Option<String>,
}

// ─────────────────────────────────────────────────────────────────────────────
// Sort / table UI state
// ─────────────────────────────────────────────────────────────────────────────

/// Sort direction for a table column.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum SortDirection {
    Asc,
    Desc,
}

/// Sort configuration for a single table column.
///
/// Mirrors `ColumnSort` in the React types.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct ColumnSort {
    pub column: String,
    pub direction: SortDirection,
}

/// UI state for the results table (per-tab).
///
/// Mirrors `TableUIState` in the React types.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct TableUIState {
    pub sort_by: Vec<ColumnSort>,
    pub current_page: u32,
    pub page_size: u32,
}

impl Default for TableUIState {
    fn default() -> Self {
        Self {
            sort_by: Vec::new(),
            current_page: 1,
            page_size: 50,
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Sidebar tab
// ─────────────────────────────────────────────────────────────────────────────

/// Which sidebar panel is open on the right side.
///
/// The React store uses plain strings ("catalog", "history", "details").
/// We model these as an enum for type safety.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum SidebarTab {
    Catalog,
    History,
    Details,
}

// ─────────────────────────────────────────────────────────────────────────────
// Catalog types
// ─────────────────────────────────────────────────────────────────────────────

/// Type of node in the catalog tree.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum CatalogNodeType {
    Project,
    Dataset,
    Schema,
    Database,
    Table,
    View,
    /// Column nodes carry the column's data type as a string.
    Column(String),
}

/// A node in the catalog tree (project > dataset/schema > table > column).
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct CatalogNode {
    pub name: String,
    pub node_type: CatalogNodeType,
    pub children: Vec<CatalogNode>,
    /// Fully-qualified name (e.g. "project.dataset.table").
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub full_name: Option<String>,
}

// ─────────────────────────────────────────────────────────────────────────────
// History types
// ─────────────────────────────────────────────────────────────────────────────

/// A single entry in the query history list.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct QueryHistoryEntry {
    pub id: String,
    pub query_text: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub execution_time_ms: Option<i32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bytes_processed: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub row_count: Option<i32>,
    pub status: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error_message: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub datasource: Option<String>,
    pub is_saved: bool,
    pub created_at: String,
}

// ─────────────────────────────────────────────────────────────────────────────
// Data for creating a new tab (omits auto-generated fields)
// ─────────────────────────────────────────────────────────────────────────────

/// Input data for creating a new result tab. Fields that are auto-generated
/// (id, createdAt, updatedAt, pinned, colorIndex) are omitted — they are set
/// by `SqlEditorState::add_tab()`.
///
/// Mirrors `Omit<ResultTab, 'id' | 'createdAt' | 'updatedAt' | 'pinned' | 'colorIndex'>`.
#[derive(Clone, Debug)]
pub struct NewTabData {
    pub label: String,
    pub query: String,
    pub status: QueryStatus,
    pub result: Option<QueryResult>,
    pub error: Option<QueryError>,
    pub visualization: Option<Visualization>,
    pub needs_refresh: bool,
    pub datasource_slug: Option<String>,
    pub datasource_type: Option<String>,
}
