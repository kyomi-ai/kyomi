// SPDX-License-Identifier: AGPL-3.0-or-later

//! Regression tests for KYO-809.
//!
//! Every `expand_*` leaf in `expansion.rs` (`expand_table_to_columns`,
//! `expand_table_to_learnings`, `expand_learning_to_tables`,
//! `expand_metric_to_tables`) used to catch its own DB query error, log a
//! `tracing::warn!`, and return `vec![]` -- exactly what it also returns when
//! the graph genuinely has no matching edges. That made a broken leaf query
//! indistinguishable from "no related context" to any caller. KYO-808's
//! `agent_learnings.is_superseded` bug (a column that never existed on either
//! DB dialect) was present in `expand_table_to_learnings`'s query, so that
//! leaf failed on every call; this change fixes that query (see
//! `expansion.rs::expand_table_to_learnings`) as well as the swallow-on-error
//! pattern that hid it -- see `expansion.rs::expand_from_anchors`'s doc
//! comment for the full error-policy rationale.
//!
//! `expand_from_anchors` now returns `kyomi_core::Result<Vec<ExpansionHit>>`
//! and each leaf propagates its query error with `?` instead of swallowing
//! it. Each test below first runs `expand_from_anchors` against the real,
//! migrated scratch Postgres schema (KYO-242 pattern; harness in
//! `tests/common/mod.rs`) with the seed data intact, as a healthy-schema
//! control: it asserts `Ok` with the specific hit that seed data should
//! produce, proving the leaf under test works end-to-end before anything is
//! broken. It then breaks the one schema object that leaf's query depends on
//! -- via `ALTER TABLE ... RENAME COLUMN` -- supplies only the anchor kind
//! that reaches that leaf, and asserts `expand_from_anchors` now returns
//! `Err`, not `Ok(vec![])`. Because the control already proved this exact
//! seed produces a hit through this exact leaf, the `Err` after the break can
//! only be attributed to this test's own schema break, not to some other
//! leaf that happened to be broken. Pre-fix, every one of these broken-schema
//! inputs produced `Ok(vec![])` (an empty vec is what the swallowed leaf
//! returned, and the un-broken leaves in the same call also return `vec![]`
//! because their own predicates simply match nothing) -- these are exactly
//! the inputs the broken code answered differently from the fixed code.
//!
//! Schema object broken per leaf (each is read by only the leaf under test,
//! so breaking it cannot accidentally fail a different leaf in the same
//! call):
//! - `expand_table_to_columns`: `column_embeddings.column_name`
//! - `expand_table_to_learnings`: `agent_learnings.insight`
//! - `expand_learning_to_tables`: `datasource_table_cache.table_metadata`
//! - `expand_metric_to_tables`: `learning_references.ref_name`

mod common;

use common::{seed_learning, with_scratch_workspace};
use std::collections::HashSet;

/// KYO-809 regression test for `expansion.rs::expand_table_to_columns`.
///
/// Breaks `column_embeddings.column_name` -- the one column that leaf's
/// query selects and that no other leaf's query touches -- and supplies a
/// table anchor (the only anchor kind that reaches this leaf).
#[tokio::test]
async fn expand_table_to_columns_error_is_observable() {
    let result = with_scratch_workspace("excol", |db, workspace_id| async move {
        let (table_cache_id,): (i32,) = sqlx::query_as(
            "INSERT INTO datasource_table_cache \
               (workspace_id, project_id, dataset_id, table_id, table_metadata, is_archived) \
             VALUES ($1, '', 'public', 'orders', '{}'::json, false) \
             RETURNING id",
        )
        .bind(&workspace_id)
        .fetch_one(db.pg_pool())
        .await
        .expect("seed datasource_table_cache row");

        sqlx::query(
            "INSERT INTO column_embeddings (table_cache_id, workspace_id, column_name, data_type) \
             VALUES ($1, $2, 'amount', 'numeric')",
        )
        .bind(table_cache_id)
        .bind(&workspace_id)
        .execute(db.pg_pool())
        .await
        .expect("seed column_embeddings row");

        let mut injected_tables = HashSet::new();
        injected_tables.insert("public.orders".to_string());

        // Healthy-schema control: prove this leaf actually surfaces the
        // seeded column before anything is broken, so the Err asserted below
        // can only be attributed to this test's own schema break.
        let healthy = kyomi_knowledge::expansion::expand_from_anchors(
            &db,
            &workspace_id,
            &injected_tables,
            &HashSet::new(),
            &HashSet::new(),
            &HashSet::new(),
        )
        .await
        .expect("expand_from_anchors must succeed against the healthy, unbroken schema");
        assert!(
            healthy.iter().any(|hit| matches!(
                &hit.kind,
                kyomi_knowledge::expansion::ExpansionHitKind::Column { name, table_full_name, data_type }
                    if name == "amount" && table_full_name == "public.orders" && data_type == "numeric"
            )),
            "healthy-schema control: expected a Column hit for public.orders.amount, got: {healthy:?}"
        );

        // Break the exact column expand_table_to_columns selects, after
        // seeding (the seed insert above needs the real column name).
        sqlx::query("ALTER TABLE column_embeddings RENAME COLUMN column_name TO column_name_old")
            .execute(db.pg_pool())
            .await
            .expect("break column_embeddings.column_name for this test's scratch schema");

        kyomi_knowledge::expansion::expand_from_anchors(
            &db,
            &workspace_id,
            &injected_tables,
            &HashSet::new(),
            &HashSet::new(),
            &HashSet::new(),
        )
        .await
    })
    .await;

    let err = result.expect_err(
        "expand_from_anchors must propagate expand_table_to_columns's DB error as Err, \
         not silently degrade to Ok(vec![]) -- KYO-809",
    );
    assert!(
        err.to_string().contains("expand_table_to_columns"),
        "the propagated error must carry the leaf name as context, got: {err}"
    );
}

/// KYO-809 regression test for `expansion.rs::expand_table_to_learnings`.
///
/// Breaks `agent_learnings.insight` -- selected only by this leaf, not by
/// `expand_table_to_columns` (the other leaf a table anchor also reaches) --
/// and supplies a table anchor.
#[tokio::test]
async fn expand_table_to_learnings_error_is_observable() {
    let result = with_scratch_workspace("exlearn", |db, workspace_id| async move {
        let learning_id =
            seed_learning(&db, &workspace_id, "insight about orders", true, None, None).await;

        sqlx::query(
            "INSERT INTO learning_references (learning_id, workspace_id, ref_type, ref_name) \
             VALUES ($1, $2, 'table', 'public.orders')",
        )
        .bind(&learning_id)
        .bind(&workspace_id)
        .execute(db.pg_pool())
        .await
        .expect("seed learning_references row");

        let mut injected_tables = HashSet::new();
        injected_tables.insert("public.orders".to_string());

        // Healthy-schema control: prove this leaf actually surfaces the
        // seeded learning before anything is broken, so the Err asserted
        // below can only be attributed to this test's own schema break. This
        // also guards KYO-808's regression directly: if the leaf's query
        // predicate ever reverts from `superseded_by IS NULL` to the
        // nonexistent `is_superseded` column, this call fails even though
        // nothing here was deliberately broken.
        let healthy = kyomi_knowledge::expansion::expand_from_anchors(
            &db,
            &workspace_id,
            &injected_tables,
            &HashSet::new(),
            &HashSet::new(),
            &HashSet::new(),
        )
        .await
        .expect("expand_from_anchors must succeed against the healthy, unbroken schema");
        assert!(
            healthy.iter().any(|hit| matches!(
                &hit.kind,
                kyomi_knowledge::expansion::ExpansionHitKind::Learning { id, insight }
                    if *id == learning_id && insight == "insight about orders"
            )),
            "healthy-schema control: expected a Learning hit for {learning_id}, got: {healthy:?}"
        );

        sqlx::query("ALTER TABLE agent_learnings RENAME COLUMN insight TO insight_old")
            .execute(db.pg_pool())
            .await
            .expect("break agent_learnings.insight for this test's scratch schema");

        kyomi_knowledge::expansion::expand_from_anchors(
            &db,
            &workspace_id,
            &injected_tables,
            &HashSet::new(),
            &HashSet::new(),
            &HashSet::new(),
        )
        .await
    })
    .await;

    let err = result.expect_err(
        "expand_from_anchors must propagate expand_table_to_learnings's DB error as Err, \
         not silently degrade to Ok(vec![]) -- KYO-809",
    );
    assert!(
        err.to_string().contains("expand_table_to_learnings"),
        "the propagated error must carry the leaf name as context, got: {err}"
    );
}

/// KYO-809 regression test for `expansion.rs::expand_learning_to_tables`.
///
/// Breaks `datasource_table_cache.table_metadata` -- read only by this leaf's
/// description lookup, not by `expand_table_to_columns` (which joins the
/// same table but never reads `table_metadata`) -- and supplies a learning
/// anchor.
#[tokio::test]
async fn expand_learning_to_tables_error_is_observable() {
    let result = with_scratch_workspace("exltab", |db, workspace_id| async move {
        let learning_id =
            seed_learning(&db, &workspace_id, "insight about orders", true, None, None).await;

        sqlx::query(
            "INSERT INTO datasource_table_cache \
               (workspace_id, project_id, dataset_id, table_id, table_metadata, is_archived) \
             VALUES ($1, '', 'public', 'orders', '{}'::json, false)",
        )
        .bind(&workspace_id)
        .execute(db.pg_pool())
        .await
        .expect("seed datasource_table_cache row");

        sqlx::query(
            "INSERT INTO learning_references (learning_id, workspace_id, ref_type, ref_name) \
             VALUES ($1, $2, 'table', 'public.orders')",
        )
        .bind(&learning_id)
        .bind(&workspace_id)
        .execute(db.pg_pool())
        .await
        .expect("seed learning_references row");

        let mut injected_learnings = HashSet::new();
        injected_learnings.insert(learning_id);

        // Healthy-schema control: prove this leaf actually surfaces the
        // seeded table before anything is broken, so the Err asserted below
        // can only be attributed to this test's own schema break.
        let healthy = kyomi_knowledge::expansion::expand_from_anchors(
            &db,
            &workspace_id,
            &HashSet::new(),
            &injected_learnings,
            &HashSet::new(),
            &HashSet::new(),
        )
        .await
        .expect("expand_from_anchors must succeed against the healthy, unbroken schema");
        assert!(
            healthy.iter().any(|hit| matches!(
                &hit.kind,
                kyomi_knowledge::expansion::ExpansionHitKind::Table { full_name, datasource_slug, description }
                    if full_name == "public.orders" && datasource_slug.is_empty() && description.is_none()
            )),
            "healthy-schema control: expected a Table hit for public.orders, got: {healthy:?}"
        );

        sqlx::query(
            "ALTER TABLE datasource_table_cache RENAME COLUMN table_metadata TO table_metadata_old",
        )
        .execute(db.pg_pool())
        .await
        .expect("break datasource_table_cache.table_metadata for this test's scratch schema");

        kyomi_knowledge::expansion::expand_from_anchors(
            &db,
            &workspace_id,
            &HashSet::new(),
            &injected_learnings,
            &HashSet::new(),
            &HashSet::new(),
        )
        .await
    })
    .await;

    let err = result.expect_err(
        "expand_from_anchors must propagate expand_learning_to_tables's DB error as Err, \
         not silently degrade to Ok(vec![]) -- KYO-809",
    );
    assert!(
        err.to_string().contains("expand_learning_to_tables"),
        "the propagated error must carry the leaf name as context, got: {err}"
    );
}

/// KYO-809 regression test for `expansion.rs::expand_metric_to_tables`.
///
/// Breaks `learning_references.ref_name` -- the only leaf reached by a
/// metric anchor -- and supplies a metric anchor.
#[tokio::test]
async fn expand_metric_to_tables_error_is_observable() {
    let result = with_scratch_workspace("exmetric", |db, workspace_id| async move {
        let learning_id =
            seed_learning(&db, &workspace_id, "MRR is derived from orders", true, None, None).await;

        sqlx::query(
            "INSERT INTO learning_references (learning_id, workspace_id, ref_type, ref_name) \
             VALUES ($1, $2, 'metric', 'MRR')",
        )
        .bind(&learning_id)
        .bind(&workspace_id)
        .execute(db.pg_pool())
        .await
        .expect("seed metric learning_references row");

        sqlx::query(
            "INSERT INTO learning_references (learning_id, workspace_id, ref_type, ref_name) \
             VALUES ($1, $2, 'table', 'public.orders')",
        )
        .bind(&learning_id)
        .bind(&workspace_id)
        .execute(db.pg_pool())
        .await
        .expect("seed table learning_references row");

        let mut injected_metrics = HashSet::new();
        injected_metrics.insert("MRR".to_string());

        // Healthy-schema control: prove this leaf actually surfaces the
        // seeded table before anything is broken, so the Err asserted below
        // can only be attributed to this test's own schema break.
        let healthy = kyomi_knowledge::expansion::expand_from_anchors(
            &db,
            &workspace_id,
            &HashSet::new(),
            &HashSet::new(),
            &injected_metrics,
            &HashSet::new(),
        )
        .await
        .expect("expand_from_anchors must succeed against the healthy, unbroken schema");
        assert!(
            healthy.iter().any(|hit| matches!(
                &hit.kind,
                kyomi_knowledge::expansion::ExpansionHitKind::Table { full_name, datasource_slug, description }
                    if full_name == "public.orders" && datasource_slug.is_empty() && description.is_none()
            )),
            "healthy-schema control: expected a Table hit for public.orders, got: {healthy:?}"
        );

        sqlx::query("ALTER TABLE learning_references RENAME COLUMN ref_name TO ref_name_old")
            .execute(db.pg_pool())
            .await
            .expect("break learning_references.ref_name for this test's scratch schema");

        kyomi_knowledge::expansion::expand_from_anchors(
            &db,
            &workspace_id,
            &HashSet::new(),
            &HashSet::new(),
            &injected_metrics,
            &HashSet::new(),
        )
        .await
    })
    .await;

    let err = result.expect_err(
        "expand_from_anchors must propagate expand_metric_to_tables's DB error as Err, \
         not silently degrade to Ok(vec![]) -- KYO-809",
    );
    assert!(
        err.to_string().contains("expand_metric_to_tables"),
        "the propagated error must carry the leaf name as context, got: {err}"
    );
}
