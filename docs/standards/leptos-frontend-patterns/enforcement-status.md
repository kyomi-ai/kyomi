# Enforcement status — read this before trusting any rule below

Every anti-pattern in this section carries an **Enforcement:** line stating whether CI will catch a violation. There are **three** tiers, not two. Since KYO-679, **two** of the six patterns can block a merge — one unconditionally, one only on a *new* finding:

| Pattern | Enforcement |
|---|---|
| Bare `.set()` / `.update()` in deferred contexts | **blocking** — `scripts/lint/check-disposal-safety.sh` Rule A; fails CI unconditionally |
| Bare `.get()` in `Signal::derive` / `Memo::new` | **ratcheted** — same script, Rule B; silent on the 338 sites frozen in `scripts/lint/disposal-safety-baseline.txt` (388 findings, some lines recur), **fails CI (`ERROR:B`, exit 1) on anything not in that baseline** |
| Raw `spawn_local` for user-triggered mutations | **review-only** |
| `.get()` inside `<Show>` children | **review-only** |
| Reactive closure branches gating effect-owning components | **review-only** |
| Eager signal reads in `ChildrenFn` / `Arc<dyn Fn() -> AnyView>` | **review-only** |

**Do not read "the disposal-safety lint covers this" as "CI will stop me" for Rule B the way it does for Rule A.** Rule A fails unconditionally. Rule B no longer just prints `WARN:B` and exits 0 — as of KYO-679 it is a *ratchet*: a finding whose `(file, sha256-of-line)` pair is already in the checked-in baseline, within its recorded per-line count, is silent by default and does not fail the build — pass `--show-baselined` to see it, tagged `BASELINED:B`; a finding outside the baseline (a genuinely new site, or an old line recurring more times than the baseline recorded) is reported as `ERROR:B` and exits 1. Distinct baseline entries and the findings they account for, derived directly from the committed file with `wc -l scripts/lint/disposal-safety-baseline.txt` (338 lines) and `awk -F: '{sum+=$3} END{print sum}' scripts/lint/disposal-safety-baseline.txt` (388, summing each line's third field — its recorded occurrence count) — both run 2026-09-13 against this branch: **338 distinct baselined sites, accounting for 388 total findings.** (Historical: as of 2026-07-26, before the ratchet existed, the tree carried **422** live `WARN:B` findings and the lint still exited 0 unconditionally — that count is from an older `main` and is not the current baseline; do not treat it as still accurate.)

The ratchet changes what happens at the *boundary* — a new site now fails the build — but it does not make Rule B precise, and does not claim to. The 338 frozen sites are not adjudicated as correct or safe; they are simply the set that existed when the ratchet was cut, carried forward without a verdict on any individual one. That is deliberate: Rule B cannot distinguish a derive that genuinely mixes Layout-scoped and page-scoped signals from one that only reads same-scope signals, so its false-positive rate is high (of the candidates inspected during the 2026-07-25 sweep, all were false positives), and that rate has not changed. Freezing the existing set rather than fixing or exempting it sidesteps that problem entirely — it requires no verdict on any of the 338 sites — while still turning the *next* new site into a build failure. Actually resolving the frozen sites (as opposed to freezing them) needs the same syntax-tree awareness the four review-only patterns below need.

**Why the four have no tooling at all.** They are *structural*: catching them requires knowing where an expression sits in the syntax tree, which the existing pure-bash-and-awk lint cannot do. `.get()` inside a `<Show>`'s **children** is a bug; the identical token inside its `when=` prop is correct and ubiquitous — a proximity grep over `<Show` returns 221 hits, nearly all legitimate. Likewise, `spawn_local` in an `on:click` handler is a bug, but in a WebSocket handler or a `!Send` browser-API call it is explicitly sanctioned. A regex rule here would be noisy enough to get suppressed, which is worse than no rule.

An AST-aware linter (Dylint or a clippy plugin) *could* enforce them, and was evaluated and declined: it requires a pinned nightly toolchain, which the repo does not currently have — there is no `rust-toolchain.toml` and CI runs `dtolnay/rust-toolchain@stable`. That is a new ongoing maintenance commitment, judged not worth it for these four patterns.

**The cost of that trade, recorded honestly so it can be reopened with data rather than re-argued:**

- *Where blocking, the class is dead.* A 2026-07-25 sweep of `crates/kyomi-ui/src` found 132 `spawn_local` blocks containing 318 guarded `try_set`/`try_update` calls and **zero** unguarded ones. Before Rule A existed, this panic class took 12+ tickets fixed one at a time. Rule A is the only pattern here with that record, and it is still the only pattern that blocks unconditionally — Rule B's ratchet only blocks a *new* finding, and is silent on the 338 already frozen into the baseline.
- *Where review-only, it is not.* The `Effect` auth-mode pattern was documented after being caught twice (KYO-13, KYO-17) and still went missing a fourth time in `SynapseAuthModeSection` (KYO-197). KYO-226 and KYO-227 then found **28** raw `spawn_local` user-triggered mutations across 10 files — i.e. the pattern this document calls "the #1 source of WASM panics" is precisely the one with no gate.

If that second count keeps climbing, revisit the Dylint decision.

*Numbers above are point-in-time measurements from the dates given, not continuously verified. Re-measure before relying on them for a decision.*
