// SPDX-License-Identifier: AGPL-3.0-or-later

//! SQL Editor page — types, state management, and UI components.

pub mod catalog_tree;
pub mod code_editor;
pub mod query_history;
pub mod results_container;
pub mod results_table;
pub mod sidebar;
pub mod state;
pub mod status_bar;
pub mod tab_bar;
pub mod types;

// Re-export the most commonly used items for convenience.
pub use catalog_tree::CatalogTree;
pub use code_editor::SqlCodeEditor;
pub use query_history::QueryHistory;
pub use results_container::ResultsContainer;
pub use results_table::ResultsTable;
pub use sidebar::SqlEditorSidebar;
pub use state::SqlEditorState;
pub use status_bar::{DryRunStatus, StatusBar};
pub use tab_bar::TabBar;
pub use types::{
    CatalogNode, CatalogNodeType, ColumnMetadata, ColumnSort, NewTabData, QueryError, QueryHandle,
    QueryHistoryEntry, QueryResult, QueryStatus, ResultTab, SidebarTab, SortDirection,
    TableUIState, Visualization,
};
