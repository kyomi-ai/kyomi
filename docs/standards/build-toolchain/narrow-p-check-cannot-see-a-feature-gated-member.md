# A per-crate `cargo check -p` cannot see `enterprise/kyomi-slack`

This workspace's normal verification idiom is narrow and per-crate — `cargo check -p kyomi-agent`, `cargo check -p kyomi-ui --features ssr`, `cargo clippy -p kyomi-auth -- -D warnings`. That is the right default, and `.claude/build-test.md` explicitly recommends it: a `--workspace` check costs 20-40 minutes in a cold worktree against a couple of minutes for `-p`.

The cost of that default is a blind spot with a specific shape. `enterprise/kyomi-slack` is an unconditional workspace member (root `Cargo.toml`, `members`), but it is an *optional* dependency of `apps/server` behind a feature (`apps/server/Cargo.toml:12-16`: `default = ["slack"]`, `slack = ["dep:kyomi-slack", "kyomi-ui/slack"]`). It consumes `kyomi-agent`'s public config types directly. So a change to a shared struct's field set compiles clean under every `-p` invocation for the crates the diff touches, and breaks only under `--workspace`, `-p kyomi-slack`, or `-p kyomi-server`. The check that would have caught it is not one that failed — it is one that was never run.

This is not the same as enumerating consumers badly (`code-organization/enumerate-consumers-from-the-type-not-from-the-diff.md`). You can enumerate correctly and still ship this bug, because the enumeration and the verification are two separate acts: the sweep finds the call site, and then the *compiler command you chose* never visits it, so nothing contradicts you when you drop it. Conversely, a `-p kyomi-slack` check catches it even when the enumeration was never attempted at all.

**Rule:** when a diff changes the *shape* of a type that crosses crate boundaries — adding, removing, renaming or retyping a field on a `pub struct` like `AgentExecutionConfig` or `AgentConfig`, or changing a `pub enum`'s variants — the per-crate check is not sufficient verification. Run `cargo check -p kyomi-slack --locked` (or `cargo check --workspace --locked`) before claiming the change compiles, and say in the PR body which of the two you ran. A grep sweep is not a substitute: it tells you where the call sites are, not whether the crate still builds.

**WRONG** — every command clean, workspace broken:

```
$ cargo check -p kyomi-agent --locked      # the crate that defines the struct
$ cargo check -p kyomi-auth --locked       # a crate the diff touches
$ cargo check -p kyomi-ui --features ssr --locked
# all three clean -> "compiles clean"
# enterprise/kyomi-slack was never compiled by any of them.
```

**RIGHT**:

```
$ cargo check -p kyomi-agent --locked
$ cargo check -p kyomi-ui --features ssr --locked
$ cargo check -p kyomi-slack --locked      # the member no -p above reaches
    Finished `dev` profile [unoptimized + debuginfo] target(s)
# PR body: "AgentExecutionConfig gained a field; verified with -p kyomi-slack
#  as well as the touched crates, since enterprise/ is outside them."
```

Real precedent: KYO-492 (`docs/review-logs/2026-08-23.md`, 00:58 entry) renamed `AgentExecutionConfig::user_message_id` and declared a six-file sweep. A seventh construction site at `enterprise/kyomi-slack/src/routes.rs:1837` was left on the old name, and `cargo check -p kyomi-slack --locked` and `cargo check -p kyomi-server --locked --features slack` both failed with `E0560: struct AgentExecutionConfig has no field named user_message_id`. The review was not signed — the workspace did not build as staged. That entry's own closing note records the diagnosis: *"The narrowed verification commands specified for this review (`-p kyomi-auth -p kyomi-agent`, `-p kyomi-ui --features ssr`) do not touch `enterprise/kyomi-slack`, so they cannot catch finding #1."*

The same struct hit this again in KYO-534, which added `max_tokens` to `AgentExecutionConfig`: the ticket's own description enumerated four call sites to update, and there are five — Slack being the omitted one, exactly as before.

Sibling of `verify-lint-fixes-on-the-toolchain-that-produces-them.md`: that rule is about a toolchain that cannot produce the failure; this one is about a *crate selection* that cannot produce it. Both describe a green result that was structurally incapable of being red.
