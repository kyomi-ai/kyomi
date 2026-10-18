# A cargo target-selection claim must not imply necessity it does not have

Standards documents explain cargo flags, and the explanation drifts toward
absolute: *"without `--all-targets` no test target is compiled at all"*, *"the
same `--features`/`--all-targets` flags as above, or the test module is not
compiled"*. Both sentences sound like a prerequisite. Neither is one. A bare
`cargo check` compiles no test target — true. `cargo check --tests` compiles
test targets without `--all-targets`, and `cargo test`'s default target
selection already compiles the lib test target, so `--all-targets` on a
`cargo test` invocation is belt-and-braces rather than load-bearing. The flag
that actually gates a feature-gated test module is `--features`.

This matters because a standards doc is read as a contract. A reader who
believes `--all-targets` is required on `cargo test` will add it everywhere,
and the next reader who omits it and gets green will not know which of the two
beliefs was right. Worse, each correction of one overabsolute claim in a shell
block has historically been followed by the same claim family reappearing a few
lines later in the same file — scoped in prose, reintroduced in a comment.

**Rule:** When a documentation comment states what a cargo flag does or
requires, scope the claim to the exact command it was observed under. Name the
flag that is genuinely load-bearing separately from the ones that are
belt-and-braces. Do not write *"X is required, or Y does not happen"* unless
omitting X from *that specific command* actually prevents Y — check
`cargo check --help` / `cargo test --help` target-selection sections before
asserting it.

```sh
# WRONG — implies --all-targets is what compiles the test module
cargo test -p kyomi-ui --locked --features ssr --all-targets \
  -- components::chat::chat_engine::tests_disposal_safety
# "--all-targets and --features, or the test module is not compiled"

# RIGHT — name the load-bearing flag, demote the other
cargo test -p kyomi-ui --locked --features ssr --all-targets \
  -- components::chat::chat_engine::tests_disposal_safety
# "--features ssr is load-bearing: kyomi-ui default = [] and the module is
#  cfg(all(test, feature = "ssr")). --all-targets is belt-and-braces; cargo
#  test's default selection already compiles the lib test target."
```

Real precedent — two review cycles of one file on 2026-10-16, with the cycle-4
reviewer tallying the pattern across that file's earlier review history:

- **KYO-432 cycle 3** (review log `2026-10-16`, `05:52`, finding 3, 🟢):
  *"without `--all-targets` no test target is compiled at all" is false as
  written — `cargo check --tests` compiles test targets without
  `--all-targets`. True for a bare check (the intent); scope it to "a plain
  `cargo check`."*
- **KYO-432 cycle 4** (review log `2026-10-16`, `06:06`, finding 1, 🟢): the
  cycle-3 fix landed in prose and the same claim family reappeared inside the
  newly added RIGHT block — *"the same `--features`/`--all-targets` flags as
  above, or the test module is not compiled" implies `--all-targets` is
  load-bearing on `cargo test`. It is not."* The reviewer's tally note:
  *"Recurring pattern confirmed a third cycle running: overabsolute claims about
  cargo target-selection flags — each cycle one gets scoped and an adjacent one
  appears."*

The tally's "third cycle" counts instances across the file's history rather
than three cycles on 2026-10-16: an earlier cycle 1 left *"compiles none of
them"* in the same file, still visible as a stale staged-blob line in the
`04:03` staging-hygiene review that day. Counting only the cycles whose *finding*
is this claim family, 2026-10-16 has two.

Sibling of
[no-guarantee-stronger-than-code-enforces.md](no-guarantee-stronger-than-code-enforces.md):
that rule forbids promising more than the code delivers; this one forbids
claiming more *requirement* than the tooling imposes. Both are the same failure
at different altitudes — a statement presented as a hard prerequisite that a
reader will restructure their workflow around, which is not actually one.
