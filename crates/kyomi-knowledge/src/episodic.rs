// SPDX-License-Identifier: AGPL-3.0-or-later

//! Episodic layer -- cross-conversation memory and knowledge quality management.
//!
//! Provides three capabilities:
//!
//! 1. **Post-conversation recording** -- when a conversation ends, writes to
//!    the `conversation_discussed` table recording every table, learning, and
//!    metric that was injected during the session.
//!
//! 2. **Contradiction detection** -- finds metrics that have multiple conflicting
//!    definitions (multiple learnings defining the same metric name differently).
//!
//! 3. **Staleness detection** -- finds learnings that haven't been retrieved in
//!    a configurable number of days.
//!
//! # Design
//!
//! All operations use the database directly (Postgres or SQLite).
//! All operations are fire-and-forget safe: callers should log errors and
//! continue if these fail.

use chrono::{DateTime, Utc};
use kyomi_core::db::DbPool;

use crate::context::ConversationContext;

// ---------------------------------------------------------------------------
// Public types
// ---------------------------------------------------------------------------

/// A detected contradiction: a metric with multiple conflicting definitions.
#[derive(Debug, Clone)]
pub struct Contradiction {
    /// The metric name (e.g., "MRR").
    pub metric_name: String,
    /// The conflicting learning definitions (at least 2).
    pub conflicts: Vec<ConflictingLearning>,
}

/// One learning that participates in a metric contradiction.
#[derive(Debug, Clone)]
pub struct ConflictingLearning {
    /// The learning UUID.
    pub learning_id: String,
    /// The insight text.
    pub insight: String,
    /// The learning type (e.g., "metric", "navigation").
    pub learning_type: String,
    /// How many times this learning has been used.
    pub times_used: i32,
    /// When this learning was created (ISO 8601 string).
    pub created_at: String,
}

/// A learning that hasn't been used recently.
#[derive(Debug, Clone)]
pub struct StaleLearning {
    /// The learning UUID.
    pub learning_id: String,
    /// The learning insight text.
    pub insight: String,
    /// When this learning was last used (None if never used).
    pub last_used_at: Option<DateTime<Utc>>,
    /// When this learning was created.
    pub created_at: DateTime<Utc>,
}


// ---------------------------------------------------------------------------
// 1. Post-conversation recording
// ---------------------------------------------------------------------------

/// Record what was discussed in a conversation to the `conversation_discussed`
/// table.
///
/// For each injected table, learning, and metric in the `ConversationContext`,
/// inserts a row into `conversation_discussed`. Uses `ON CONFLICT DO NOTHING`
/// for idempotency -- safe to call multiple times for the same session.
///
/// Called after `execute_agent_chat` completes. The `ConversationContext`
/// (loaded from Redis) tells us what was discussed.
///
/// Fire-and-forget safe: returns `Result` so callers can log errors and continue.
pub async fn record_conversation(
    db: &DbPool,
    session_id: &str,
    user_id: &str,
    workspace_id: &str,
    context: &ConversationContext,
) -> anyhow::Result<()> {
    // Nothing to record if no context was injected.
    if context.is_empty() {
        tracing::debug!(
            session_id,
            "No context was injected, skipping conversation recording"
        );
        return Ok(());
    }

    let mut inserted = 0u32;

    let sql = "INSERT INTO conversation_discussed (session_id, workspace_id, user_id, entity_type, entity_id) \
               VALUES ($1, $2, $3, $4, $5) \
               ON CONFLICT (session_id, entity_type, entity_id) DO NOTHING";

    // Record discussed tables.
    for table_name in &context.injected_tables {
        let rows = kyomi_core::db_execute!(db, sql, session_id, workspace_id, user_id, "table", table_name)
            .map(|r| r.rows_affected());
        match rows {
            Ok(n) => inserted += n as u32,
            Err(e) => {
                tracing::warn!(
                    session_id,
                    table_name,
                    error = %e,
                    "Failed to record discussed table"
                );
            }
        }
    }

    // Record discussed learnings.
    for learning_id in &context.injected_learnings {
        let rows = kyomi_core::db_execute!(db, sql, session_id, workspace_id, user_id, "learning", learning_id)
            .map(|r| r.rows_affected());
        match rows {
            Ok(n) => inserted += n as u32,
            Err(e) => {
                tracing::warn!(
                    session_id,
                    learning_id,
                    error = %e,
                    "Failed to record discussed learning"
                );
            }
        }
    }

    // Record discussed metrics.
    for metric_name in &context.injected_metrics {
        let rows = kyomi_core::db_execute!(db, sql, session_id, workspace_id, user_id, "metric", metric_name)
            .map(|r| r.rows_affected());
        match rows {
            Ok(n) => inserted += n as u32,
            Err(e) => {
                tracing::warn!(
                    session_id,
                    metric_name,
                    error = %e,
                    "Failed to record discussed metric"
                );
            }
        }
    }

    tracing::info!(
        session_id,
        tables = context.injected_tables.len(),
        learnings = context.injected_learnings.len(),
        metrics = context.injected_metrics.len(),
        inserted,
        "Recorded conversation in database"
    );

    Ok(())
}

// ---------------------------------------------------------------------------
// 2. Contradiction detection
// ---------------------------------------------------------------------------

/// Find metrics that have multiple conflicting definitions.
///
/// A metric is "contradicted" when multiple active learnings define the same
/// metric name differently. Uses `structured_metadata->>'metric_name'` from
/// the `agent_learnings` table to identify conflicts.
///
/// Returns contradictions only within the specified workspace.
pub async fn detect_contradictions(
    db: &DbPool,
    workspace_id: &str,
) -> anyhow::Result<Vec<Contradiction>> {
    let is_pg = db.is_postgres();
    let json_extract = kyomi_core::sql_compat::json_extract_text(is_pg, "structured_metadata", "metric_name");

    let sql = format!(
        "WITH conflicting_metrics AS ( \
             SELECT COALESCE({json_extract}, '') AS metric_name \
             FROM agent_learnings \
             WHERE workspace_id = $1 \
               AND learning_type = 'metric' \
               AND enabled = {true_val} \
               AND is_superseded = {false_val} \
               AND {json_extract} IS NOT NULL \
               AND {json_extract} != '' \
             GROUP BY {json_extract} \
             HAVING COUNT(*) > 1 \
         ) \
         SELECT COALESCE({json_extract}, '') AS metric_name, \
                CAST(al.learning_id AS TEXT) AS learning_id, \
                al.insight AS insight, \
                CAST(al.learning_type AS TEXT) AS learning_type, \
                al.times_used AS times_used, \
                CAST(al.created_at AS TEXT) AS created_at \
         FROM agent_learnings al \
         JOIN conflicting_metrics cm \
           ON COALESCE({json_extract}, '') = cm.metric_name \
         WHERE al.workspace_id = $1 \
           AND al.learning_type = 'metric' \
           AND al.enabled = {true_val} \
           AND al.is_superseded = {false_val} \
         ORDER BY metric_name, al.created_at ASC",
        json_extract = json_extract,
        true_val = kyomi_core::sql_compat::bool_true(is_pg),
        false_val = kyomi_core::sql_compat::bool_false(is_pg),
    );

    let rows = kyomi_core::db_fetch_all!(
        db,
        ContradictionRow,
        &sql,
        &workspace_id
    )?;

    // Group rows by metric_name into Contradiction structs.
    let mut contradictions: Vec<Contradiction> = Vec::new();
    let mut current_metric: Option<String> = None;
    let mut current_conflicts: Vec<ConflictingLearning> = Vec::new();

    for row in rows {
        let is_new_group = current_metric.as_ref() != Some(&row.metric_name);

        if is_new_group {
            // Flush the previous group (if any).
            if let Some(metric_name) = current_metric.take() {
                if current_conflicts.len() > 1 {
                    contradictions.push(Contradiction {
                        metric_name,
                        conflicts: std::mem::take(&mut current_conflicts),
                    });
                }
                current_conflicts.clear();
            }
            current_metric = Some(row.metric_name.clone());
        }

        current_conflicts.push(ConflictingLearning {
            learning_id: row.learning_id,
            insight: row.insight,
            learning_type: row.learning_type,
            times_used: row.times_used,
            created_at: row.created_at,
        });
    }

    // Flush the last group.
    if let Some(metric_name) = current_metric
        && current_conflicts.len() > 1 {
            contradictions.push(Contradiction {
                metric_name,
                conflicts: current_conflicts,
            });
        }

    tracing::info!(
        workspace_id,
        count = contradictions.len(),
        "Detected metric contradictions"
    );

    Ok(contradictions)
}

#[derive(sqlx::FromRow)]
struct ContradictionRow {
    metric_name: String,
    learning_id: String,
    insight: String,
    learning_type: String,
    times_used: i32,
    created_at: String,
}

// ---------------------------------------------------------------------------
// 3. Staleness detection
// ---------------------------------------------------------------------------

/// Find learnings that haven't been used in the last `days` days.
///
/// Returns learnings that are enabled, not superseded, and either:
/// - Never used (`last_used_at` is NULL), or
/// - Last used more than `days` days ago.
pub async fn detect_stale_learnings(
    db: &DbPool,
    workspace_id: &str,
    days: i64,
) -> anyhow::Result<Vec<StaleLearning>> {
    let is_pg = db.is_postgres();
    let true_val = kyomi_core::sql_compat::bool_true(is_pg);
    let false_val = kyomi_core::sql_compat::bool_false(is_pg);
    let staleness_check = if is_pg {
        "(last_used_at IS NULL OR last_used_at < NOW() - make_interval(days => $2))".to_string()
    } else {
        "(last_used_at IS NULL OR last_used_at < datetime('now', '-' || $2 || ' days'))".to_string()
    };

    let sql = format!(
        "SELECT CAST(learning_id AS TEXT) AS learning_id, \
                insight AS insight, \
                last_used_at AS last_used_at, \
                created_at AS created_at \
         FROM agent_learnings \
         WHERE workspace_id = $1 \
           AND enabled = {true_val} \
           AND is_superseded = {false_val} \
           AND {staleness_check} \
         ORDER BY created_at ASC",
    );

    let days_i32 = days as i32;
    let rows = kyomi_core::db_fetch_all!(
        db,
        StaleLearningRow,
        &sql,
        &workspace_id,
        &days_i32
    )?;

    let learnings: Vec<StaleLearning> = rows
        .into_iter()
        .map(|row| {
            let created_at = parse_datetime_string(&row.created_at);
            let last_used_at = row.last_used_at.as_deref().map(parse_datetime_string);
            StaleLearning {
                learning_id: row.learning_id,
                insight: row.insight,
                last_used_at,
                created_at,
            }
        })
        .collect();

    tracing::info!(
        workspace_id,
        days,
        count = learnings.len(),
        "Detected stale learnings"
    );

    Ok(learnings)
}

#[derive(sqlx::FromRow)]
struct StaleLearningRow {
    learning_id: String,
    insight: String,
    last_used_at: Option<String>,
    created_at: String,
}

/// Parse a datetime string from the database into `DateTime<Utc>`.
///
/// Handles both RFC 3339 (Postgres) and SQLite's `"YYYY-MM-DD HH:MM:SS"` format.
fn parse_datetime_string(s: &str) -> DateTime<Utc> {
    chrono::DateTime::parse_from_rfc3339(s)
        .map(|dt| dt.with_timezone(&Utc))
        .unwrap_or_else(|_| {
            chrono::NaiveDateTime::parse_from_str(s, "%Y-%m-%d %H:%M:%S")
                .map(|ndt| ndt.and_utc())
                .unwrap_or_default()
        })
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn contradiction_struct_fields() {
        let c = Contradiction {
            metric_name: "MRR".into(),
            conflicts: vec![
                ConflictingLearning {
                    learning_id: "learn-1".into(),
                    insight: "MRR is monthly recurring revenue".into(),
                    learning_type: "metric".into(),
                    times_used: 50,
                    created_at: "2025-01-01T00:00:00Z".into(),
                },
                ConflictingLearning {
                    learning_id: "learn-2".into(),
                    insight: "MRR is annualized recurring revenue / 12".into(),
                    learning_type: "metric".into(),
                    times_used: 3,
                    created_at: "2025-06-15T00:00:00Z".into(),
                },
            ],
        };
        assert_eq!(c.metric_name, "MRR");
        assert_eq!(c.conflicts.len(), 2);
        assert_eq!(c.conflicts[0].times_used, 50);
        assert_eq!(c.conflicts[1].learning_id, "learn-2");
    }

    #[test]
    fn stale_learning_struct_fields() {
        let s = StaleLearning {
            learning_id: "learn-abc".into(),
            insight: "Old learning".into(),
            last_used_at: None,
            created_at: Utc::now(),
        };
        assert_eq!(s.learning_id, "learn-abc");
        assert!(s.last_used_at.is_none());
    }

    #[test]
    fn record_conversation_skips_empty_context() {
        // Verify the logic: is_empty() returns true for a fresh context.
        let ctx = ConversationContext::new();
        assert!(ctx.is_empty());
    }

    #[test]
    fn conflicting_learning_struct_fields() {
        let cl = ConflictingLearning {
            learning_id: "learn-1".into(),
            insight: "Some insight".into(),
            learning_type: "metric".into(),
            times_used: 10,
            created_at: "2025-01-01T00:00:00Z".into(),
        };
        assert_eq!(cl.learning_id, "learn-1");
        assert_eq!(cl.learning_type, "metric");
        assert_eq!(cl.times_used, 10);
    }

    #[test]
    fn stale_learning_with_last_used() {
        let now = Utc::now();
        let s = StaleLearning {
            learning_id: "learn-xyz".into(),
            insight: "Some stale insight".into(),
            last_used_at: Some(now),
            created_at: now,
        };
        assert_eq!(s.learning_id, "learn-xyz");
        assert!(s.last_used_at.is_some());
        assert_eq!(s.last_used_at.unwrap(), now);
    }

    #[test]
    fn contradiction_with_many_conflicts() {
        let c = Contradiction {
            metric_name: "ARR".into(),
            conflicts: vec![
                ConflictingLearning {
                    learning_id: "l1".into(),
                    insight: "ARR = MRR * 12".into(),
                    learning_type: "metric".into(),
                    times_used: 100,
                    created_at: "2024-01-01T00:00:00Z".into(),
                },
                ConflictingLearning {
                    learning_id: "l2".into(),
                    insight: "ARR = sum of annual contracts".into(),
                    learning_type: "metric".into(),
                    times_used: 5,
                    created_at: "2025-03-01T00:00:00Z".into(),
                },
                ConflictingLearning {
                    learning_id: "l3".into(),
                    insight: "ARR = trailing 12-month revenue".into(),
                    learning_type: "metric".into(),
                    times_used: 2,
                    created_at: "2025-06-01T00:00:00Z".into(),
                },
            ],
        };
        assert_eq!(c.conflicts.len(), 3);
        assert_eq!(c.metric_name, "ARR");
    }
}
