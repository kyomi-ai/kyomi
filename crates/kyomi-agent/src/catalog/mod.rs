// SPDX-License-Identifier: AGPL-3.0-or-later

//! SQL-based catalog indexing — provider indexers and orchestration service.
//!
//! This module was moved from `kyomi-auth/src/catalog/` to break the cyclic
//! dependency: `kyomi-auth` → `kyomi-datasource` → `kyomi-auth`. Since
//! `kyomi-agent` depends on both `kyomi-auth` AND `kyomi-datasource`, the
//! SQL-based indexers (which need `DatasourceProvider`) compile cleanly here.
//!
//! ## What lives here
//!
//! - **[`traits`]** — `CatalogIndexer` and `SQLCatalogIndexer` traits,
//!   `IndexerContext`, credential resolution, and `index_catalog_sql` template method
//! - **[`indexing_service`]** — `CatalogIndexingService` dispatcher
//! - **[`indexers`]** — 8 SQL-based provider indexers + BigQuery REST indexer
//!
//! ## What stays in `kyomi-auth::catalog`
//!
//! - `types` — `CatalogIndexResult`, `TableEntry`, `ColumnEntry`, `SearchEntry`
//! - `helpers` — `cache_table`, `archive_missing_tables`, status updates (no `kyomi_datasource_server` dep)
//! - `search_entries` — weighted search entry generation
//! - `indexers::sample_data`, `indexers::user_dataset`, `indexers::bigquery_public` — special indexers

pub mod indexers;
pub mod indexing_service;
pub mod traits;
