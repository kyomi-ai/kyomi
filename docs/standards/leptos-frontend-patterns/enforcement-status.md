# Enforcement status — read this before trusting any rule below

Every anti-pattern in this section carries an **Enforcement:** line stating whether CI will catch a violation. There are **three** tiers, not two, and only **one** of the six patterns actually blocks a merge:

| Pattern | Enforcement |
|---|---|
| Bare `.set()` / `.update()` in deferred contexts | **blocking** — `scripts/lint/check-disposal-safety.sh` Rule A; fails CI |
| Bare `.get()` in `Signal::derive` / `Memo::new` | **advisory** — same script, Rule B; prints `WARN:B` but **exits 0** |
| Raw `spawn_local` for user-triggered mutations | **review-only** |
| `.get()` inside `<Show>` children | **review-only** |
| Reactive closure branches gating effect-owning components | **review-only** |
| Eager signal reads in `ChildrenFn` / `Arc<dyn Fn() -> AnyView>` | **review-only** |

**Do not read "the disposal-safety lint covers this" as "CI will stop me."** Only Rule A does. The script's exit logic (`case "$line" in *:WARN*) ;;`) deliberately excludes `WARN`-tagged findings from the failure path, so a Rule B hit is reported and ignored. As of 2026-07-26 the tree carries **422** live `WARN:B` findings and the lint still exits 0 — which is itself the proof that Rule B is not a gate.

Rule B is advisory *by design*, not by accident: it cannot distinguish a derive that genuinely mixes Layout-scoped and page-scoped signals from one that only reads same-scope signals, so its false-positive rate is high (of the candidates inspected during the 2026-07-25 sweep, all were false positives). Gating on it would fail every build. Making it blocking requires the same syntax-tree awareness the four review-only patterns need — see below.

**Why the four have no tooling at all.** They are *structural*: catching them requires knowing where an expression sits in the syntax tree, which the existing pure-bash-and-awk lint cannot do. `.get()` inside a `<Show>`'s **children** is a bug; the identical token inside its `when=` prop is correct and ubiquitous — a proximity grep over `<Show` returns 221 hits, nearly all legitimate. Likewise, `spawn_local` in an `on:click` handler is a bug, but in a WebSocket handler or a `!Send` browser-API call it is explicitly sanctioned. A regex rule here would be noisy enough to get suppressed, which is worse than no rule.

An AST-aware linter (Dylint or a clippy plugin) *could* enforce them, and was evaluated and declined: it requires a pinned nightly toolchain, which the repo does not currently have — there is no `rust-toolchain.toml` and CI runs `dtolnay/rust-toolchain@stable`. That is a new ongoing maintenance commitment, judged not worth it for these four patterns.

**The cost of that trade, recorded honestly so it can be reopened with data rather than re-argued:**

- *Where blocking, the class is dead.* A 2026-07-25 sweep of `crates/kyomi-ui/src` found 132 `spawn_local` blocks containing 318 guarded `try_set`/`try_update` calls and **zero** unguarded ones. Before Rule A existed, this panic class took 12+ tickets fixed one at a time. Rule A is the only pattern here with that record, and it is the only blocking one.
- *Where review-only, it is not.* The `Effect` auth-mode pattern was documented after being caught twice (KYO-13, KYO-17) and still went missing a fourth time in `SynapseAuthModeSection` (KYO-197). KYO-226 and KYO-227 then found **28** raw `spawn_local` user-triggered mutations across 10 files — i.e. the pattern this document calls "the #1 source of WASM panics" is precisely the one with no gate.

If that second count keeps climbing, revisit the Dylint decision.

*Numbers above are point-in-time measurements from the dates given, not continuously verified. Re-measure before relying on them for a decision.*
