# A scope key must carry every field the run varies over

Enumerate what you saw, then act on everything you didn't: that is the shape of every
reconciliation pass — archive the cache rows a refresh no longer found, delete the objects a
sync no longer lists, expire the sessions a sweep no longer touched. The set of things-seen is
the safety gate, and it is usually built out of the most obvious identifier to hand: the
container's own name.

That name is unique inside its parent, not inside the run. As long as the parent is a constant
for the whole pass, the two are indistinguishable and the bare key is correct. The moment one
run iterates several parents, the set silently merges them: a name enumerated under parent A
now vouches for the identically-named container under parent B, which this run may never have
reached at all. The gate reports coverage it does not have, and the destructive half of the
pass proceeds against live data.

Nothing here fails loudly. The types line up, the tests pass — because a test fixture almost
always has one parent — and the collision needs two conditions to co-occur: a duplicated name
across parents, and one parent failing or being skipped in the same run. Both are ordinary.
Same-named `dev`/`prod` datasets, `public` schemas, `default` buckets are the norm rather than
the exception, and a per-parent error branch that pushes onto an `errors` vec and continues is
exactly how a partial run is supposed to behave.

**Rule:** Before using a collection of identifiers as a scope, coverage count, or deletion
filter, name the fields that vary *within one run* and confirm the key carries all of them. If
the run iterates projects, tenants, datasources or schemas, the key is the tuple —
`(project_id, dataset_id)`, not `dataset_id` — on the insert side, the filter side, and in any
`SELECT DISTINCT` that counts the same thing for a coverage or shortfall check. Adding the
qualifier is a no-op for the call paths where the parent really is constant, so there is no
trade-off to weigh: the only cost of qualifying is verbosity, and the only cost of not
qualifying is deleting rows nobody looked at. Where the parent is genuinely constant, say why
in a comment at the construction site rather than leaving it implicit, so the next caller that
loops does not inherit the assumption silently.

```rust
// WRONG — the scope key is the child's own name. `index_workspace_catalog`
// iterates several `project_id`s in a single run and inserts only `dataset_id`.
// Project A enumerates `analytics`; project B fails on a permission error and is
// pushed onto `errors`. B's `analytics` rows match the filter — the name was
// enumerated, by the *other* project — and are archived unlooked-at.
let mut enumerated_datasets: HashSet<String> = HashSet::new();
enumerated_datasets.insert(dataset_id.clone());
// ...
archive_stale_rows(ArchiveScope::Containers(enumerated_datasets)).await?;

// ...and the coverage gate that is supposed to catch exactly this under-counts too,
// because it collapses both projects' `analytics` into one row:
// SELECT DISTINCT dataset_id FROM datasource_table_cache WHERE ...

// RIGHT — the key carries the field the run varies over, on both sides.
let mut enumerated_datasets: HashSet<(String, String)> = HashSet::new();
enumerated_datasets.insert((project_id.clone(), dataset_id.clone()));
// ...
archive_stale_rows(ArchiveScope::Containers(enumerated_datasets)).await?;
// SELECT DISTINCT project_id, dataset_id FROM datasource_table_cache WHERE ...
```

Flagged 🟡 in KYO-614's `container-scoped archive gate` review (`2026-09-03`), on the diff
whose entire purpose was to stop a partial enumeration from archiving live catalog rows.
`ArchiveScope::Containers(HashSet<String>)` was keyed by bare `dataset_id`
(`crates/kyomi-auth/src/catalog/helpers.rs`, filter; `catalog/indexers/user_dataset.rs`,
insert). Two of the three indexer paths were safe by accident — SQL-template and Connect hold
`project_id` constant for the whole run — but `UserDatasetIndexer::index_workspace_catalog`
iterates multiple `project_id`s per run (confirmed via
`resolve_project_scope`/`ConfiguredProjectScope::Explicit`, KYO-444) and dropped the project
when inserting into `enumerated_datasets`. Two GCP projects that both have an `analytics`
dataset, plus one of them failing on permission/quota/network in that run, reintroduced the
exact bug the ticket existed to fix. `check_container_coverage`'s `SELECT DISTINCT dataset_id`
carried the identical ambiguity and could under-count `live_count`, masking the material
shortfall that was the second line of defence.

Sibling of
[split-a-value-that-answers-two-questions.md](split-a-value-that-answers-two-questions.md):
there, one value carries two *meanings* and must be split so both become expressible. Here the
value has exactly one meaning — "this container was enumerated" — and is simply
under-qualified for the namespace it is asserted in; nothing is overloaded, and splitting is
not the fix. Related to
[audit-write-sites-when-tightening-constraint.md](audit-write-sites-when-tightening-constraint.md):
if the cache table already declares the composite `(project_id, dataset_id)` as its real
identity, the in-memory scope key that filters it has to agree, and the divergence between a
schema's notion of identity and the code's is what to look for.
