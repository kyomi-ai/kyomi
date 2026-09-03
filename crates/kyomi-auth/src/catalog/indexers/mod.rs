// SPDX-License-Identifier: AGPL-3.0-or-later

//! Special catalog indexer implementations (no `kyomi_datasource` dependency).
//!
//! These are standalone services called by the scheduler or explicit requests:
//! - [`sample_data::SampleDataIndexer`] — shared ClickHouse sample data
//! - [`user_dataset::UserDatasetIndexer`] — workspace-scoped BigQuery user datasets
//! - [`bigquery_public::BigQueryPublicIndexer`] — shared BigQuery public datasets
//!
//! The 8 SQL-based provider indexers (postgres, mysql, etc.) and the BigQuery REST
//! indexer live in `kyomi-agent/src/catalog/indexers/` where `kyomi_datasource`
//! is available without a cyclic dependency.

pub mod bigquery_public;
pub(crate) mod bigquery_rest;
pub mod sample_data;
pub mod user_dataset;

/// Maximum results per BigQuery REST API listing page.
pub(crate) const BIGQUERY_API_MAX_RESULTS: &str = "1000";

// Re-export special indexer structs.
pub use bigquery_public::BigQueryPublicIndexer;
pub use sample_data::SampleDataIndexer;
pub use user_dataset::UserDatasetIndexer;
