# Prove a flagged secret is fake before suppressing the scanner, and scope the suppression to one file

A secret scanner (Trivy, gitleaks, whatever runs next) does not know the difference between a
real credential and a test fixture shaped like one — it fires on anything that pattern-matches a
private key, a Slack token, a service-account JSON blob. The reflex once it looks like a fixture
is to add an allow-rule and move on.

That reflex is where the risk lives, and it has two halves. *"It's obviously a fake GCP
service-account JSON, look at the project id"* is an assertion, not evidence. And an allow-rule
is a standing instruction to every future run of the scanner to stop looking at that exact file
— so if the assertion is wrong, the suppression is a real leaked credential permanently excluded
from detection, not a CI inconvenience fixed. A suppression written in a hurry also tends to be
scoped as broadly as will make the failure go away rather than as narrowly as the fixture
actually requires.

**Rule:** Before adding an entry to `trivy-secret.yaml` (or any secret-scanner allow-rule), do
three things, not one:

1. **Prove the flagged string cannot be real key material, independently.** Decode it and check
   its structure — a fabricated base64 `private_key` is usually not even valid base64 (length not
   a multiple of 4), or decodes to far too few bytes for the key type it claims to be (an
   RSA-2048 private key is on the order of 1200+ DER bytes; 48 bytes cannot hold one). Check the
   surrounding fields for the same synthetic tells — `project_id`, `client_email`,
   `private_key_id`. As a weaker, corroborating check, the raw base64 *text itself* — read as
   characters, without decoding, since decoding yields binary DER rather than text — sometimes
   visibly spells a marker. Treat that as supporting evidence only: it is case-dependent and not
   every fixture author leaves one. *"It's in a `tests/` module"* or *"the PR says it's fake"* is
   not the proof. Decode it yourself.

2. **Scope the allow-rule to the exact file** — not a directory, not an extension, not the rule
   class. Every entry in `trivy-secret.yaml` matches one literal file path (`path:
   crates/kyomi-auth/src/encryption\.rs`, never `crates/kyomi-auth/`), so a suppression added for
   today's fixture cannot later mask an unrelated real secret in a sibling file or a future edit
   to the same directory.

3. **State the check you ran in the PR, not just the conclusion.** *"65 characters, 65 mod 4 = 1,
   so not valid base64 at all"* is verifiable by the next reader. *"Obviously fake"* is not.

Prefer editing the fixture over suppressing it. A suppression is a permanent, easy-to-forget
carve-out; changing the fixture so it no longer looks like a secret removes the finding at
source. Suppression is the right call only when editing would risk changing behaviour under test
— because the code path being exercised validates against that exact shape.

WRONG — suppressing on inspection, with the reasoning inverted (it claims the marker text appears
after decoding, when the string never validly decodes at all):

```yaml
# private_key is clearly a placeholder — it decodes to spell out "TEST"
- id: gcp-fixture-example
  description: Fake GCP fixture, safe to ignore
  path: scripts/e2e-regression/example\.cjs
```

RIGHT — the allow-rule exists, and the PR that added it recorded the check that justifies it:

```
private_key: '-----BEGIN PRIVATE KEY-----\nMIIBVgIBADANBgkqhkiG9w0BAQEFAASCAUAwggE8AgEAAkEAtESTkeyF0rE2eTest\n-----END PRIVATE KEY-----\n'
```

```
$ python3 -c "s='MIIBVgIBADANBgkqhkiG9w0BAQEFAASCAUAwggE8AgEAAkEAtESTkeyF0rE2eTest'; print(len(s), len(s) % 4)"
65 1
```

65 characters is not a multiple of 4, so the body is not valid base64 at all — it cannot be
decoded, let alone hold RSA/EC key material. The `tESTkeyF0rE2eTest` marker is visible in the
raw, undecoded characters, not in any decoded output. `private_key_id` is
`e2e0000000000000000000000000000000000000` (an `e2e` prefix over zeros, not a real key-id shape)
and `project_id` is `kyomi-e2e-project`, both synthetic and internally consistent with each other
and with the fixture's `client_email`.

Established practice by the time KYO-602 added its own entry (2026-09-02): the four preceding
entries in `trivy-secret.yaml` — `fake-slack-tokens-in-tests`
(`crates/kyomi-auth/src/encryption.rs`), `gcp-placeholder-datasource-ui`
(`crates/kyomi-ui/src/pages/settings/datasources.rs`), `gcp-test-fixture-catalog-scheduler`
(`crates/kyomi-agent/src/catalog_scheduler.rs`), and `gcp-test-fixture-credential-service`
(`crates/kyomi-auth/src/credential_service.rs`) — are each scoped to exactly one file. Three of
the four are the identical class of fabricated-GCP-fixture finding, which is precisely why each
new instance needs its own reproducible check rather than inheriting the precedent on sight.

The correction that produced this rule is itself the point. The first standard mined from
KYO-602's review misattributed *where* the marker text was visible, claiming it appeared "once
decoded" — a re-review caught it by actually running the decode and observing it fail (review log
`2026-09-02`, cycle-2 entry). A standard about verifying scanner suppressions is only credible if
its own evidence was independently run rather than paraphrased from the PR that added the rule.
