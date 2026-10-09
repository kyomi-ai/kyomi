# Never bake a real customer or production identifier into this public repo

The most useful comment you can write about a provider quirk is the one that quotes the
real thing: the exact response body BigQuery returned, the exact account that could see the
dataset and the exact project that could not. That is also how a customer's project ID, a
production service-account address, or a company name ends up permanently in a repo that is
public and dual-licensed — pasted verbatim out of a reproduction, into a doc comment or a
test fixture, because it was what the terminal said.

No tool objects. Trivy's secret scanner fires on credential *shapes* — a PEM header, a
token prefix — and a GCP project ID or an `@`-address is neither. There is no lint, no
pre-commit hook and no CI job that will ever tell you a comment names a real customer. The
only gate is the author noticing, and the author is the person least likely to, because the
identifier is the part they just spent an hour looking at.

The fix costs nothing, because the identity is never the load-bearing part. What makes the
comment worth keeping is the *shape* — a 200 response whose list key is absent, one identity
with access and one without. Swap the names for obviously synthetic ones and the comment
teaches exactly as much.

**Rule:** Before staging any comment, doc comment, test fixture, log-message example or
error-context string copied out of a real reproduction, strip the identity: project IDs,
account and user emails, workspace/tenant/user IDs, hostnames, bucket names, customer or
company names. Replace each with a placeholder that could not be mistaken for real
(`acme-corp-472819`, `test-service-account@test-project`). Keep the structural payload —
a response body with no identifying fields is fine verbatim, and saying so is better than
paraphrasing it. If the finding genuinely cannot be described without the customer's
details, it does not belong in this repo at all: `CLAUDE.md` routes anything of that kind to
`~/repos/kyomi-private/docs/`.

```rust
// WRONG — a real GCP project and a real service-account identity, in a doc comment,
// in a public repo, for the rest of the repo's life.
/// The exact observed body: `test-service-account@test-project` has access to the dataset,
/// `acme-corp-472819` does not, and the 200 omits the `datasets` key entirely.

// RIGHT — same shape, same lesson, no identity.
/// The exact observed body: one identity has access to the dataset and the other
/// does not, and the 200 omits the `datasets` key entirely — see the fixture below,
/// which is the response verbatim (it carries no identifying fields).
```

Flagged in **KYO-619** (`2026-09-03`, `13:15`) — the sole blocking finding on a diff whose
parsing and testing work the reviewer called excellent. A real-looking GCP project ID
(`acme-corp-472819`) and a service-account identity (`test-service-account@test-project`) appeared across
four sites in two files (`bigquery_rest.rs:47-48`, `:246-247`, `user_dataset.rs:1213-1215`,
`:1240` — line numbers as they stood at that review, before PR #482 rewrote the module) —
in doc comments, a test context string and a code comment. The framing ("the
exact observed body … has access, … does not") showed it had been copied out of a
production reproduction rather than invented. The remedy accepted in cycle 2 was a straight
substitution of synthetic placeholders; the JSON body literal was kept verbatim, because it
carries no identifying fields.

**It recurred within hours, which is the real lesson here.** The KYO-619 review grepped the
repo and found no other hits, and concluded the pattern was a one-off. It was not: PR #482
("tell a BigQuery listing's absence apart from its emptiness", merged the same day as
`d71b9efd`) rewrote `bigquery_rest.rs` and re-introduced **both** identifiers at new
locations, plus a third in `helpers.rs` — so `grep -rn "test-project\|acme-corp-472819" crates/`
comes back dirty on `origin/main` today. The two changes were in flight concurrently, so the
sanitisation and the re-introduction never saw each other. Filed as KYO-643.

That is why the check belongs at the point the material is *pasted in*, not at the point
someone remembers to grep: a clean grep proves only that the copies you already fixed are
gone, and says nothing about the reproduction another branch is pasting in right now.

Nearest sibling is
[prove-a-flagged-secret-is-fake-before-suppressing-the-scanner.md](prove-a-flagged-secret-is-fake-before-suppressing-the-scanner.md):
that rule is about a string the scanner *does* flag — credential-shaped material, where the
work is proving the fixture fake and scoping the suppression. This one is about the
identifiers no scanner will ever flag, which reach `main` with CI fully green, and whose
remedy is substitution rather than suppression. The two meet at the same fixture: a
fabricated key needs to be visibly fabricated, and the `project_id` and `client_email`
sitting beside it need to be visibly synthetic for the same reason.
