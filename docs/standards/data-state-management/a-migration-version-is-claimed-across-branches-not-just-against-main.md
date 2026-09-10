# The next migration number is claimed across every open branch, not just against `main`

`apps/server/migrations-sqlite/` numbers its files sequentially — `00036_…`, `00037_…` — so
adding a migration means picking "the next number." The check every author performs is `ls`
against `main`, and it is the wrong check. Two branches open at the same time both see
`00036` as the highest, both pick `00037`, and both are correct at the moment they look.
Neither PR conflicts in git, because a new file under a new name never collides. CI is green
on both, because each ran against a base that contained only its own `00037`.

The breakage lands on `main`, after the second merge, for everyone. `sqlx` parses the
integer prefix as the migration *version*
(`sqlx-core-0.8.6/src/migrate/source.rs`, the `parts[0].parse()` in `resolve_blocking`),
sorts by it, and does not reject duplicates. `Migrator::run` snapshots the applied-version
set once before its loop (`sqlx-core-0.8.6/src/migrate/migrator.rs`), so both files with
version `37` are applied — and the second `apply` inserts version `37` into
`_sqlx_migrations`, whose `version` column is declared `BIGINT PRIMARY KEY` in both backends
(`sqlx-sqlite-0.8.6/src/migrate.rs` and `sqlx-postgres-0.8.6/src/migrate.rs`). The second
insert violates the primary key, the migration transaction fails, and every consumer of the
chain — server boot, `schema_parity.rs`, every `test_pool` in `kyomi-auth`, `kyomi-agent`
and `kyomi-core` — fails with it.

The Postgres chain uses a timestamp prefix (`20260910120000_…`), which makes an exact
collision unlikely but does not make ordering safe: `Migrator::run` applies any version
absent from `_sqlx_migrations` regardless of whether a higher version already ran, so a
branch carrying an *earlier* timestamp that merges *later* executes out of authoring order
against databases that already migrated past it.

**Rule:** Before choosing a migration number, enumerate the numbers claimed by open branches
as well as the ones on `main` — `git log --all --diff-filter=A --name-only -- apps/server/migrations-sqlite` and a
`gh pr list` sweep of PR file lists, not `ls`. Say in the PR body which number you claimed
and what you checked it against, so a concurrent author has something to collide with in
review. Re-check at rebase and immediately before merge: the number was free when you picked
it and that says nothing about now. If the number has been taken, renumber your file — never
edit an already-merged migration to make room, because `Migrator::run` compares checksums
against `_sqlx_migrations` and a changed file that has already been applied somewhere errors
with `VersionMismatch`.

```
WRONG — the check that both branches passed:

  $ ls apps/server/migrations-sqlite | tail -1
  00036_add_message_source_to_chat_messages.sql
  # → "00037 is next"     (true on main; true for the other branch too)

RIGHT — enumerate what is in flight, not just what has landed:

  $ git fetch --all --quiet
  $ git log --all --diff-filter=A --name-only --pretty=format: \
      -- apps/server/migrations-sqlite | grep -o '000[0-9][0-9]' | sort -u | tail -3
  $ gh pr list --state open --json number \
      --jq '.[].number' | xargs -I{} gh pr view {} --json files \
      --jq '.files[].path' | grep migrations-sqlite
  # → 00037 is claimed by an open PR; take 00038 and say so in the PR body.
```

Precedent — two open branches, both correct, both `00037`. **KYO-622** (review log
`2026-09-06`, heading *"KYO-622: container-liveness GC for the catalog-archive scope
check"*) adds `apps/server/migrations-sqlite/00037_add_datasource_container_cache.sql`, and
its re-review explicitly records the check that passed: *"no migration filename collision
(sqlite `00037` is next in sequence)"*. **KYO-683** (review log `2026-09-10`, heading
*"KYO-683 Phase 1: signup single-email (server-side)"*) adds
`apps/server/migrations-sqlite/00037_reconcile_unverified_user_rows.sql`. Neither had landed
as of `main` @ `851462da`, whose highest SQLite migration is still `00036`. The reviewer who
verified the number was free did the diligence the convention asks for and could not have
caught this, because the fact that makes it wrong lived on a branch nobody was looking at.

Nearest sibling is
[../comments-documentation/an-ordinal-in-a-comment-collides-with-every-concurrent-addition.md](../comments-documentation/an-ordinal-in-a-comment-collides-with-every-concurrent-addition.md),
which is the same shape — a hand-picked sequence number that two concurrent authors both
believe they own — applied to prose, where the cost is a wrong ordinal in a comment. Here
the artefact is executable and the cost is a `main` that will not boot or test. Distinct
from
[../version-control-working-tree/verify-tree-is-current-before-concluding.md](../version-control-working-tree/verify-tree-is-current-before-concluding.md):
that rule is about a stale local checkout, where fetching fixes it; fetching does not help
here, because the colliding claim is not in any commit's tree until the other PR merges.
