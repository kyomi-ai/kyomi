# Verify a lint fix on the toolchain that produces the lint, not just the one you have locally

A clean local `clippy` proves nothing when the lint is newer than your local toolchain. CI's `dtolnay/rust-toolchain@stable` step resolves to whatever `stable` currently is — unpinned, and it drifts out from under a local checkout that isn't kept in lockstep. During KYO-400, local stable was `rustc 1.95.0` while CI had already rolled to `1.98.0`. The new `clippy::chunks_exact_to_as_chunks` lint (introduced in 1.98) could not fire on the 1.95 toolchain at all — not "fired and got missed," but structurally absent from that clippy binary's lint table. A "clippy is clean" claim made from that machine was not a weak signal; it was vacuous. It said nothing about whether the fix worked, because the check that would have failed never ran.

This is a different failure from the general "verify against the object that ships" discipline in `version-control-working-tree/` — that family covers a working tree diverging from a commit or a remote. This is the *tool itself* diverging: the binary you ran and the binary CI runs disagree on which lints even exist.

**Rule:** before claiming a lint fix works, run `rustc --version` and confirm it is at or above the release that introduced the lint — the clippy lint's own documentation page names the version (e.g. `.../rust-1.98.0/index.html#chunks_exact_to_as_chunks`). If your toolchain is behind, `rustup update stable` before verifying, not after reporting green. State the toolchain version you verified on in the PR body next to the clippy result, so a reviewer can tell a real green from a vacuous one without re-deriving it themselves.

**WRONG** — a true statement that proves nothing:

```
$ rustc --version
rustc 1.95.0 (...)
$ cargo clippy --locked -p kyomi-core -- -D warnings
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.41s
# "clippy is clean" — but chunks_exact_to_as_chunks didn't exist in 1.95's clippy.
# This run could not have failed no matter what the code did.
```

**RIGHT**:

```
$ rustup update stable
$ rustc --version
rustc 1.98.0 (...)
$ cargo clippy --locked -p kyomi-core -- -D warnings
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.52s
# Same command, now capable of producing the failure it's supposed to catch.
```

Real precedent: KYO-400 (2026-08-21) fixed `clippy::chunks_exact_to_as_chunks` in `crates/kyomi-core/src/embedding_compat.rs`. The reviewing pass confirmed `rustc/cargo 1.98.0` on the reviewer's own shell before trusting the clean `clippy` result, and the KYO-401 re-review explicitly re-stated the toolchain version — "clean, on `rustc 1.98.0` (the toolchain the new lint requires to even fire)" — as part of the verification record rather than as incidental detail.
