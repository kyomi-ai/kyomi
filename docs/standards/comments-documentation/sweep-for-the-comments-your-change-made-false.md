# Sweep for the comments your change made false, not just the code it broke

Deleting a symbol, removing a guard, or collapsing two flows into one breaks the code that
depended on it loudly — `cargo check` names every site. It also falsifies *prose* describing
the world before the change, and that prose is in files the diff never opens, in crates the
ticket never mentions, and nothing checks it. The comment was true when it was written, it is
true-shaped now, and the next reader trusts it over the code.

Two shapes recur, and the second is the one that survives the sweep you would naturally run:

- **The comment names something you deleted.** A doc comment cites a helper as a design
  precedent; the helper is gone. A grep finds this — but only if you grep for the
  *identifiers* you removed rather than for the feature's vocabulary. KYO-705's sweep for
  beta-access terminology (`hasBetaAccess`, `beta_access::`, the notice copy) came back clean
  across `crates/`, `apps/` and `scripts/` while a `kyomi-agent` doc comment still named the
  deleted `bq_kyomi_oauth_access_gate_satisfied`.
- **The comment names nothing you deleted.** It describes a *property* or a *population*:
  "both signup flows", "each view keeps its own `<AuthDivider>` call", "preserved for the
  three signup callers below", "the same name and convention as `check-ticket-in-flight.sh`'s
  own guard". There is no identifier to grep, the sentence still parses, and every word in it
  was accurate until your diff changed how many there are or what the other side does.

The disposition is not a judgement call. A comment your change falsified is not a discovery in
neighbouring code — it is a defect your change introduced, and it fails the provenance test in
[a deferral ticket is not always enough](../version-control-working-tree/a-deferral-ticket-is-not-always-enough.md).
Fix it in the same PR even when the file is outside the stated scope; KYO-703's reviewer signed
off on exactly that reasoning: *"this change is what falsified the comment, the same reasoning
that pulled `README.md` in."*

**Rule:** After a change that removes a symbol, a branch, a call site, or a guarantee, run two
sweeps before requesting review. First, grep every identifier you removed across `apps/`,
`crates/`, `enterprise/`, `scripts/`, `.github/` and the sibling repos — including places the
name legitimately survives, because that is where a comparison *to* it now lies. Second, grep the
consumers of what you changed for the words that quantify or relate: `both`, `each`, `the two`,
`the three`, `always`, `never`, `only`, `same as`, `mirrors`, `unlike`. Every hit is a sentence
that was true about a population your diff resized. Fix what you falsified in the same PR, and
state in the PR body which files you had to reach outside the ticket's scope to correct.

```sh
# WRONG — `scripts/reconcile-merged-tickets.sh` at `efb3d62e^`. The diff deleted
# check-ticket-in-flight.sh's PR_LIST_LIMIT guard outright; this header, in a file
# the diff never staged, still describes that guard as live and as the precedent
# for its own. Nothing here names a deleted symbol: `PR_LIST_LIMIT` legitimately
# survives in *this* script.
# THE PR-LISTING LIMIT IS LOAD-BEARING, THE SAME WAY IT IS IN
# check-ticket-in-flight.sh: `gh pr list` silently truncates at `--limit`
# (default 30). `PR_LIST_LIMIT` (env-overridable, same name and convention as
# check-ticket-in-flight.sh's own guard) bounds the request, and a listing
# that comes back at >= that many rows is treated as POSSIBLY TRUNCATED and
# fails closed (exit 3) rather than silently reporting only part of the
# window.

# RIGHT — the same header on `main` today, corrected in the PR that caused it.
# It states this script's own property, records that the sibling's guard is gone
# and why, and closes the path back to the wrong parity argument.
# THE PR-LISTING LIMIT IS LOAD-BEARING HERE: `gh pr list` silently truncates
# at `--limit` (default 30). ...
#
# This guard is local to THIS script. check-ticket-in-flight.sh had a
# similarly-named one and KYO-703 deleted it outright: its fetch is unbounded
# (the whole PR corpus), so the ceiling became a cliff the moment the repo
# reached it. Do not "restore parity" by copying either script's choice to
# the other.
```

Five tickets, four log days, every one caught by a reviewer rather than by the sweep:

- **KYO-703**, review log `2026-09-09`, the entry headed *"KYO-703: remove the PR-listing
  ceiling from check-ticket-in-flight.sh"* (🟢 #1) and its cycle-2 re-review — the block
  quoted above. Fixed in-diff rather than ticketed.
- **KYO-705**, review log `2026-09-10`, the entry headed *"KYO-705: remove Google OAuth
  beta-access attestation"* (🟢 #2) and its cycle-2 re-review — the `kyomi-agent`
  `classify_configured_project_scope` doc comment naming a `kyomi-ui` function the diff
  deleted. Verifiable in git: the citation is present at `545a40b7^` and gone at `545a40b7`.
- **KYO-728 phase A**, review log `2026-09-12`, the cycle-2 and cycle-3 entries under
  *"collapse passkey-vs-email signup fork"* — 🟡 twice on `google_section.rs`'s doc comment,
  first for *"each view keeps its own `<AuthDivider>` call"* after `SignupView`'s divider was
  deleted, then for the replacement's description of `CredentialsView`'s remaining two. No
  identifier was deleted; the population changed.
- **KYO-683 phase 3**, review log `2026-09-10`, the entry headed *"KYO-683 Phase 3: signup
  single-email (dead-path removal + resend fix)"* — two 🟢, both *"pre-existing doc text this
  diff didn't touch, but became inaccurate as a direct, unavoidable consequence of the
  deletions in this diff"*: *"preserved for the three signup callers below"* (two remained)
  and *"both signup flows"* (one remained).
- **KYO-700**, review log `2026-09-09`, the entries headed *"KYO-700: revoke Google OAuth
  grant on disconnect"* — `google_oauth_disconnect_service`'s doc comment said it *"mirrors
  the logic from"* the REST route, inverted the moment the diff made the route delegate to it.
  Left for a future touch at cycle 1, fixed at cycle 3.

Distinct from [a doc comment that outlived its behavior is a defect](stale-doc-comment-is-a-defect.md):
that is the same-site case — you changed a function's contract, so you update *that
function's* doc comment in the same commit, and the comment is right in front of you in the
diff. Every instance above is a comment attached to code the diff did not touch. Distinct from
[a comment must describe this code](comment-must-describe-this-code.md): that rule is
prevention, and its remedy is to stop writing cross-file comparisons at all; this one is
cleanup for comments that already exist, most of which are not cross-file references —
"the three signup callers below" points at the same file. Distinct from
[withdrawing a claim means withdrawing it from everything that carried it](withdraw-a-claim-from-everything-that-carried-it.md):
there you are correcting a claim and must chase its own scaffolding and the artefacts it cited;
here nothing is being corrected, and the falsified prose was never part of any argument.

Sibling of
[re-derive an enumeration comment from the source](re-derive-enumeration-comment-from-source.md):
that rule is how to *repair* one of these once found — read every function the sentence names,
not just the flagged one. This one is how to find it before a reviewer does. Sibling of
[enumerate a change's consumers from the type, not from the diff](../code-organization/enumerate-consumers-from-the-type-not-from-the-diff.md):
the same sweep discipline for code, where the misses compile wrong or fail a test. Here the
misses are sentences nothing checks. See also
[a count is not a safety argument](name-the-invariant-not-a-count.md) for why the tally was a
liability before your change arrived.
