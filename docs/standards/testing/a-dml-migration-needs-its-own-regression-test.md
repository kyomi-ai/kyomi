# A migration that UPDATEs or DELETEs rows needs its own regression test — schema parity does not cover it

`crates/kyomi-core/tests/schema_parity.rs` proves the Postgres and SQLite migration chains
produce the same *shape* — tables, columns, constraints — and it does so by running the real
embedded-Postgres chain against a scratch database. That makes it easy to mistake for full
migration coverage. It asserts nothing about a migration's *data* effects. A one-time
reconciliation migration that reassigns or deletes rows on a `WHERE`/`EXISTS` predicate can
have that predicate silently wrong — an omitted referencing column, an inverted condition,
an off-by-one in a cutoff — and schema parity stays green, because the DDL is identical
either way.

The codebase already has the shape for this. `collections_created_by_migration.rs` and
`refresh_tokens_family_id_migration.rs` each define a `sqlite_migrator_up_to(version_limit)`
helper that filters a `Migrator` down to versions `<= version_limit`, so a test can seed a
database at the state immediately *before* the migration, run exactly that one migration, and
assert the resulting rows. A new DML migration landing without such a test is a gap against
an established local pattern, not an unprecedented ask.

**Rule:** A migration whose job is UPDATE/DELETE disposition logic — not just DDL — ships
with a dedicated test that runs it via the `Migrator`-restricted-to-version pattern against a
real seeded database, asserts the resulting row state, and is proven non-vacuous by a
negative-control mutation: break one predicate *in the migration file itself*, confirm the
test fails for that reason, then restore and re-run clean. The test must exercise the real
`.sql` file, never a Rust re-implementation of its predicate — a test that restates the
migration's own logic passes for exactly the same reason the migration is wrong.

```rust
// WRONG — asserts the shape the DDL produced; passes whether the disposition
// predicate reassigned the right rows, the wrong rows, or none at all.
// (schema_parity.rs already covers this and covers nothing more)

// RIGHT — seed at version N-1, run exactly version N, assert the rows.
let migrator = sqlite_migrator_up_to(TARGET_MIGRATION_VERSION - 1).await;
migrator.run(&pool).await.unwrap();
seed_unverified_user_referenced_via_watch_executions_deleted_by(&pool).await;

sqlite_migrator_up_to(TARGET_MIGRATION_VERSION).await.run(&pool).await.unwrap();

let orphan: Option<i64> = sqlx::query_scalar("SELECT 1 FROM users WHERE id = ?1")
    .bind(&drop_id).fetch_optional(&pool).await.unwrap();
assert_eq!(orphan, None, "the referenced unverified row must survive the DELETE");
```

Flagged in **KYO-683** (review log `2026-09-10`, heading *"KYO-683 Phase 1: signup
single-email (server-side)"*): the reconciliation migration
(`20260910120000_reconcile_unverified_user_rows.sql` plus its SQLite twin) shipped with no
test exercising its disposition logic, despite the two precedents above sitting in the same
directory. The re-review (same log, heading *"KYO-683 Phase 1: signup single-email
(server-side) — re-review"*) records the fix —
`crates/kyomi-core/tests/reconcile_unverified_user_rows_migration.rs`, using the
`Migrator`-restricted-to-version pattern — and the reviewer independently reproduced the
negative control by deleting the `watch_executions.deleted_by` `EXISTS` clause from the
SQLite migration and watching the corresponding assertion fail (`left: None, right: Some(1)`,
3 passed / 1 failed) before restoring the file and confirming a clean 4/4 re-run. That is
what makes it a regression trap against the migration file rather than a paraphrase of it.

See [prove-test-fails-without-fix.md](prove-test-fails-without-fix.md) for the
negative-control methodology this rule requires, and
[mutate-by-relocating-real-code.md](mutate-by-relocating-real-code.md) for keeping the
mutation honest; this file is the narrower claim that a DML migration is a category of
change that methodology must be applied to, not only Rust logic changes. Companion to
`data-state-management/enumerate-every-referencing-table-before-an-irreversible-migration.md`,
which is about getting the migration's column list right in the first place — this rule is
what catches it when you did not. That file is **in flight on PR #511** (KYO-683) and is not
on `origin/main`, so it is named here in plain text rather than linked; read it at
`git show 5c3011ae:docs/standards/data-state-management/enumerate-every-referencing-table-before-an-irreversible-migration.md`.
