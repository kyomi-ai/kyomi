# A test's name is a claim — the body must still prove it

A test's identifier is the first thing anyone reads: in a failure report, in `cargo test
name::`, in a coverage grep for "do we test X". It functions as documentation of the
guarantee under test, but nothing type-checks it against the body — so when the body
changes, the name is exactly as likely to go stale as a doc comment, and less likely to be
caught, because a reviewer checking "does this test pass" never has to read the name as a
sentence.

Two shapes have appeared, in two unrelated tickets in the same week:

- **A precision word survives a weakening of what's actually measured.** A test named
  `..._exactly_once` implies a runtime-observed call count. The body instead argues
  single-dispatch by reading two call sites and reasoning about their interaction —
  correct, and honestly disclosed in the test's own doc comment, but the bare identifier
  still promises a stronger, unmeasured guarantee to anyone who only reads the name.
- **A qualifier stops describing what the body constructs.** A test named
  `..._not_configured_by_default` is supposed to prove that an unconfigured service is
  what you get *by default* — i.e., from the same construction path production code takes
  absent explicit configuration. If a later edit changes the body to build an
  explicitly-unconfigured instance through a different seam than the default path, the
  assertion can still be true while "by default" is no longer what it demonstrates.

Both are invisible to every other check: the test compiles, runs, passes, and the assertion
genuinely holds for the value in front of it. The property that broke is the *description*,
not the *result* — nothing green tells you the name and the body still agree.

**Rule:** whenever you touch a test's body — narrow an assertion, change how a fixture is
built, swap which helper constructs the value under test — re-read the test's own
name and any doc comment claiming a guarantee, as a sentence, and check it still holds
against the body as it now stands. Fix the name in the same commit, the same way a stale
doc comment gets fixed in the same commit as the behavior it described (see
[stale-doc-comment-is-a-defect.md](../comments-documentation/stale-doc-comment-is-a-defect.md)).
Where the name claims something the body cannot measure at all — a call count, an
ordering guarantee, "by default" — either strengthen the body to measure it, or rename to
the weaker claim the body actually proves and say in a comment why the stronger one isn't
tested here.

```rust
// WRONG — the pre-fix name, per the KYO-541 review log (`docs/review-logs/2026-09-14.md`,
// 17:16 entry); already renamed before landing, so there is no `<sha>^` on `main` to quote.
#[tokio::test]
async fn edit_knowledge_file_refreshes_knowledge_chunks_exactly_once() { /* ... */ }
// The body never measured a call count — it read two call sites and reasoned about their
// interaction. "Exactly once" is a stronger claim than anything the test runs.

// RIGHT — `crates/kyomi-agent/src/tools/knowledge.rs`, verbatim, on `main`.
/// Single-dispatch — deliberately NOT claimed in this test's name,
/// because nothing here measures it — is enforced by
/// `apply_update` passing `embed: None` to
/// `dashboard_service::update_dashboard`, ...
/// confirmed by reading that both call sites remain as described, not by a
/// runtime assertion here ... What this test asserts is the one thing
/// state-inspection after `execute()` returns actually can prove: the
/// refreshed content is correct and present.
#[tokio::test]
async fn edit_knowledge_file_refreshes_knowledge_chunks_via_unified_path() { /* ... */ }
```

Real precedent:

- **KYO-541** — `docs/review-logs/2026-09-14.md`, *"KYO-541: refresh both dashboard/knowledge
  search indexes on edit"*, 17:16 entry, finding #2 (🟢, Test Coverage — overclaim):
  `edit_knowledge_file_refreshes_knowledge_chunks_exactly_once`'s name implied a
  runtime-verified "exactly once" guarantee the body never measured; the doc comment
  candidly said so, the bare name did not. Resolved same day in the immediate re-review
  (identifier renamed to `..._via_unified_path`, doc comment reworded) — quoted above as it
  stands on `main`.
- **KYO-697** — `docs/review-logs/2026-09-14.md`, *"KYO-697: typed `EmailSendError` for
  email send observability"*, cycle-2 finding #3 and cycle-3 finding #2 (both 🟢, Naming):
  `crates/kyomi-auth/src/email_service.rs`'s `email_service_not_configured_by_default` was
  flagged across two consecutive re-review cycles as no longer testing a default — "the body
  builds an explicitly-unconfigured service and its own new comment says so. Name is now the
  opposite of what it asserts." Left unactioned through cycle 3 ("ticket-worthy"); cited here
  for the finding, not as a landed fix — the exact construction this branch's body used at
  cycle 3 is not represented on `main` as inspected for this rule, so no code block is quoted
  for it.

Distinct from
[an-assertion-a-constant-already-satisfies-cannot-fail.md](an-assertion-a-constant-already-satisfies-cannot-fail.md):
that rule is about an assertion that cannot fail because the value it reads back is a
production-code literal, regardless of what the test is named. This rule is about the
opposite mismatch — the assertion can fail, and does test something real, but the name
promises a different or stronger thing than the body delivers.

Distinct from
[anchor-a-marker-on-the-name-not-the-signature-qualifiers.md](anchor-a-marker-on-the-name-not-the-signature-qualifiers.md):
that rule is about a source-text marker string used to locate a symbol for extraction: it
asks the marker to survive the symbol's *own* future edits. This rule is about a test's
identifier as a claim to a human reader: it asks the name to still describe the body *today*.
