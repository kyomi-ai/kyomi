# A count is not a safety argument — state the property that actually holds

A comment that justifies an edit with a tally (`appears exactly once in the whole file`, `two guard tests assert X`, `each retains one branch`) is making a claim a reader will verify with one `grep` — and when the tally is off by even one, the reader loses trust in the *reasoning*, not just the number. The deeper failure is that the count is usually the wrong property to have cited: a marker-based edit is safe because the real definition is the **leftmost** match, not because it is the **only** match, and the two coincide only until someone adds a test that mentions the marker string.

**Rule:** When a comment exists to explain why something is safe, name the invariant the safety actually rests on — leftmost match, first occurrence after a delimiter, uniqueness *within a scope you name*. Cite a count only when the count is itself the invariant, and derive it mechanically (`grep -c`) at the moment you write it. If you catch yourself writing "exactly once" about a whole file, check whether you meant "the first match" — you usually did, and that claim stays true as the file grows.

```rust
// WRONG — a whole-file tally, which is both wrong (3 occurrences) and not the
// reason the edit is safe. A reader greps, finds 3, and distrusts the block.
// `"fn connection_step_satisfied_from("` appears exactly once in the whole
// file, as the real definition, so the marker is unambiguous.

// RIGHT — names the property that makes it safe and survives new occurrences
// The real definition is the LEFTMOST match of
// `"fn connection_step_satisfied_from("`; later occurrences (this comment's
// marker literal, and a test that names the same symbol) are all to its right,
// so a leftmost-match replace targets the definition regardless of how many
// other mentions exist.
```

The same applies to attributing an assertion to a group of tests: say which test asserts which property, or say "one of the two guard tests". Naming two when one covers it overstates the guard and hides a real gap.

When the number *genuinely* is the contract — a fixed-size array, a protocol field width — don't stop at `grep -c`, which only proves the count correct at the moment you write it; assert it in code, where a compile-time check keeps it correct afterward:

```rust
const _: () = assert!(ROUTES.len() == 5);
```

Mined from the `2026-08-21` review log, which records three comment-accuracy findings of this shape across four review entries spanning two tickets — KYO-416's docs review and its cycle 2, and KYO-407's cycle-2 and cycle-3 re-reviews. (Date derived mechanically rather than recalled: all three appear in that day's log and in no other day's.) Deliberately cited by log date and by the *shape* of each claim rather than by file:line — those findings were logged against in-flight branch state, so their line numbers no longer resolve against `main`, and a citation a reader cannot follow is the very failure this rule is about. The three: a marker comment asserting a symbol "appears exactly once in the whole file" when it appeared three times and *leftmost match* was the property that actually made the edit safe; a follow-up comment claiming both marker strings "also occur below, in this comment" when neither appeared in the comment at all; and a standards-doc line attributing one assertion to "two guard tests" when only one made it and the sibling test checked something else. All three were 🟢 and none blocked signing — the cost is that each one invited a reader to grep and find the comment wrong. Sibling of [re-derive an enumeration comment from the source](re-derive-enumeration-comment-from-source.md): that one is about *repairing* such a comment, this one is about not reaching for a tally in the first place.
