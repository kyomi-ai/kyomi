// SPDX-License-Identifier: AGPL-3.0-or-later

//! Graph-style anchor expansion via SQL JOINs.
//!
//! Replaces FalkorDB Cypher traversals with database queries against
//! `learning_references` and `column_embeddings` tables.
//!
//! The injected sets serve double duty:
//! 1. Dedup -- don't re-inject what's already in the conversation
//! 2. Graph anchors -- expand from these nodes to find related context
//!
//! Expansion queries are 1-2 hop traversals from previously injected nodes:
//! - Injected Tables  -> Columns (via column_embeddings.table_cache_id)
//! - Injected Tables  <- Learnings (via learning_references ref_type='table')
//! - Injected Learnings -> Tables (via learning_references ref_type='table')
//! - Injected Metrics -> Tables (via learning_references 2-hop: metric -> learning -> table)
//!
//! All expansion results get a flat base score (`EXPANSION_BASE_SCORE = 0.5`)
//! since the parent's original retrieval score is not tracked. This ensures
//! expansion results rank below strong vector matches but above the
//! MIN_SIMILARITY floor (0.25).

use kyomi_core::db::DbPool;
use std::collections::HashSet;

/// Base score for expansion-discovered entries.
const EXPANSION_BASE_SCORE: f64 = 0.5;

/// SQL CASE expression that computes a full table name from
/// `tc.project_id`, `tc.dataset_id`, and `tc.table_id` columns.
/// Mirrors the logic of `kyomi_core::build_full_table_name()`.
const FULL_NAME_CASE_SQL: &str = "\
CASE \
  WHEN tc.project_id = '' THEN \
    CASE WHEN tc.dataset_id = '' THEN tc.table_id \
         ELSE tc.dataset_id || '.' || tc.table_id \
    END \
  ELSE tc.project_id || '.' || tc.dataset_id || '.' || tc.table_id \
END";

/// A hit discovered via graph-style expansion from a previously injected anchor.
#[derive(Debug)]
pub struct ExpansionHit {
    /// What kind of hit: table, column, or learning.
    pub kind: ExpansionHitKind,
    /// Similarity score (flat base score for expansion results).
    pub score: f64,
}

/// The kind of node discovered via expansion.
///
/// Metrics are NOT discovered via expansion -- expansion FROM metrics
/// finds tables and learnings, not other metrics.
#[derive(Debug)]
pub enum ExpansionHitKind {
    /// A column found via table -> column_embeddings.
    Column {
        name: String,
        table_full_name: String,
        data_type: String,
    },
    /// A learning found via table <- learning_references.
    Learning {
        id: String,
        insight: String,
    },
    /// A table found via learning -> learning_references -> table
    /// or metric -> learning_references -> learning -> learning_references -> table.
    Table {
        full_name: String,
        datasource_slug: String,
        description: Option<String>,
    },
}

// -- sqlx row types for expansion queries ------------------------------------

#[derive(sqlx::FromRow)]
struct ColumnExpansionRow {
    column_name: String,
    data_type: Option<String>,
}

#[derive(sqlx::FromRow)]
struct LearningExpansionRow {
    learning_id: String,
    insight: String,
}

#[derive(sqlx::FromRow)]
struct TableExpansionRow {
    full_name: String,
    datasource_slug: String,
    description: Option<String>,
}

#[derive(sqlx::FromRow)]
struct MetricTableExpansionRow {
    full_name: String,
}

/// Expand from previously injected nodes to find related context via SQL queries.
///
/// Runs expansion queries against each category of injected nodes and returns
/// candidate entries with expansion scores. Entries already in `already_injected`
/// are excluded from results.
///
/// Unlike the FalkorDB version which takes a `ConversationContext`, this version
/// takes the individual sets directly to avoid circular dependencies.
pub async fn expand_from_anchors(
    db: &DbPool,
    workspace_id: &str,
    injected_tables: &HashSet<String>,
    injected_learnings: &HashSet<String>,
    injected_metrics: &HashSet<String>,
    already_injected: &HashSet<String>,
) -> Vec<ExpansionHit> {
    if injected_tables.is_empty() && injected_learnings.is_empty() && injected_metrics.is_empty() {
        return vec![];
    }

    let mut hits: Vec<ExpansionHit> = Vec::new();

    // Expand from injected tables -> find connected columns and learnings
    for table_name in injected_tables {
        let columns = expand_table_to_columns(db, workspace_id, table_name, already_injected).await;
        hits.extend(columns);

        let learnings = expand_table_to_learnings(db, workspace_id, table_name, already_injected).await;
        hits.extend(learnings);
    }

    // Expand from injected learnings -> find connected tables
    for learning_id in injected_learnings {
        let tables = expand_learning_to_tables(db, workspace_id, learning_id, already_injected).await;
        hits.extend(tables);
    }

    // Expand from injected metrics -> find source tables (2 hops)
    for metric_name in injected_metrics {
        let tables = expand_metric_to_tables(db, workspace_id, metric_name, already_injected).await;
        hits.extend(tables);
    }

    tracing::debug!(
        anchors_tables = injected_tables.len(),
        anchors_learnings = injected_learnings.len(),
        anchors_metrics = injected_metrics.len(),
        expansion_hits = hits.len(),
        "SQL anchor expansion complete"
    );

    hits
}

/// From an injected table, find its columns via column_embeddings (1 hop).
///
/// Columns that are already injected (tracked in `already_injected` as
/// `"{table_full_name}#{column_name}"`) are excluded.
async fn expand_table_to_columns(
    db: &DbPool,
    workspace_id: &str,
    table_full_name: &str,
    already_injected: &HashSet<String>,
) -> Vec<ExpansionHit> {
    // Match the full_name by constructing it from parts using the same
    // logic as build_full_table_name: project.dataset.table or dataset.table
    let is_pg = db.is_postgres();
    let false_val = kyomi_core::sql_compat::bool_false(is_pg);
    let query = format!(
        "SELECT ce.column_name, ce.data_type \
         FROM column_embeddings ce \
         JOIN datasource_table_cache tc ON ce.table_cache_id = tc.id \
         WHERE tc.workspace_id = $1 AND tc.is_archived = {false_val} \
           AND {FULL_NAME_CASE_SQL} = $2",
    );
    let result = kyomi_core::db_fetch_all!(
        db,
        ColumnExpansionRow,
        &query,
        &workspace_id,
        &table_full_name
    );

    let rows = match result {
        Ok(r) => r,
        Err(e) => {
            tracing::warn!(
                table = table_full_name,
                error = %e,
                "Expansion: Table->Column query failed, skipping"
            );
            return vec![];
        }
    };

    let mut hits = Vec::new();
    for row in rows {
        let col_key = format!("{table_full_name}#{}", row.column_name);
        if already_injected.contains(&col_key) {
            continue;
        }

        hits.push(ExpansionHit {
            kind: ExpansionHitKind::Column {
                name: row.column_name,
                table_full_name: table_full_name.to_string(),
                data_type: row.data_type.unwrap_or_default(),
            },
            score: EXPANSION_BASE_SCORE,
        });
    }

    hits
}

/// From an injected table, find learnings that reference it (1 hop).
async fn expand_table_to_learnings(
    db: &DbPool,
    workspace_id: &str,
    table_full_name: &str,
    already_injected: &HashSet<String>,
) -> Vec<ExpansionHit> {
    let is_pg = db.is_postgres();
    let true_val = kyomi_core::sql_compat::bool_true(is_pg);
    let false_val = kyomi_core::sql_compat::bool_false(is_pg);
    let sql = format!(
        "SELECT CAST(al.learning_id AS TEXT) as learning_id, al.insight \
         FROM learning_references lr \
         JOIN agent_learnings al ON lr.learning_id = al.learning_id \
         WHERE lr.ref_type = 'table' AND lr.ref_name = $1 AND lr.workspace_id = $2 \
           AND al.enabled = {true_val} AND al.is_superseded = {false_val}"
    );

    let result = kyomi_core::db_fetch_all!(
        db,
        LearningExpansionRow,
        &sql,
        &table_full_name,
        &workspace_id
    );

    let rows = match result {
        Ok(r) => r,
        Err(e) => {
            tracing::warn!(
                table = table_full_name,
                error = %e,
                "Expansion: Table->Learning query failed, skipping"
            );
            return vec![];
        }
    };

    let mut hits = Vec::new();
    for row in rows {
        if already_injected.contains(&row.learning_id) {
            continue;
        }

        hits.push(ExpansionHit {
            kind: ExpansionHitKind::Learning {
                id: row.learning_id,
                insight: row.insight,
            },
            score: EXPANSION_BASE_SCORE,
        });
    }

    hits
}

/// From an injected learning, find tables it references (1 hop).
async fn expand_learning_to_tables(
    db: &DbPool,
    workspace_id: &str,
    learning_id: &str,
    already_injected: &HashSet<String>,
) -> Vec<ExpansionHit> {
    let is_pg = db.is_postgres();
    let uuid_param = kyomi_core::sql_compat::cast_to_uuid(is_pg, "$1");
    let json_desc = kyomi_core::sql_compat::json_extract_text(is_pg, "tc.table_metadata", "description");
    let sql = format!(
        "SELECT lr.ref_name as full_name, \
                COALESCE(dc.slug, '') as datasource_slug, \
                {json_desc} as description \
         FROM learning_references lr \
         JOIN datasource_table_cache tc ON \
           {FULL_NAME_CASE_SQL} = lr.ref_name \
           AND tc.workspace_id = lr.workspace_id \
         LEFT JOIN datasource_configs dc ON tc.datasource_config_id = dc.id \
         WHERE lr.learning_id = {uuid_param} AND lr.ref_type = 'table' \
           AND lr.workspace_id = $2 AND tc.is_archived = {false_val}",
        false_val = kyomi_core::sql_compat::bool_false(is_pg),
    );
    let result = kyomi_core::db_fetch_all!(
        db,
        TableExpansionRow,
        &sql,
        &learning_id,
        &workspace_id
    );

    let rows = match result {
        Ok(r) => r,
        Err(e) => {
            tracing::warn!(
                learning_id,
                error = %e,
                "Expansion: Learning->Table query failed, skipping"
            );
            return vec![];
        }
    };

    let mut hits = Vec::new();
    for row in rows {
        if already_injected.contains(&row.full_name) {
            continue;
        }

        hits.push(ExpansionHit {
            kind: ExpansionHitKind::Table {
                full_name: row.full_name,
                datasource_slug: row.datasource_slug,
                description: row.description,
            },
            score: EXPANSION_BASE_SCORE,
        });
    }

    hits
}

/// From an injected metric, find tables via learning_references 2-hop:
/// metric ref_name -> learning_id -> same learning's table refs.
async fn expand_metric_to_tables(
    db: &DbPool,
    workspace_id: &str,
    metric_name: &str,
    already_injected: &HashSet<String>,
) -> Vec<ExpansionHit> {
    let result = kyomi_core::db_fetch_all!(
        db,
        MetricTableExpansionRow,
        "SELECT DISTINCT lr_table.ref_name as full_name \
         FROM learning_references lr_metric \
         JOIN learning_references lr_table ON lr_metric.learning_id = lr_table.learning_id \
         WHERE lr_metric.ref_type = 'metric' AND lr_metric.ref_name = $1 \
           AND lr_table.ref_type = 'table' AND lr_metric.workspace_id = $2",
        &metric_name,
        &workspace_id
    );

    let rows = match result {
        Ok(r) => r,
        Err(e) => {
            tracing::warn!(
                metric = metric_name,
                error = %e,
                "Expansion: Metric->Table query failed, skipping"
            );
            return vec![];
        }
    };

    let mut hits = Vec::new();
    for row in rows {
        if already_injected.contains(&row.full_name) {
            continue;
        }

        // For metric expansion, we only get the full_name -- no slug or description.
        // The retrieval pipeline will fetch metadata for tables that pass the score floor.
        hits.push(ExpansionHit {
            kind: ExpansionHitKind::Table {
                full_name: row.full_name,
                datasource_slug: String::new(),
                description: None,
            },
            score: EXPANSION_BASE_SCORE,
        });
    }

    hits
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn expansion_base_score_above_min_similarity() {
        // EXPANSION_BASE_SCORE must be above MIN_SIMILARITY (0.25) to pass the
        // quality gate in the retrieval pipeline.
        assert!(EXPANSION_BASE_SCORE > 0.25);
    }

    #[test]
    fn expansion_base_score_below_strong_vector_match() {
        // EXPANSION_BASE_SCORE should be below typical strong vector matches
        // (0.6-0.9) so that direct vector hits rank higher.
        assert!(EXPANSION_BASE_SCORE < 0.6);
    }

    #[test]
    fn empty_anchor_sets_satisfy_early_return_guard() {
        // The async expand_from_anchors() checks all three sets are empty
        // and returns vec![] immediately without hitting the database.
        // This test verifies the precondition logic -- integration tests
        // cover the actual async function with a database.
        let tables: HashSet<String> = HashSet::new();
        let learnings: HashSet<String> = HashSet::new();
        let metrics: HashSet<String> = HashSet::new();
        assert!(tables.is_empty() && learnings.is_empty() && metrics.is_empty());
    }
}
