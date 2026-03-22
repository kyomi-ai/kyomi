// SPDX-License-Identifier: AGPL-3.0-or-later

//! SQL Editor page — types, state management, and UI components.

pub mod state;
pub mod types;

// Re-export the most commonly used items for convenience.
pub use state::SqlEditorState;
pub use types::{
    CatalogNode, CatalogNodeType, ColumnMetadata, ColumnSort, NewTabData, QueryError, QueryHandle,
    QueryHistoryEntry, QueryResult, QueryStatus, ResultTab, SidebarTab, SortDirection,
    TableUIState, Visualization,
};
