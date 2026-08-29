# A resolving identifier is not a verified claim

The check for a fabricated citation is an existence grep: does `DiscoveryStatus` appear
anywhere in `crates/`? That check is necessary, and
[verify-every-identifier-in-a-doc-code-example.md](verify-every-identifier-in-a-doc-code-example.md)
is right to demand it. But it answers only "is this name real?", and almost every citation
that survives to review is about a real name. The claim *attached* to the name is a separate
proposition, and nothing about the grep touches it.

That gap is not a smaller version of the fabrication problem — it is a worse one. A
fabricated identifier fails loudly the first time anyone looks for it. A real identifier
carrying a false claim passes the mechanical check, reads as verified, and is repeated
downstream by everyone who trusts the document. The reviewer who caught one put it exactly:
*"worse than a fabricated identifier, since `TestConnectionResult` does resolve, so a shallow
'does the name exist' check passes while the claim about it is false."*

The same asymmetry applies one level up, to citations of tickets, review logs, and commits.
A log file with the date you named exists; a review entry at the timestamp you named exists;
that tells you nothing about whether the finding you attributed to it is in it. Rewriting a
bad citation into a vaguer one does not fix this — it moves the unverified claim somewhere
harder to check, which is how the same paragraph gets flagged twice.

**Rule:** For every citation you write, open the thing you cited and confirm it says what
your sentence says it says. Resolve the name *and* verify the proposition: that the field
belongs to *that* struct, that the finding is in *that* log entry on *that* date, that the
count is the count. When a reviewer flags one citation in a document, re-verify all of them —
a fix to one claim is not evidence about its neighbours. If you cannot check a claim, delete
it; an unsourced sentence is worth more than a confidently wrong reference.

```rust
// WRONG — `TestConnectionResult` resolves, so an existence grep passes.
// The claim about it does not: that struct has only `success` and `message`.
//
//   "KYO-466 — `TestConnectionResult` gained a `resource_errors` map."
//
// grep -n "pub struct TestConnectionResult" -A5 crates/kyomi-ui/src/server_fns/datasources.rs

// RIGHT — read the definition, then name the struct that actually has the field.
//
//   "KYO-466 — `DiscoverResourcesResult` gained a `resource_errors` map."
//
// Verified: `DiscoverResourcesResult` in crates/kyomi-ui/src/server_fns/datasources.rs
// declares `pub resource_errors: HashMap<String, String>`; `TestConnectionResult`,
// declared immediately above it, has only `success` and `message`.
```

The same move for a review-log citation — grep the log for the claim, not just for the file:

```sh
# Don't stop at "the log exists". Confirm the finding is in the entry you named.
grep -in "exactly once\|guard test" docs/review-logs/2026-08-22.md || \
  echo "NOT IN THIS LOG — cite the one that has it"
```

Three diffs in three days, none of them caught by an existence check:

- **KYO-440** (review log `2026-08-24`, cycle 1 at `13:19`, finding #2) —
  `docs/standards/code-organization/enumerate-consumers-from-the-type-not-from-the-diff.md`
  stated that `TestConnectionResult` "gained a `resource_errors` map". Both structs are
  declared in the same file a few lines apart; `resource_errors` belongs to
  `DiscoverResourcesResult`. The wrong one was cited in a rule whose own subject is
  enumerating consumers *from the type*. Corrected on `main`.

- **KYO-429** (review log `2026-08-22`, cycles 1 at `05:06` and 2 at `05:45`) — one paragraph
  in `docs/CODING_STANDARDS.md`, flagged twice. Cycle 1: three cited incident locations, none
  of which corresponded to real content — the reviewer read line 9999 and found an unrelated
  test. Cycle 2: the *rewrite* replaced the file:line pointers with a log-date citation and
  was wrong again, in the way this rule is about — it named the `2026-08-21` **and**
  `2026-08-22` logs when all three findings live only in `2026-08-21`, and framed them as "one
  review pass" when they come from three separate entries (`16:24 — KYO-416`, and KYO-407
  re-review cycles at `21:40` and `21:25`). Both logs existed; the attribution did not
  survive being opened. Cycle 2's own note: *"the same failure mode one level up."*

- **KYO-433** (review log `2026-08-23`, cycles at `12:27` / `12:31` / `12:34`) —
  [verify-every-identifier-in-a-doc-code-example.md](verify-every-identifier-in-a-doc-code-example.md)'s
  motivating case, and the reason to treat these as one family. Cycle 1 found two
  prose citations naming the wrong signal and the wrong count; cycle 2, after both prose
  fixes landed, found a fabricated enum variant in the code blocks, untouched because cycle
  1's findings had been about prose. Fixing the flagged claim is not fixing the document.

Related: [anchor-a-citation-to-a-symbol-not-a-line-number.md](anchor-a-citation-to-a-symbol-not-a-line-number.md)
(what to cite, so the pointer survives), [re-derive-enumeration-comment-from-source.md](re-derive-enumeration-comment-from-source.md)
(re-derive rather than copy), [no-guarantee-stronger-than-code-enforces.md](no-guarantee-stronger-than-code-enforces.md)
(the same overclaiming failure in production comments), and
[verify-a-precedent-claim-against-its-source.md](verify-a-precedent-claim-against-its-source.md)
(the same family, split by whether a mechanical check exists: that rule covers a precedent
claim in prose that nothing can grep for, while this one covers a citation whose grep
passes — and it is the passing grep that supplies the false confidence).
