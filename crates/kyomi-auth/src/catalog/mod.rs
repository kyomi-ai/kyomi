// SPDX-License-Identifier: AGPL-3.0-or-later

//! Catalog indexing system — table discovery, caching, and embedding for semantic search.
//!
//! This module provides the `kyomi_datasource`-independent parts of the catalog
//! infrastructure. The SQL-based indexers and `CatalogIndexingService` that depend
//! on `kyomi_datasource` live in `kyomi-agent/src/catalog/`.
//!
//! ## Architecture
//!
//! - **[`types`]** — Core types: `CatalogIndexResult`, `SearchEntry`, `TableEntry`, `ColumnEntry`
//! - **[`search_entries`]** — 4-tier weighted search entry generation
//! - **[`helpers`]** — Shared helpers (caching, archiving, status updates, `IndexerContext`)
//!
//! ## Special Indexers (Phase 7G)
//!
//! These are standalone services called by the scheduler or explicit requests:
//! - [`indexers::sample_data::SampleDataIndexer`] — shared ClickHouse sample data
//! - [`indexers::user_dataset::UserDatasetIndexer`] — workspace-scoped BigQuery user datasets
//! - [`indexers::bigquery_public::BigQueryPublicIndexer`] — shared BigQuery public datasets

pub mod helpers;
pub mod indexers;
pub mod search_entries;
pub mod sql_helpers;
pub mod types;
