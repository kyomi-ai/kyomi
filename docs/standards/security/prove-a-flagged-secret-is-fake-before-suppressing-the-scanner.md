# Prove a flagged secret is fake before suppressing the scanner, and scope the suppression to one file

Trivy's secret scanner does not know the difference between a real credential and a test
fixture shaped like one — it fires on anything that pattern-matches a private key, a Slack
token, a service-account JSON blob. The reflex once that's confirmed to be a fixture is to
add an allow-rule and move on. That reflex is where the risk lives: "it's obviously a test
fixture" is an assumption, not a check, and a suppression written in a hurry tends to be
scoped as broadly as will make the CI failure go away rather than as narrowly as the actual
fixture requires.

**Rule:** Before adding an entry to `trivy-secret.yaml` (or any secret-scanner allow-rule),
do two things, not one:

1. **Prove the flagged string is fake, independently.** Decode it and check its structure —
   a fabricated base64 `private_key` field is usually not even valid base64 (wrong length,
   not a multiple of 4), or decodes to far too few bytes for real RSA/EC key material. As a
   weaker, corroborating check, the raw base64 *text itself* (read as characters, without
   decoding — decoding it yields binary DER, not text) sometimes visibly spells out a marker
   like "tESTkeyF0rE2eTest"; treat this as supporting evidence only, since it's
   case-dependent and not every fixture author leaves one. Check the surrounding fields too
   (`project_id`, `client_email`, `private_key_id`) for the same synthetic tells. "It's in a
   `tests/` module" or "the PR says it's fake" is not the proof — decode it yourself.
2. **Scope the allow-rule to the exact file, not a directory, an extension, or the rule
   class.** Every existing entry in `trivy-secret.yaml` matches one literal file path
   (`path: crates/kyomi-auth/src/encryption\.rs`, not `crates/kyomi-auth/`), so a suppression
   added for today's fixture cannot later mask an unrelated real secret introduced in a
   sibling file or a future edit to the same directory.

If editing the fixture to remove the flagged shape (e.g., stripping the PEM header from a
fabricated key) would risk changing behavior under test — because the code path being
exercised validates against that shape — suppression is the right call over editing.
Otherwise, prefer editing the fixture so it no longer looks like a secret at all; a
suppression is a permanent, easy-to-forget carve-out and should be reached for only when
editing isn't safe.

Established practice by the time KYO-602 added its own entry (2026-09-02): all four
preceding entries in `trivy-secret.yaml` — `fake-slack-tokens-in-tests`
(`crates/kyomi-auth/src/encryption.rs`), `gcp-placeholder-datasource-ui`
(`crates/kyomi-ui/src/pages/settings/datasources.rs`),
`gcp-test-fixture-catalog-scheduler` (`crates/kyomi-agent/src/catalog_scheduler.rs`), and
`gcp-test-fixture-credential-service` (`crates/kyomi-auth/src/credential_service.rs`) — are
each scoped to exactly one file. KYO-602's review independently decoded the flagged
`bigquery-create-modal.cjs` fixture's `private_key` field before accepting the fifth
allow-rule: 65 base64 characters (not a multiple of 4, so not valid base64 to begin with),
~48 decoded bytes (roughly 25x too short for real RSA/EC key material), and the raw base64
text visibly spelling `tESTkeyF0rE2eTest` (in the characters themselves, not the decoded
bytes) — combined with a `private_key_id` of `e2e0000000000000000000000000000000000000`
and a synthetic `project_id`/`client_email`, conclusive proof of fabrication rather than an
assumption from context.
