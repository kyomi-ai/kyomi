# Build a guard with the feature set the artifact ships with

A `--features` flag on a verification command is not a build detail — it is a claim about
*which source* the check compiled. Cargo features add and remove whole item bodies, and this
workspace uses them that way: `crates/kyomi-ui/src/server_fns/slack.rs` and
`server_fns/workspace.rs` each carry paired `#[cfg(feature = "slack")]` /
`#[cfg(not(feature = "slack"))]` `#[server]` implementations *of the same function names*.
Under `--features ssr` those files compile the stubs. Under `--features ssr,slack` they
compile the real ones. Both builds succeed, both report the same shape of result, and only
one of them contains the code that ships.

The shipping feature set is rarely written down at the place you run the check. It is
assembled two files away: `apps/server/Cargo.toml` sets `default = ["slack"]` and
`slack = ["dep:kyomi-slack", "kyomi-ui/slack"]`, and `release.yml`'s two
`cargo build --release -p kyomi-server` steps pass no `--features` override at all — so the
production binary always has `kyomi-ui/slack` on, without the word `slack` appearing in the
build command anywhere. An author reading only the command they are about to type has no
way to notice that `--features ssr` is a different program.

This matters most for a *permanent* guard — a scheduled workflow, a smoke example, a
registry floor. A one-off local check built with the wrong features wastes one review cycle.
A CI job built with the wrong features is wrong on every run it will ever make, always
green, and its greenness is the entire reason nobody looks at it again.

**Rule:** when a check exists to prove something about the shipping artifact, derive its
feature set from the manifest that ships, not from the crate you are checking. Read the
consuming binary's `[features] default = [...]` and the release workflow's build command,
enable the same set, and record *why* in the place the flags live — `required-features` on
the `[[example]]`, a comment on the workflow step — so the next person editing the command
can tell a deliberate feature set from a copied one. When a feature toggles between paired
`#[cfg(feature = ...)]` / `#[cfg(not(feature = ...))]` implementations, say so explicitly:
that is the case where the wrong flag does not fail, it silently measures the other program.

**WRONG** — a guard that permanently exercises a configuration Kyomi does not produce.
This block is a reconstruction, not a quote: the reviewed state was fixed before it was ever
committed, so `git show` has no version of this file carrying `--features ssr` alone. The
flag it shows is the one the review names; the surrounding lines are illustrative.

```yaml
# .github/workflows/server-fn-lto-smoke.yml — proves inventory's server-fn
# self-registration survives [profile.release].
- run: cargo build --locked --release --example server_fn_count -p kyomi-ui --features ssr
# Green forever. It registered the `#[cfg(not(feature = "slack"))]` stubs;
# the release binary registers the real impls. The count it reports is a
# count for a build that never ships.
```

**RIGHT** — the flags match `apps/server`'s defaults, and the manifest says why:

```toml
# crates/kyomi-ui/Cargo.toml, verbatim from `[[example]]` onward with the
# first seven comment lines (the KYO-275/KYO-191 background) elided:
[[example]]
# ... what the example is and why it exists ...
# leptos::server_fn::axum, the same feature apps/server enables. It also
# needs `slack`: apps/server/Cargo.toml sets `default = ["slack"]` and
# release.yml builds `-p kyomi-server` with no `--features` override, so
# the shipping binary always compiles kyomi-ui's `slack` feature in.
# server_fns/slack.rs and workspace.rs both carry paired
# `#[cfg(feature = "slack")]` / `#[cfg(not(...))]` server_fn implementations
# — building this example with `ssr` alone would register the `not(slack)`
# variants instead of what actually ships, silently testing a feature
# combination Kyomi doesn't produce.
name = "server_fn_count"
path = "examples/server_fn_count.rs"
required-features = ["ssr", "slack"]
```

Real precedent — one incident, caught before the guard landed: KYO-275's
`LTO-profile server_fn registry smoke test` review (`docs/review-logs/2026-09-02.md`, and its
re-review under the same heading) opened `0 🔴, 1 🟡, 1 🟢`, the 🟡 being *"feature-flag build
mismatch"* — the smoke check "builds `kyomi-ui` with `--features ssr` only", while production
"always ships with the `slack` feature enabled, which changes which `#[server]`
implementations compile". The finding's verdict: *"The guard therefore does not exercise the
exact release configuration the ticket says it closes the gap for."* The re-review recorded
the resolution: `required-features = ["ssr", "slack"]` in `Cargo.toml`, `--features
ssr,slack` in the workflow build step, and the example's own docs updated to match. Both
edits are on `main` today — `crates/kyomi-ui/Cargo.toml` and
`.github/workflows/server-fn-lto-smoke.yml`.

Nearest siblings, all four describing a green result that was structurally incapable of
being red, distinguished by *what* made it incapable:

- [narrow-p-check-cannot-see-a-feature-gated-member.md](narrow-p-check-cannot-see-a-feature-gated-member.md)
  — **crate selection**. A `-p` scoping that never compiles `enterprise/kyomi-slack` at all;
  the remedy is adding a crate to the check (`-p kyomi-slack`, or `--workspace`). That is a
  different act from this one. A `-p kyomi-ui --features ssr` build names exactly the right
  crate and still compiles the wrong `#[cfg]` arms inside it, and no amount of adding crates
  to a `-p` list changes which arms that crate compiles. Conversely, enabling the shipping
  feature set does not reach a crate the command never names.
- [verify-lint-fixes-on-the-toolchain-that-produces-them.md](verify-lint-fixes-on-the-toolchain-that-produces-them.md)
  — **toolchain version**. The lint does not exist in the binary you ran.
- [a-cargo-run-that-compiled-nothing-verified-nothing.md](a-cargo-run-that-compiled-nothing-verified-nothing.md)
  — **fingerprint replay**. The right code, the right flags, and no compilation; the
  discriminator is elapsed time.
- [../leptos-frontend-patterns/a-green-host-suite-says-nothing-about-wasm32-gated-code.md](../leptos-frontend-patterns/a-green-host-suite-says-nothing-about-wasm32-gated-code.md)
  — **target selection**, the closest analogue in shape: `#[cfg(target_arch = "wasm32")]`
  bodies the host build never parsed. Its remedy is to move the decision behind a gate the
  host build *can* compile, because this workspace has no wasm test harness. Features are the
  easier case — you can just enable them — so the fix here is a flag, not a refactor.
