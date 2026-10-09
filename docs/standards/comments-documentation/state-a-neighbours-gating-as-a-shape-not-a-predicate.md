# State a neighbour's gating as a shape, not as its predicate

Some comments have to describe a condition they do not own: a component doc explaining why a
divider is the *caller's* concern and not a prop, a module header summarising how its
consumers gate it, a `SAFETY`-adjacent note about what the surrounding view renders. The
comment is genuinely load-bearing — it is the justification for the boundary — so deleting it
loses the reason. The tempting way to write it is to restate the condition itself: *"`CredentialsView` renders `<AuthDivider>`
gated on its own passkey-section state."* That is a claim about code in another file, nothing
type-checks it, and it is wrong the next time that file's layout moves — which is exactly what
the comment concedes will happen, since the whole point of the paragraph is that the decision
lives over there.

It is also surprisingly hard to get right at authoring time. The neighbour usually has more
than one call site, and their conditions differ. Three consecutive review cycles went into one
such paragraph, each repair accurate about the clause a reviewer had flagged and newly wrong
about the clause beside it. The version that finally held names no condition at all: it says
how many dividers there are, that each is gated on a different combination of the same two
signals, and that the sibling view has none.

Restating the predicate binds the comment to a boolean form it does not control, and that form
has several independent ways to be wrong at once: how many call sites read the condition, which
signals each one reads, and which operator joins them. Getting one of the three right does not
constrain the other two, so a repair aimed at the clause a reviewer just flagged lands cleanly
while a different clause in the same sentence goes stale unnoticed — which is exactly the shape
the three cycles below took, one wrong detail at a time.

**Rule:** When a comment must describe a neighbour's behaviour, state the shape a reader can
rely on — how many of the thing there are, that they are gated differently, which side owns the
decision — and stop before restating the condition itself. The exact predicate is already
stated exactly, in the other file, and it is the part that moves. Reserve it for a comment
sitting on the code that enforces it; if you find yourself on a second review cycle re-deriving
the same predicate into the same sentence, that is the signal to change altitude, not to derive
more carefully. When the neighbour's precise behaviour genuinely is load-bearing for this file's
contract, do not settle it with a tighter sentence: enumerate per call site and say plainly what
is *not* guaranteed (see
[no-guarantee-stronger-than-code-enforces.md](no-guarantee-stronger-than-code-enforces.md)),
or change the code so the claim holds and the comment no longer has to carry it.

```rust
// WRONG — reconstructions, not quotes: both forms were corrected during review and
// never committed, so there is no `<sha>^` to quote. Text as the review log records it.
//
// cycle 1 →  "...each view keeps its own `<AuthDivider>` call."
//            False as soon as the same PR deleted SignupView's divider.
// cycle 2 →  "`CredentialsView` renders `<AuthDivider>` gated on its own
//             passkey-section state."
//            False too: it renders two, and neither is gated on that signal alone.

// RIGHT — quoted from `crates/kyomi-ui/src/pages/auth/components/google_section.rs`,
// landed on `main` in `ac4a1a55` (PR #519, squash-merge of
// `origin/jason/kyo-728-confirm-credentials`). No condition is restated, so no
// edit to login.rs's `<Show when=...>` expressions can falsify it.
/// Deliberately does not render any divider that may sit between this section
/// and what follows it. Whether a divider appears, and on what condition, is
/// part of the caller's own layout: `CredentialsView` renders two, each gated
/// on a different combination of its passkey- and Google-section visibility,
/// while `SignupView` renders none. A divider prop here would have to change
/// shape every time a caller's layout did, so ownership stays with the caller.
```

Real precedent — one paragraph over three cycles, and one landed comment that took the other
way out:

- **KYO-728 phase A** — review log `2026-09-12`, headings *"KYO-728 phase A: collapse
  passkey-vs-email signup fork"* cycles 2, 3 and 4. Cycle 1's own 🟢 asked for `SignupView`'s
  leftover divider to be deleted; doing so falsified the sibling doc comment on
  `GoogleSignInSection`, which still said each view kept its own `<AuthDivider>` call (cycle 2,
  🟡). The repair named `CredentialsView`'s gate instead, and was wrong again (cycle 3, 🟡):
  `CredentialsView` (`crates/kyomi-ui/src/pages/auth/login.rs`) renders **two** dividers — the
  `text="or"` one gated `show_passkey_section() && show_google_section()`, and the
  `text="or sign in with email"` one gated `show_passkey_section() || show_google_section()` —
  so neither depends on passkey state alone (`google_section.rs:18-23`): *"the comment's singular
  'gated on its own passkey-section state' describes neither divider's actual predicate. A
  future reader relying on this comment (e.g. to decide whether changing `show_google_section`
  could affect `CredentialsView`'s divider) would draw the wrong conclusion."* Cycle 4's
  paragraph names both signals instead of
  one, without pinning to the exact operators — the entry calls this *"a level of specificity
  that stays true if a gate's exact boolean form changes later"*. Cycle 4 signed clean on the
  structural form quoted above; the reviewer's log entry names the pattern explicitly: *"Three
  consecutive cycles (2, 3, 4) were spent on this single doc-comment paragraph. Each of the
  first two fixes correctly resolved the flagged inaccuracy but introduced a new one in the same
  sentence by trying to be
  precise about which signal gates which divider. The cycle-4 fix breaks the pattern by
  describing the relationship structurally (two dividers, two different combinations of the
  same two signals) rather than naming a specific predicate — worth remembering as the right
  altitude for comments describing multi-condition gates that may be tuned later: name the
  signals and that they combine differently, not the exact boolean form."* Landed on `main` in
  `ac4a1a55` (PR #519), the squash-merge of branch `origin/jason/kyo-728-confirm-credentials`
  (`ab70a330`) — unmerged at authoring time, merged by the time this rule was rescued.
  Conditions above were confirmed directly against `main`, not the review log.
- **KYO-704 phase A** — landed in `686c9a91` (PR #513), review log `2026-09-11`, headings
  *"KYO-704 Phase A: retire BigQuery kyomi_oauth, default to service_account"* cycles 1 to 3.
  `check_credential_status`'s doc comment (`crates/kyomi-auth/src/datasource_auth_service.rs`)
  claimed its call sites all deny a `"retired_auth_mode"` row because each allows only
  `"valid"`/`"shared"`. Two cycles found that false at two different sites — one whose
  `can_enable` also falls back to a `user_enabled` preference that defaults to `true`, and one
  whose shared-auth arm never consults the function at all. Because the behaviour genuinely was
  load-bearing here, the resolution was not a softer sentence: the code gained a retired-mode
  guard where the claim had to hold, and the landed comment enumerates each call site and states
  the limit outright — *"this function guarantees only that the value is reported distinctly and
  by name, not that every caller fails closed on it uniformly."* Three review cycles for one doc
  comment that was wrong in two different ways.

Distinct from [re-derive-enumeration-comment-from-source.md](re-derive-enumeration-comment-from-source.md):
that rule is the repair procedure once the comment *is* going to enumerate behaviour across
named functions — re-read every one of them, not just the flagged one, because a finding
against one claim does not vouch for the rest. It is what KYO-704's resolution followed. It was
also followed here, twice, and produced a false sentence each time, because re-deriving
accurately is only half the problem: the restatement has more independent ways to be wrong than
the fact it stands in for, and every repair re-rolls all of them. This rule is what breaks that
chain when re-derivation alone does not — stop restating the predicate. Its flagship (KYO-314)
is the same three-cycles-on-one-comment-block shape, which is the tell. This rule sits one step
earlier and applies where the enumeration was never load-bearing: for a neighbour's gating
condition, the cheapest correct comment is the one that does not name it.

Distinct from [name-the-invariant-not-a-count.md](name-the-invariant-not-a-count.md): both rules
ask the same question — which half of this sentence survives the next edit? — and answer it
differently because the volatile term differs. There a tally is the perishable part and the
property is durable; here the *count* ("renders two") is what stays true across layout edits and
the *condition* is what rots. Reach for whichever of the two the other file's authors are not
about to change, rather than for a fixed preference between counts and properties. That rule's
own answers — derive the count mechanically at the moment you write it, or, when the number
genuinely is the contract, pin it with `const _: () = assert!(…)` — are not available here: a
caller's layout gate has no compile-time assertion to pin it, and mechanical derivation is
exactly what cycles 2 and 3 did, each time landing on a fresh wrong predicate rather than a
stale count.

Distinct from [comment-must-describe-this-code.md](comment-must-describe-this-code.md): that
rule covers a comment whose only content is a comparison to another file, and its remedy is
deletion — nothing is lost. Here the reference is the content: the paragraph exists to record
that the caller, not this component, owns the decision, and deleting it would invite the next
author to add the divider prop it argues against.

See also [no-guarantee-stronger-than-code-enforces.md](no-guarantee-stronger-than-code-enforces.md):
its remedy — state an invariant once, at the site that enforces it, and reference that site
everywhere else — is the same instinct, and is the better answer whenever a single enforcing
site exists to point at. A layout gate has none: two `<Show>` blocks with different predicates,
in a file this component deliberately does not depend on.

See also [read-back-the-whole-block-you-edited.md](read-back-the-whole-block-you-edited.md) and
[withdraw-a-claim-from-everything-that-carried-it.md](withdraw-a-claim-from-everything-that-carried-it.md):
in KYO-728 the sentence went stale because of the fix cycle 1 had asked for — a deletion in
`login.rs`, a different file from the one the next cycle's finding named. A change that removes
the thing a comment describes has to walk to the comment.
