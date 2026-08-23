# Run CI's `--all-targets` clippy before trusting a lint fix — and don't write a test that routes around the function it claims to cover

`.github/workflows/ci.yml` (~line 376) runs `cargo clippy --locked --workspace --exclude kyomi-desktop --all-targets -- -D warnings -A clippy::unwrap_used`. `--all-targets` compiles and lints `#[cfg(test)]` code along with production code — a lint fix is not verified until that invocation is clean, not just the default `cargo clippy` a developer runs against `lib`/`bin` targets alone.

During KYO-400, a test named `as_chunks_drops_trailing_partial_chunk_like_chunks_exact` was added to prove the rewritten decode's semantics, and it called `bytes.chunks_exact(4)` directly to compare the two APIs against each other:

```
error: using `chunks_exact` with a constant chunk size
   --> crates/kyomi-core/src/embedding_compat.rs:107:14
error: could not compile `kyomi-core` (lib test) due to 1 previous error
```

The test tripped the exact lint the PR existed to fix. Had it shipped, CI would have gone red on the very PR whose purpose was making clippy pass — re-triggering the rework loop KYO-400 itself was written to end.

There is a sharper lesson underneath the mechanical one. That test asserted that `[u8]::chunks_exact` and `[u8]::as_chunks` agree with each other — a property of the **standard library**, not of this project's code. A test that can only fail if `std` regresses is not earning its place in this suite. The tell was in the test's own doc comment, which conceded it deliberately called `chunks_exact` directly instead of going through `bytes_to_embedding` (the function it was nominally testing), specifically to dodge that function's `debug_assert!`. If a test has to route around the function it claims to cover in order to compile or pass, it is testing something else, and its existence should be questioned rather than patched around.

**Rule:** when fixing a lint, run CI's exact `--all-targets` invocation, not just the default one, before calling the fix verified. When writing a test to pin a lint fix's semantics, check that the test body itself doesn't use the banned construct — and if the only way to express the test is to use it, that's a signal the behavior belongs to the language or standard library and isn't yours to pin; extend coverage of the real function instead of writing the test. Never reach for `#[allow(...)]` to make such a test legal — lint suppressions are blocked by the pre-commit hook and CI (see repo `CLAUDE.md`, *Lint Suppression Policy*).

**WRONG** — proves std agrees with itself, and fails CI on the exact lint it was meant to confirm fixed:

```rust
#[test]
fn as_chunks_drops_trailing_partial_chunk_like_chunks_exact() {
    let bytes = [0u8; 10];
    // Bypasses bytes_to_embedding's debug_assert! on purpose — see note above.
    let via_chunks_exact: Vec<_> = bytes.chunks_exact(4).collect(); // relints here
    let (via_as_chunks, _) = bytes.as_chunks::<4>();
    assert_eq!(via_chunks_exact.len(), via_as_chunks.len());
}
```

**RIGHT** — tests the actual function, through its actual entry point:

```rust
#[test]
fn roundtrip_384dim_embedding_is_byte_exact() {
    let embedding: Vec<f32> = (0..384).map(|i| i as f32 * 0.001).collect();
    let bytes = embedding_to_bytes(&embedding);
    let decoded = bytes_to_embedding(&bytes); // exercises the real debug_assert! too
    assert_eq!(decoded, embedding);
}
```

Real precedent: KYO-400 (2026-08-21), `crates/kyomi-core/src/embedding_compat.rs`. The self-consistency test was removed before the PR was submitted for review; the review log records confirming "no stray test file or leftover artifact from the earlier revision the implementer mentioned removing (`chunks_exact` self-consistency test)," and the surviving `roundtrip_384dim_embedding_is_byte_exact` test was mutation-tested against the real function instead. See also `prove-test-fails-without-fix.md` and `mutate-by-relocating-real-code.md` in this section — both are about a test's evidentiary value; this rule is about what happens when the thing under test is the wrong thing entirely.
