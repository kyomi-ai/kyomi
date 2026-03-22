// SPDX-License-Identifier: AGPL-3.0-or-later

//! SQL Editor page — types, state management, and UI components.

pub mod code_editor;
pub mod state;
pub mod status_bar;
pub mod types;

// Re-export the most commonly used items for convenience.
pub use code_editor::SqlCodeEditor;
pub use state::SqlEditorState;
pub use status_bar::{DryRunStatus, StatusBar};
pub use types::{
    CatalogNode, CatalogNodeType, ColumnMetadata, ColumnSort, NewTabData, QueryError, QueryHandle,
    QueryHistoryEntry, QueryResult, QueryStatus, ResultTab, SidebarTab, SortDirection,
    TableUIState, Visualization,
};
