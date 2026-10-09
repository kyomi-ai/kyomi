// SPDX-License-Identifier: AGPL-3.0-or-later

//! Regression tests for KYO-808.
//!
//! `references.rs::backfill_all_references`, `expansion.rs::expand_table_to_learnings`,
//! and `populate.rs::populate_workspace`'s learning-embedding query all filtered on
//! `agent_learnings.is_superseded`, a column that has never existed on either dialect's
//! schema — only `superseded_by` (self-FK) and `superseded_at` do (see
//! `idx_agent_learnings_superseded` in both `apps/server/migrations/20260215000000_baseline.sql`
//! and `apps/server/migrations-sqlite/00001_baseline.sql`). In production this failed
//! every one of these queries with `column "is_superseded" does not exist`, breaking
//! `kyomi_agent::catalog_scheduler`'s hourly learning-reference backfill for every
//! workspace. `references.rs::materialize_learning_references` -- the function that
//! actually writes `learning_references` rows -- is called only from
//! `backfill_all_references` (`catalog_scheduler.rs:891`), so this failure meant
//! `learning_references` has never been populated on any deployment: every
//! `learning_references`-based expansion in `expansion.rs` (Table->Learning,
//! Learning->Table, and the Metric->Table 2-hop) returned nothing during chat context
//! retrieval, independent of `expand_table_to_learnings`'s own `is_superseded` bug.
//! `populate.rs::populate_workspace`'s learning-embedding query was broken the same
//! way, but that function currently has no callers in the workspace, so it carries no
//! production impact today -- it is fixed here because it is dead code with a live bug,
//! not because anything currently depends on it.
//!
//! The fix replaces `is_superseded = {false}` with the codebase's canonical "not
//! superseded" predicate, `superseded_by IS NULL` (already used by
//! `vector_search.rs`, `learning_service.rs`, and `watch_execution.rs`).
//!
//! Each test below creates its own scratch Postgres database (KYO-242 pattern,
//! matching `crates/kyomi-core/tests/agent_learnings_superseded_by_on_delete.rs` and
//! `crates/kyomi-core/tests/schema_parity.rs`), runs the real migration chain via
//! `kyomi_core::db::DbPool::connect` -- the same entry point production uses -- seeds
//! `agent_learnings` rows that are live / superseded-via-pointer / disabled, and
//! asserts the fixed query returns only the live row. Because these exercise the real
//! migrated schema rather than a mock, a regression back to `is_superseded` is caught,
//! not a false green: all three tests fail with the exact
//! `column "is_superseded" does not exist` Postgres error production hit. (For the
//! `expand_from_anchors` test that relies on KYO-809: `expand_table_to_learnings` now
//! propagates its query error as `Err` instead of logging it and returning no hits.)
//! See the mutation check recorded in the KYO-808 PR description.

mod common;

use common::{seed_learning, with_scratch_workspace};
use std::collections::HashSet;

/// KYO-808 regression test for `references.rs::backfill_all_references`.
///
/// Seeds three learnings -- live, superseded-via-pointer, and disabled -- and
/// asserts the backfill processes only the live one. Before the fix this fails
/// outright with `column "is_superseded" does not exist` (see the mutation check
/// in the KYO-808 PR description); the assertion on the *count*, not just presence,
/// also catches a filter that widened rather than vanished (see
/// docs/standards/testing/assert-result-size-not-just-seeded-rows.md).
#[tokio::test]
async fn backfill_all_references_processes_only_the_live_learning() {
    let (result, live_id, refs): (kyomi_core::Result<usize>, String, Vec<(String, String, String)>) =
        with_scratch_workspace("backfill", |db, workspace_id| async move {
            let live_id = seed_learning(
                &db,
                &workspace_id,
                "live learning",
                true,
                None,
                Some(serde_json::json!({ "related_metrics": ["mrr"] })),
            )
            .await;

            let _superseded_id = seed_learning(
                &db,
                &workspace_id,
                "superseded learning",
                true,
                Some(&live_id),
                Some(serde_json::json!({ "related_metrics": ["should-not-appear-superseded"] })),
            )
            .await;

            let _disabled_id = seed_learning(
                &db,
                &workspace_id,
                "disabled learning",
                false,
                None,
                Some(serde_json::json!({ "related_metrics": ["should-not-appear-disabled"] })),
            )
            .await;

            let result = kyomi_knowledge::references::backfill_all_references(&db, &workspace_id).await;

            let refs: Vec<(String, String, String)> = sqlx::query_as(
                "SELECT CAST(learning_id AS TEXT), ref_type, ref_name \
                 FROM learning_references WHERE workspace_id = $1",
            )
            .bind(&workspace_id)
            .fetch_all(db.pg_pool())
            .await
            .expect("query learning_references after backfill");

            (result, live_id, refs)
        })
        .await;

    let processed = result.unwrap_or_else(|e| {
        panic!(
            "backfill_all_references must succeed against the real migrated schema, \
             not fail with a missing-column error: {e}"
        )
    });
    assert_eq!(
        processed, 1,
        "backfill_all_references must process exactly the one live (enabled, not \
         superseded) learning -- not zero (over-filtering) and not all three \
         (the superseded_by IS NULL predicate not applied)"
    );

    assert_eq!(
        refs.len(),
        1,
        "learning_references must contain exactly one row -- only the live \
         learning's metric reference, not the superseded or disabled learnings'"
    );
    assert_eq!(
        refs[0],
        (live_id, "metric".to_string(), "mrr".to_string()),
        "the one materialized reference must belong to the live learning"
    );
}

/// KYO-808 regression test for `expansion.rs::expand_table_to_learnings` (reached
/// via the public `expand_from_anchors`).
///
/// Seeds `learning_references` rows linking a table anchor to all three learnings,
/// then asserts anchor expansion surfaces only the live one. Since KYO-809,
/// `expand_from_anchors` propagates leaf query errors as `Err`, so under the pre-fix
/// `is_superseded` mutation this test goes red on the `.expect` below with the
/// "column does not exist" error, rather than silently seeing `0` learnings.
#[tokio::test]
async fn expand_from_anchors_surfaces_only_the_live_learning() {
    const TABLE: &str = "public.orders";

    let hits = with_scratch_workspace("expansion", |db, workspace_id| async move {
        let live_id =
            seed_learning(&db, &workspace_id, "live insight about orders", true, None, None).await;
        let superseded_id = seed_learning(
            &db,
            &workspace_id,
            "superseded insight about orders",
            true,
            Some(&live_id),
            None,
        )
        .await;
        let disabled_id =
            seed_learning(&db, &workspace_id, "disabled insight about orders", false, None, None)
                .await;

        for learning_id in [&live_id, &superseded_id, &disabled_id] {
            sqlx::query(
                "INSERT INTO learning_references (learning_id, workspace_id, ref_type, ref_name) \
                 VALUES ($1, $2, 'table', $3)",
            )
            .bind(learning_id)
            .bind(&workspace_id)
            .bind(TABLE)
            .execute(db.pg_pool())
            .await
            .expect("seed learning_references row");
        }

        let mut injected_tables = HashSet::new();
        injected_tables.insert(TABLE.to_string());

        let hits = kyomi_knowledge::expansion::expand_from_anchors(
            &db,
            &workspace_id,
            &injected_tables,
            &HashSet::new(),
            &HashSet::new(),
            &HashSet::new(),
        )
        .await
        .expect("expand_from_anchors must succeed against the real migrated schema");

        (hits, live_id)
    })
    .await;

    let (hits, live_id) = hits;

    let learning_hits: Vec<&str> = hits
        .iter()
        .filter_map(|hit| match &hit.kind {
            kyomi_knowledge::expansion::ExpansionHitKind::Learning { id, .. } => Some(id.as_str()),
            _ => None,
        })
        .collect();

    assert_eq!(
        learning_hits.len(),
        1,
        "table -> learning expansion must surface exactly the one live (enabled, \
         not superseded) learning that references this table -- got {learning_hits:?} \
         (before the KYO-808 fix this query fails outright with a missing-column error)"
    );
    assert_eq!(
        learning_hits[0], live_id,
        "the surfaced learning must be the live one, not the superseded or disabled learning"
    );
}

/// KYO-808 regression test for `populate.rs::populate_workspace`'s learning-embedding
/// query.
///
/// Seeds the same three learnings with no `datasource_table_cache` rows (so the
/// table/column embedding loops are no-ops and only the learning-embedding query
/// under test runs), calls `populate_workspace` with a real `EmbeddingService`, and
/// asserts only the live learning received an embedding.
#[tokio::test]
async fn populate_workspace_embeds_only_the_live_learning() {
    let embed = kyomi_embed::EmbeddingService::new().expect("load embedding model");

    let (result, embedded_flags): (kyomi_core::Result<()>, Vec<(String, bool)>) =
        with_scratch_workspace("populate", |db, workspace_id| {
            let embed = &embed;
            async move {
                let live_id =
                    seed_learning(&db, &workspace_id, "live learning to embed", true, None, None)
                        .await;
                let superseded_id = seed_learning(
                    &db,
                    &workspace_id,
                    "superseded learning, must not be embedded",
                    true,
                    Some(&live_id),
                    None,
                )
                .await;
                let disabled_id = seed_learning(
                    &db,
                    &workspace_id,
                    "disabled learning, must not be embedded",
                    false,
                    None,
                    None,
                )
                .await;

                let result = kyomi_knowledge::populate::populate_workspace(&db, embed, &workspace_id).await;

                // Read embedding state back while the scratch database still
                // exists -- with_scratch_workspace drops it as soon as this
                // async block returns.
                let mut embedded_flags = Vec::new();
                for (label, id) in
                    [("live", &live_id), ("superseded", &superseded_id), ("disabled", &disabled_id)]
                {
                    let (embedded,): (bool,) = sqlx::query_as(
                        "SELECT embedding IS NOT NULL FROM agent_learnings WHERE learning_id = $1",
                    )
                    .bind(id)
                    .fetch_one(db.pg_pool())
                    .await
                    .unwrap_or_else(|e| panic!("query embedding state for {label} learning: {e}"));
                    embedded_flags.push((label.to_string(), embedded));
                }

                (result, embedded_flags)
            }
        })
        .await;

    result.unwrap_or_else(|e| {
        panic!(
            "populate_workspace must succeed against the real migrated schema, \
             not fail with a missing-column error: {e}"
        )
    });

    assert_eq!(
        embedded_flags,
        vec![
            ("live".to_string(), true),
            ("superseded".to_string(), false),
            ("disabled".to_string(), false),
        ],
        "populate_workspace's learning-embedding query must embed only the live \
         (enabled, not superseded) learning -- the superseded and disabled learnings \
         must be left with embedding = NULL"
    );
}
