# A ticket's claimed mechanism is a hypothesis, not a spec

A ticket is written against a snapshot of the code, by whoever triaged it, at some point
before the implementer opens the file. When it names a specific function, branch, or
existing capability as the mechanism to use — "send explicit JSON `null` so function X's
clear branch removes the field," "the registry already maps field Y to mode Z" — that
claim reads as settled fact, because it's phrased the way a spec is phrased. It is not
one. It is the triager's best read of the code, and the only way to know whether it's
right is to open the function it names and check, before writing anything that depends on
it being true.

The failure mode is not "the ticket was vague." A vague ticket announces its own
uncertainty. A ticket with a specific, wrong, mechanism is worse: it reads as more
authoritative than a correct one, because specificity is usually a signal of care. Two
different tickets in this corpus prescribed exact code paths that didn't exist as
described, and in one case following the prescription literally would have shipped the
data-corruption bug the ticket existed to fix.

**Rule:** when a ticket names a specific function, branch, or existing capability as the
mechanism for the fix, read that function before implementing against it. Confirm the
branch condition, the field set it actually operates over, and whether the "existing"
mapping it describes actually exists — don't assume the description is a compressed but
accurate summary of code you haven't opened yet. When it's wrong, say so on the record
(commit message or PR body), state what you built instead and why, and don't silently
"fix" the ticket text to match — the next reader needs to know the original description
was wrong, not just that the final code is right.

```
WRONG — the ticket's mechanism is implemented literally, unverified:

// Ticket: "send explicit JSON null for the inactive mode's fields so
// finalize_connection_config_secrets' explicit-clear branch removes them."
incoming.insert("oauth_client_secret".into(), Value::Null);
// finalize_connection_config_secrets' clear branch only iterates
// COMMON_SENSITIVE (3 unrelated fields) — oauth_client_secret isn't in it.
// The literal null is never cleared; it's persisted as the stored value,
// destroying the real secret it was meant to remove.

RIGHT — the cited function read first, the gap named on the record:

// Two things in the ticket's description are wrong and are NOT implemented
// as written:
// - It prescribes sending explicit JSON null so finalize_connection_config_
//   secrets' "explicit clear" branch removes the field. That loop iterates
//   COMMON_SENSITIVE only, which excludes oauth_client_secret and
//   service_account_json, so a null would be persisted literally. The keys
//   are removed outright instead.
// - It says the registry already maps fields to auth modes. It did not —
//   AuthModeConfig::credential_fields holds per-user credential fields, not
//   connection_config keys. This adds connection_config_fields alongside it.
incoming.remove("oauth_client_secret");
```

Real precedent — two distinct tickets, both caught only by reading the code the ticket
cited, not by trusting its description:

- **KYO-702** (this repo, commit `ee1ba9cc`, "strip a datasource's inactive auth-mode
  fields on write"). The commit message states both wrong claims verbatim, quoted above.
  Had the first one been implemented as prescribed, the `null` would have been persisted
  literally by `finalize_connection_config_secrets`' `COMMON_SENSITIVE`-only restore loop
  — not cleared — because that function's gap (documented separately as KYO-780) already
  treats anything outside its three hardcoded field names as pass-through. The review
  (`docs/review-logs/2026-09-15.md`, the `02:10` re-review — the signed one; the `01:25`
  initial review was **unsigned**, having found two unrelated pre-existing criticals)
  independently re-checked the registry-driven `connection_config_fields` design that was
  built instead, caller-by-caller against `build_connection_config`'s real per-type arms,
  and confirmed it — the deviation from the ticket's prescribed mechanism was the correct
  call, not a shortcut.
- **KYO-725** (`docs/review-logs/2026-09-10.md`, `09:56` entry, "chart-builder SQL editor
  cursor-reversal fix"). The diff added a `provide_context` shadowing fix for
  `EditorHandle` that the ticket didn't ask for. The reviewer didn't take the addition on
  the implementer's say-so: it traced the actual component tree — `ChartBuilderModal` is
  mounted from inside `SqlEditorPage` via `ResultsContainer`, which itself provides the
  same context type — and only then recorded that *"the premise the ticket got wrong is
  real and the shadowing fix is necessary, not decorative."* Confirming the ticket's gap
  was the thing that turned an unrequested addition into a justified one.

Distinct from
[declare-the-change-the-ticket-did-not-ask-for.md](declare-the-change-the-ticket-did-not-ask-for.md):
that rule is about disclosing a behavior change the ticket never mentioned. This rule is
about a mechanism the ticket *did* mention, specifically and wrongly — the defect isn't an
undisclosed extension, it's an unverified premise. The two compound in the KYO-725
citation above: the fix was both an extension past the ticket (that rule's axis) and a
correction of what the ticket assumed (this rule's axis), and the review checked both.

Distinct from
[../agent-orchestration/a-review-finding-is-a-claim-to-verify-not-an-instruction.md](../agent-orchestration/a-review-finding-is-a-claim-to-verify-not-an-instruction.md):
that rule is about the same discipline applied to a *reviewer's* report, arriving after the
diff is written. This one is about the *ticket*, arriving before a line is written — the
earliest point the same mistake (trusting a claim about the code instead of checking it)
can be made, and the cheapest one to catch.
