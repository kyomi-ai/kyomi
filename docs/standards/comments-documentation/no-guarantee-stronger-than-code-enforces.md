# Never claim a guarantee stronger than the code enforces

This is not the stale-comment case above — it is a comment that was never true. Two shapes recur: a doc that states an invariant as an inherent property when some *other* function is what actually enforces it, and a `SAFETY` block that asserts the lock/guard excludes more than it does. Both read as verified facts, both are trusted over the code, and both survive review easily because nothing type-checks a claim inside a `///`. The tell is a comment that contradicts a neighbouring doc or test — if two comments in one module state different guarantees, at least one is wrong.

**Rule:** State an invariant once, in the function that enforces it; everywhere else reference that site instead of restating the property. In a `SAFETY` comment, name the half of the obligation you discharge *and* the half you do not — a deliberately accepted, documented risk is honest engineering; silently widening the guarantee is a defect. When in doubt, read the upstream contract (std's own docs, the module doc) rather than paraphrasing it from memory.

```rust
// WRONG — asserts reader-exclusion the mutex does not provide, and directly
// contradicts the module doc's own "What this guard does not guarantee"
// SAFETY: the lock is held, so no other thread can be calling
// `set_var`/`remove_var`/`var` concurrently with this call.
unsafe { std::env::set_var(key, value) };

// RIGHT — names what is covered and what is deliberately not
// SAFETY: the crate-wide lock is held, which discharges only the writer half
// of `set_var`'s contract (no concurrent mutators). Concurrent *readers* are
// not excluded — see the module doc's "What this guard does not guarantee".
unsafe { std::env::set_var(key, value) };
```

Flagged in KYO-240 (`current_user_id_from`'s doc asserted "an empty id can never match a real `member.user_id`" as inherent, when only the guard inside `is_owner_from` makes it true) and in KYO-318 cycle 2, where the per-call `SAFETY` comments claimed the env lock excluded concurrent *readers* while the module doc said the opposite — the contradiction passed a full review pass and was only caught by CI after merge.
