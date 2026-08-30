# Closing a defect class means the wrong call stops compiling — not that a right one now exists

The standard shape of a fix in this codebase is: notice that some helper is
wrong for a class of inputs, write a correct one beside it, migrate the call
sites the ticket named, ship. That leaves the wrong helper in scope, still
compiling, still reading as ordinary house style, indistinguishable at the call
site from the right one. The class is not closed — it is frozen at whatever
coverage the last sweep achieved, and it starts decaying immediately, because
the next person to write that line has no signal telling them which of the two
to reach for.

The failure is not hypothetical drift, either. The sweep is usually incomplete
*on the day it lands*, and often incomplete inside the very function it fixed:
an author working from a ticket's list of sites stops when the list is done, and
the list was written from a snapshot.

The strong remedy is to make the wrong call fail to type-check for the class you
just fixed — narrow a blanket `impl` with a sealed marker trait, put the raw
constructor behind a checked builder, or delete the old helper outright and
route every site through the new one. Then the compiler, not a future reviewer's
grep, enumerates the stragglers, and a new call site added next month gets the
same answer as the ones in the diff.

**Rule:** When you add a correct way to do X because the existing way is wrong
for some class of X, make the existing way unavailable *for that class* in the
same change. Prove the guard fires by reintroducing the wrong call at a site the
implementation did not touch and showing the compiler rejects it. If the guard
genuinely cannot be built, that is a disclosure, not a silence: grep the count
of sites still on the old path (do not estimate it), state the number in the PR
body, and file the follow-up ticket before shipping. "I migrated the sites the
ticket listed" is not a completion criterion; "the wrong call no longer
type-checks" is.

**WRONG** — the safe variant is added, the unsafe one stays legal, so nothing
stops the next call site (or the two remaining in this same function) from
taking the leaky path:

```rust
// New helper for sqlx errors, which leak constraint/column names via Display.
pub(crate) trait IntoServerFnErrorSqlx<T> {
    fn into_sfn_sqlx(self) -> Result<T, ServerFnError>;
}

// ...but `impl NotKyomiCoreError for sqlx::Error {}` is untouched, so this
// still compiles everywhere it did before, in the same file, on the same type:
let row = kyomi_core::db_fetch_optional!(ac.db(), LastIndexedRow, "...")
    .into_sfn()?; // leaks; nothing in the type system objects
```

**RIGHT** — narrow the bound so the class cannot reach the old path at all:

```rust
/// Sealed marker: `.into_sfn()` is only available for error types with no
/// log-only prefix. `kyomi_core::Error` deliberately does not implement it,
/// so `.into_sfn()` on one is an E0599 and `.into_sfn_core()` is the only
/// way through.
pub(crate) trait NotKyomiCoreError: std::fmt::Display {}
impl NotKyomiCoreError for sqlx::Error {}
impl NotKyomiCoreError for kyomi_connect_protocol::Error {}

impl<T, E: std::fmt::Display + NotKyomiCoreError> IntoServerFnError<T> for Result<T, E> {
    fn into_sfn(self) -> Result<T, ServerFnError> { /* ... */ }
}
```

Real precedent, the same week, in both directions:

- **KYO-523** (review log `2026-08-29`, `22:44`, PR #433) did the strong
  version: `.into_sfn_core()` *plus* the sealed `NotKyomiCoreError` marker that
  narrows `IntoServerFnError`'s blanket impl, so `.into_sfn()` on a
  `kyomi_core::Error` no longer compiles. 195 call sites migrated; the trait's
  own doc comment records how they were enumerated — "audited by temporarily
  requiring this bound and reading off every resulting E0599." The reviewer
  proved the guard was load-bearing rather than decorative by "reintroduc[ing]
  `.into_sfn()` at a fresh call site (`watches.rs:159`, `list_watches`, distinct
  from the implementer's own `analytics.rs:67` check)" and confirming
  `cargo check` failed with E0599. The design assessment recorded that
  "the compile-error-over-lint approach (sealed marker trait) is a stronger
  regression guard than the ticket's suggested lint."
- **KYO-526** (review log `2026-08-30`, `00:12` and `12:56`, PR #435) did the
  weak version for the sibling leak class: `.into_sfn_sqlx()` was added for the
  8 sites the ticket named, but nothing was narrowed — `sqlx::Error` still
  implements `NotKyomiCoreError`, so bare `.into_sfn()` keeps compiling for
  sqlx results. The rebase review found "11 more `db_fetch_optional!`/
  `db_execute!` call sites (sqlx::Result) pipe through bare `.into_sfn()`", and
  noted that two of them "are inside the very function (`get_catalog_stats`)
  this PR partially fixed" — `crates/kyomi-ui/src/server_fns/datasources.rs`,
  the `last_catalog_refresh` and `catalog_refresh_status` reads. KYO-557 tracks
  finishing the migration.
- **KYO-468** (review log `2026-08-29`, `21:20` through `23:40`) is the same
  lesson outside error handling. Three consecutive review cycles each closed one
  more incomplete `bq_projects`/`bq_projects_error`/`bq_projects_attempted`
  reset site by hand — the routing predicate, then the Remove chip, then three
  more sites — and the class only closed in cycle 4, when all seven sites were
  routed through `reset_bq_projects_signals`/`try_reset_bq_projects_signals`,
  "making a partial reset structurally impossible." The cycle-4 note is the rule
  in one sentence: "This is the shape a review should take once a bug has
  recurred three times: verify the invariant is now enforced by the type/call
  signature, not merely re-checked by convention."

Distinct from
[propagate-predicate-changes-to-every-copy.md](propagate-predicate-changes-to-every-copy.md):
that rule covers a predicate already duplicated across N sites and tells you to
extract it and route every site through the extraction. This one covers the
moment *before* that — a new, correct API landing beside an old, wrong one that
remains legal, which is how the N copies come to exist in the first place; and
its remedy is stronger than a sweep plus a guard test, because a sweep only
covers the call sites that exist today. Distinct from
[../data-state-management/teardown-clears-the-whole-derived-state-group.md](../data-state-management/teardown-clears-the-whole-derived-state-group.md),
which says *what* a reset site must clear; this one is about why a seventh
hand-written reset site was reachable at all. Distinct from
[../security/unused-security-helper-worse-than-none.md](../security/unused-security-helper-worse-than-none.md),
where the helper has no callers; here it has callers — just not all of them.
