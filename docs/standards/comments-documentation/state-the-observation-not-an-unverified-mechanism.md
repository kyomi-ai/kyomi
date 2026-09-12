# A comment may state what you observed, not a mechanism you inferred from it

An experiment establishes an *outcome*: the health endpoint blocked for 34.4s and recovered to 3ms once the call was chunked. It does not, by itself, establish the *mechanism*: why one saturated task propagated across the whole worker pool. Writing the mechanism into a doc comment anyway converts a measurement into an explanation nobody ran, and the explanation is what the next reader builds on — they will reason from the asserted cause, not from the timing series that is actually solid.

This is a different failure from the two nearest siblings in this directory. [`stale-doc-comment-is-a-defect.md`](stale-doc-comment-is-a-defect.md) covers a comment that *was* true and was outlived by the code. [`no-guarantee-stronger-than-code-enforces.md`](no-guarantee-stronger-than-code-enforces.md) covers a claim about what the code *guarantees* when some other site is what enforces it. This one is about a claim of **causation**: the code and the comment both describe the present tree correctly, the fix genuinely works, and the sentence explaining *why* it works was never tested. Nothing in the diff is wrong except the explanation — which is exactly why it survives review.

The tell is a comment that reaches past the evidence in the same paragraph that cites it: an observation stated in concrete numbers, followed by a "because" clause with no number of its own. It is most tempting immediately after a hard-won fix, when the mechanism feels obvious precisely because you have just spent hours near it.

**Rule:** In a comment, state the observation and its evidence, then state the mechanism only if you confirmed it separately. If you did not, say so in the comment — correlation without a confirmed mechanism is a legitimate and useful thing to write down, and far more useful than a confident guess, because it tells the next reader which half is safe to build on. Never let a test's incidental configuration stand in for the production mechanism it happens to resemble.

```rust
// WRONG — the timing is measured, the "because" is not. `worker_threads = 1`
// is how the *regression* is made reliably detectable; it is not how
// production's blocking behaviour works, and conflating the two invites the
// next reader to "simplify" the chunking away.
/// Chunked because a single large `embed_passages` call saturates every
/// tokio worker thread, which is why `GET /api/health` stopped responding.

// RIGHT — the evidence, then an honest boundary on what it supports
/// Chunked after `GET /api/health` was measured blocked for 34.4s during
/// catalog indexing and recovered to ~3ms once batches ran via
/// `spawn_blocking` (5 runs, KYO-644). *Why* one saturated task degraded the
/// whole pool was not isolated — the correlation is solid, the mechanism is
/// not. The `worker_threads = 1` in the regression test is there to make the
/// pre-fix behaviour reliably detectable, and is not a model of production.
```

Flagged in KYO-644, where three sentences across two files asserted a propagation mechanism the run had not isolated; the review kept the timing series and replaced the causal claims with a stated correlation. The same shape recurs wherever a fix is documented by its theory rather than its measurement — compare [`a-correct-conclusion-does-not-vouch-for-its-supporting-fact.md`](a-correct-conclusion-does-not-vouch-for-its-supporting-fact.md), which is this rule's mirror image: there the conclusion is right and its cited support is wrong; here the support is right and the conclusion drawn from it was never checked.
