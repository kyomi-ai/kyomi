# Name the signals a condition reads, not the boolean form it reads them in

Some comments have to describe a condition they do not own: a component doc explaining why a
divider is the *caller's* concern and not a prop, a module header summarising how its
consumers gate it, a `SAFETY`-adjacent note about what the surrounding view renders. The
comment is genuinely load-bearing — it is the justification for an API boundary — so deleting
it loses the reason. What it does *not* have to do is restate the predicate.

Restating the predicate binds the comment to a boolean form it does not control, and that
form has several independent ways to be wrong at once: how many call sites read it, which
signals each one reads, and which operator joins them. Getting one of the three right does
not constrain the other two, so a repair aimed at the flagged detail lands cleanly and a
different detail in the same sentence is newly false. Then the gate gets tuned — `&&` becomes
`||`, a second site appears — and a sentence that was carefully verified goes stale with
nothing to mark it.

The altitude that survives all of that is one notch up: **which signals participate, how many
sites read them, and whether they combine the same way.** Those facts hold across a change of
operator, and they are what the reader actually needs from a comment whose job is to explain
why the condition lives on the other side of the boundary. If someone needs the exact
predicate, it is on the screen at the site that owns it, where it drifts together with the
code.

**Rule:** When a comment must describe a condition defined in another file or component, state
the signals it reads and the shape of the relationship — how many sites, and whether they gate
the same way — rather than transcribing the operators. Reserve the exact predicate for a
comment sitting on the code that enforces it. If you find yourself on a second review cycle
re-deriving the same predicate into the same sentence, that is the signal to change altitude,
not to derive more carefully.

```rust
// WRONG — quoted from the findings that flagged them (KYO-728 phase A was
// squash-merged, so neither wording exists in any commit to quote from). Two
// successive attempts at the same sentence, each accurate about the detail the
// previous cycle flagged and wrong about a different one.
//
// cycle 2's target — false once `SignupView`'s divider was deleted:
/// ... each view keeps its own `<AuthDivider>` call ...
//
// cycle 3's target — the replacement. `CredentialsView` renders *two*
// dividers, and neither is gated on passkey-section state alone: one is
// `show_passkey_section() && show_google_section()`, the other
// `show_passkey_section() || show_google_section()`.
/// ... renders `<AuthDivider>` gated on its own passkey-section state ...

// RIGHT — `crates/kyomi-ui/src/pages/auth/components/google_section.rs` on
// `main`, verbatim. Names both signals, the site count, and that the two sites
// differ — and says none of it in terms of `&&`/`||`, so tuning either gate
// cannot falsify it.
/// Deliberately does not render any divider that may sit between this section
/// and what follows it. Whether a divider appears, and on what condition, is
/// part of the caller's own layout: `CredentialsView` renders two, each gated
/// on a different combination of its passkey- and Google-section visibility,
/// while `SignupView` renders none. A divider prop here would have to change
/// shape every time a caller's layout did, so ownership stays with the caller.
```

Real precedent — one ticket, three of its four review cycles spent on this one paragraph:

- **KYO-728 phase A, the `collapse passkey-vs-email signup fork` cycle-2, cycle-3 and cycle-4
  entries** (`2026-09-12` log) — 🟡, 🟡, then clean. Cycle 1 was signed; actioning its 🟢
  deleted `SignupView`'s divider, which falsified `GoogleSignInSection`'s doc comment in a
  second file. Cycle 2 flagged that (*"'each view keeps its own `<AuthDivider>` call' — false
  now that `SignupView`'s divider is deleted"*). Cycle 3 flagged its replacement: the new
  wording said `CredentialsView` *"renders `<AuthDivider>` gated on its own passkey-section
  state"*, and the reviewer checked the source — two dividers, gated `&&` and `||`
  respectively, both reading Google-section state too, so *"the comment's singular 'gated on
  its own passkey-section state' describes neither divider's actual predicate. A future reader
  relying on this comment (e.g. to decide whether changing `show_google_section` could affect
  `CredentialsView`'s divider) would draw the wrong conclusion."* Cycle 4 approved a third
  wording and named the rule: *"Each of the first two fixes correctly resolved the flagged
  inaccuracy but introduced a new one in the same sentence by trying to be precise about which
  signal gates which divider. The cycle-4 fix breaks the pattern by describing the
  relationship structurally (two dividers, two different combinations of the same two signals)
  rather than naming a specific predicate — worth remembering as the right altitude for
  comments describing multi-condition gates that may be tuned later: name the signals and that
  they combine differently, not the exact boolean form."* The entry records the second
  motivation explicitly: the accepted wording is *"a level of specificity that stays true if a
  gate's exact boolean form changes later"*. Both gates still read as the reviewer described
  them on `main` today.

Sibling of [name-the-invariant-not-a-count.md](name-the-invariant-not-a-count.md): the same
move against a different fragile artefact — there a tally used as a safety argument, here a
transcribed predicate. The remedies do not transfer, which is why this is its own file. That
rule's answers are "derive the count mechanically at the moment you write it" and, when the
number genuinely is the contract, `const _: () = assert!(…)`. Neither is available here: a
caller's layout gate has no compile-time assertion to pin it, and mechanical derivation is
exactly what cycles 2 and 3 did.

Distinct from
[re-derive-enumeration-comment-from-source.md](re-derive-enumeration-comment-from-source.md):
that rule is the *first* response to this shape — a finding against one claim in a sentence
does not vouch for the sentence's other claims, so re-read every function it names. It was
followed here, twice, and produced a false sentence each time, because re-deriving accurately
is only half the problem: the restatement has more independent ways to be wrong than the fact
it stands in for, and every repair re-rolls all of them. This rule is what breaks that chain
when re-derivation alone does not — stop restating the predicate. Its flagship (KYO-314) is
the same three-cycles-on-one-comment-block shape, which is the tell.

Distinct from [comment-must-describe-this-code.md](comment-must-describe-this-code.md): that
rule says a comment whose only content is a cross-file comparison can simply be deleted,
because it carries nothing a reader of *this* function needs. Here the cross-file description
is the whole point — it is why `GoogleSignInSection` has no `divider` prop — so deletion loses
the boundary's justification, and the fix is altitude rather than removal.

See also
[no-guarantee-stronger-than-code-enforces.md](no-guarantee-stronger-than-code-enforces.md):
its remedy — state an invariant once, at the site that enforces it, and reference that site
everywhere else — is the same instinct, and is the better answer whenever a single enforcing
site exists to point at. A layout gate has none: two `<Show>` blocks with different
predicates, in a file this component deliberately does not depend on. And
[withdraw-a-claim-from-everything-that-carried-it.md](withdraw-a-claim-from-everything-that-carried-it.md):
that rule is why the first cycle of this chain existed at all — deleting the divider withdrew
a claim a comment in a second file was still carrying.
