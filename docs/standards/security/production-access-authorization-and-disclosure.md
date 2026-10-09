# Production access requires bounded reads, disclosure and scoped authorization

Tool access does not establish permission. A read can expose secrets or place enough
load on a live service to disrupt it, so classify both the target and the operation
before using a production tool.

**Rule:** Agents may perform bounded, non-secret, read-only production inspection
and factual queries with disclosure. Production writes, migrations, restarts,
secret-value access (including credential retrieval or decryption), and exports
require explicit task-level authorization covering the target and operation.
A generic bugfix request, available credentials or an accessible tool is insufficient.

Production includes live/customer environments and the clusters, hosted services,
databases, storage and vendor control planes that serve them. Classify the actual
target: a development-named connection can reach production, and a non-local
connection can reach a test environment. A kubeconfig filename is not the boundary.
Verify the target using non-secret metadata before proceeding; if the classification
cannot be established, do not assume the read permission applies.

Permitted reads must not invoke mutating functions or commands. Use a read-only
transaction or role where available, bound returned results, use sensible timeouts,
and avoid expensive scans or operations that could disrupt shared infrastructure.
A scalar count bounds output, but may still scan a large table; assess its cost first.
Aggregate counts and non-secret operational metadata are permitted. A database dump,
bulk extraction or saved production dataset is an export, even when its source is
read-only. Reading a secret store or decrypting credentials still needs authorization.

Disclose production access in the ticket and PR/review record: target environment,
purpose, commands or queries with secrets redacted, observed result, and material
load or limitations. Include the authorization reference for restricted actions.
Disclosure never permits sensitive data to appear in a public PR, review log or
transcript; keep private operational details in the private record and provide a
sanitized summary and reference in the public record. If no production access took
place, state that explicitly.

## Examples

**WRONG:** Treat a bugfix request as approval to run a production `UPDATE`, apply a
migration or restart a deployment because the tool is available. Treat a `SELECT`
that retrieves credential values or a read-only database dump as an ordinary read.

**RIGHT:** For a known production target, inspect non-secret health metadata or run
a low-cost count with a timeout and read-only protection, then record the purpose,
query, result and limitations in the ticket and review/PR record. For example, after
checking the table's cost, a bounded factual count can use:

```sql
BEGIN READ ONLY;
SET LOCAL statement_timeout = '5s';
SELECT count(*) FROM example_status;
ROLLBACK;
```

The example table is fictional; the timeout must suit the actual target. Obtain
explicit authorization naming the production target and the intended operation
before a write, migration, restart, secret retrieval/decryption or export. Permission
for a count does not cover any of those operations.

Motivated by **KYO-545** and its recorded policy decision of 2026-10-02. Private
operational guidance and the local instruction installation procedure live in
`kyomi-private/docs/PRODUCTION_ACCESS_POLICY.md`.
