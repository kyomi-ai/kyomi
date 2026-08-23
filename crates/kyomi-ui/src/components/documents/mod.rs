// SPDX-License-Identifier: AGPL-3.0-or-later

//! Shared document components — used by both dashboards and knowledge pages.
//!
//! These components were extracted from `dashboards_list.rs` to enable
//! code sharing across all document list pages.

pub mod document_card_grid;
pub mod search_sort_bar;

pub use document_card_grid::{format_relative_time, DocumentCardGrid, DocumentCardGridSkeleton};
pub(crate) use document_card_grid::sort_document_list_items;
pub use search_sort_bar::SearchSortBar;
