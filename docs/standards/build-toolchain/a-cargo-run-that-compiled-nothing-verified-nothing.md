# A cargo run that compiled nothing verified nothing — record the elapsed time

`cargo check`, `cargo clippy` and `cargo test` all print the same reassuring
final line whether they compiled the crate or replayed a fingerprint hit:

```
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.54s
```

Nothing else in that output distinguishes "I linted the code in front of me and
it was clean" from "cargo decided it had nothing to do and reprinted an earlier
verdict." The exit code is 0 either way, the warning count is zero either way,
and a review or PR body that says *"clippy clean"* reads identically in both
cases. The elapsed time is the only discriminator cargo gives you, and it is
usually thrown away.

This matters because a verification claim is supposed to be *your own*
observation. When the run compiles nothing you are not reporting a check you
performed — you are relaying the cached result of a run you did not watch, made
under flags and a working-tree state you did not confirm. The two are different
claims and only one of them is evidence. On this box the crates people actually
verify have well-established costs: three independent measurements during
KYO-468 put a real `cargo clippy -p kyomi-ui --locked -- -D warnings` at 45s,
49.85s and ~35s. A 0.54s result is not a fast version of that event; it is a
different event.

The fix is cheap: touch a file in the crate under test to invalidate the
fingerprint, re-run, and write the wall time down next to the verdict so the
next reader can tell a real green from a replayed one without re-deriving it.

**Rule:** Note the elapsed time cargo reports for every `check`/`clippy`/`test`
run you intend to cite as verification. If it is implausibly small for that
crate, the run is not your evidence — `touch` a source file in the crate (or
`cargo clean -p <crate>`), re-run in the foreground, and cite *that* run. State
the elapsed time in the review log or PR body alongside the result, the same way
`verify-lint-fixes-on-the-toolchain-that-produces-them.md` requires stating the
toolchain version.

**WRONG** — a true statement about cargo, reported as a statement about the code:

```
$ cargo clippy -p kyomi-ui --locked --features ssr -- -D warnings
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.54s
# "clippy clean" — but this run compiled nothing. It is the previous
# invocation's verdict, not an observation of the diff under review.
```

**RIGHT** — force the work, then cite it:

```
$ touch crates/kyomi-ui/src/lib.rs
$ cargo clippy -p kyomi-ui --locked --features ssr -- -D warnings
    Checking kyomi-ui v0.1.0 (/home/jason/repos/kyomi/crates/kyomi-ui)
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 45.12s
# 45s is the real cost of this crate — the lint actually ran. Record
# "clean, 45s real recompile" so the number survives into the record.
```

Real precedent — four runs, two tickets, two days:

- **KYO-468 cycle 2** (review log `2026-08-29`, `21:49`): the reviewer re-ran
  `cargo clippy -p kyomi-ui --locked -- -D warnings` "after a forced `touch` to
  defeat a suspiciously-fast 0.54s cached run (real recompile: 45s, exit 0)".
  The forced run was also clean — the point is not that it caught a defect, but
  that until it ran there was nothing to be clean *about*.
- **KYO-468 cycle 3** (`2026-08-29`, `22:20`): same command, "after a forced
  `touch` (49.85s real recompile, exit 0)".
- **KYO-468 cycle 4** (`2026-08-29`, `23:40`): `cargo check`/`cargo clippy` on
  `kyomi-ui` both clean with "forced recompile via `touch`, ~35s real".
- **KYO-526 rebase review** (review log `2026-08-30`, `12:56`, PR #435): the
  habit had spread to a different reviewer on a different ticket —
  "`cargo check -p kyomi-ui --features ssr`: clean (real recompile, not cache
  hit — verified via `touch` + re-run)".

Sibling of
[verify-lint-fixes-on-the-toolchain-that-produces-them.md](verify-lint-fixes-on-the-toolchain-that-produces-them.md):
that rule is about the *binary* you ran being incapable of producing the failure
(the lint did not exist in that clippy); this one is about a capable binary
never looking at the code. Both produce a clean line that means nothing, for
different reasons, and both are invisible unless you record the one number that
gives them away — there, the toolchain version; here, the elapsed time. Kin to
[../testing/a-mutation-that-did-not-run-is-not-evidence.md](../testing/a-mutation-that-did-not-run-is-not-evidence.md),
which is the same "the run never reached the code" failure scoped to mutation
testing specifically; this rule applies to any run you cite, mutated or not.
