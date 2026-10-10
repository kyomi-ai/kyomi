# A narrowed clippy run can report `dead_code` CI will never see

`scripts/preflight-clippy.sh -p <crate>` is the fast, correct default for verifying a diff
— `.claude/build-test.md` recommends it over a `--workspace` run for exactly the reason
`narrow-p-check-cannot-see-a-feature-gated-member.md` names: minutes instead of tens of
minutes. That file and `build-a-guard-with-the-features-the-artifact-ships-with.md` both
describe the cost of narrowing as a **false negative** — a crate or a `#[cfg]` arm the
command never compiles, so a real defect ships clean. This is the other direction: a
**false positive**. `preflight-clippy.sh -p kyomi-ui`'s pass 2 (no `--features` override)
reports `credential_status_indicates_connected`
(`crates/kyomi-ui/src/pages/settings/datasources.rs:77-78`) as `dead_code`. Under
`--features ssr` — and under the workspace build, where `apps/server`'s dependency on
`kyomi-ui` pulls the same feature in via Cargo's feature unification — the identical
symbol is live and the warning is gone. The full command CI actually runs
(`cargo clippy --locked --workspace --exclude kyomi-desktop --all-targets`, per
`docs/CODING_STANDARDS.md`'s CI-parity requirement) never produces this finding at all.

The trap is not that the finding is wrong — it's that nothing about the command's output
says so. `dead_code` reads exactly like a real, fixable lint, indistinguishable from one
that would also fail in CI, and every reviewer who hits it has to spend real effort
re-deriving that it's a narrowing artifact rather than a regression the diff introduced —
by rerunning with `--features ssr`, or by confirming the diff didn't touch the file at all.
That cost is paid over and over, by different agents, on different tickets, because the
check itself carries no signal distinguishing "real" from "narrowing artifact."

**Rule:** before treating a `dead_code` (or any reachability-sensitive) finding from a
narrowed `-p` clippy/check run as real — enough to justify deleting code, or to block a
review — re-run it under the feature set the workspace build actually unifies to
(`--features ssr` for `kyomi-ui`, or `cargo clippy --workspace`) before concluding anything.
If it's a pre-existing finding unrelated to your diff, don't silently reproduce the
re-derivation from scratch each time: name the tracking ticket (`KYO-723` for this
mechanism) and confirm your diff doesn't touch the file, rather than re-litigating whether
the finding is real.

```
WRONG — the narrow result is taken at face value:

$ bash scripts/preflight-clippy.sh -p kyomi-ui
pass 2: warning: function `credential_status_indicates_connected` is never used
# "clippy found dead code, let's delete it" — deletes a function the shipping
# build actually calls through apps/server's default `ssr` feature unification.

RIGHT — re-checked against the feature set that ships before acting on it:

$ bash scripts/preflight-clippy.sh -p kyomi-ui
pass 2: warning: function `credential_status_indicates_connected` is never used
$ cargo clippy --locked -p kyomi-ui --features ssr --all-targets -- -D warnings
   Finished                                                    # clean — no such warning
# Confirmed narrowing artifact (KYO-723, pre-filed); diff doesn't touch the
# file; not this PR's defect, not deleted.
```

Real precedent — the same symbol, the same misfire, across at least five distinct tickets
over a week, each requiring its own re-derivation:

- `docs/review-logs/2026-09-09.md` (KYO-682 review, panic-overlay `console_errors` fix):
  *"pre-existing `dead_code` warning on the host test build,
  `crates/kyomi-ui/src/pages/settings/datasources.rs:78`
  (`credential_status_indicates_connected` never used) — untouched by this diff, reported
  not fixed."*
- `docs/review-logs/2026-09-10.md` (KYO-725, chart-builder SQL editor fix): *"pass 2's lone
  `dead_code` on `datasources.rs:78` independently reproduced as a narrowing artifact, not
  present with `--features ssr`."* Re-confirmed verbatim in that ticket's cycle-2 re-review.
  The same log's beta-access-removal review calls it *"pre-existing
  `credential_status_indicates_connected` dead-code narrowing artifact in pass 2, confirmed
  untouched by this diff and reachable only from a `#[cfg(target_arch = "wasm32")]` call
  site plus an `ssr`-feature test module."*
- `docs/review-logs/2026-09-13.md` — three separate reviews on one day. KYO-536 (`10:23`)
  and KYO-684 (`17:30`) each hit it on unrelated diffs, and KYO-679 (the Rule B
  disposal-safety lint ratchet) hit it in *both* of its review cycles (`00:00` and `01:00`),
  each naming *"the pre-existing, separately-ticketed (KYO-754) `dead_code` failure"* and
  confirming via `git diff --cached` that the flagged file is untouched. KYO-679 changed no
  Rust files at all, and still had to reason about it twice.
- `docs/review-logs/2026-09-14.md` (KYO-697 email-observability fix, three review cycles):
  cycles 1 and 2 both hit *"pass 2 fails only on the pre-declared KYO-754
  `credential_status_indicates_connected` dead_code."* Cycle 3 is the sharpest evidence for
  why this can't be trusted as a stable signal either way: *"`-p kyomi-server -p kyomi-ui`
  CLEAN on all three passes — including `kyomi-ui` pass 2, so the pre-declared KYO-754
  `dead_code` failure did not reproduce here."* The same narrow command, same crate, same
  untouched file, different result on a different run — the finding's presence or absence
  depends on incidental build-graph state, not on the diff under review.

**Cite `KYO-723`, not `KYO-754`.** KYO-723 — *"preflight-clippy.sh -p kyomi-ui reports a
false dead-code error that CI never sees — narrowing the scope drops feature unification"*
— is the ticket that owns the mechanism and fixing its underlying cause, either by making
`preflight-clippy.sh`'s pass 2 unify the same features `apps/server` does, or by resolving
whatever makes the symbol legitimately unreachable outside `ssr`.

KYO-754 is a *duplicate of that*, filed later by an agent who hit this exact symbol and
read the narrowed result as a real CI failure. Its description was subsequently falsified
in place and now carries a correction block recommending it be closed as a duplicate of
KYO-723 — because implementing it as written would delete
`credential_status_indicates_connected`, a function the shipping build genuinely calls.
Several of the review-log entries above cite KYO-754 simply because it is the more recent
ticket, and the first draft of *this file* copied that citation for the same reason —
caught only by opening KYO-754 and reading its correction block. Follow a ticket reference
to the ticket before repeating it. A duplicate whose premise was disproved is the worst
possible thing to point a future agent at, because acting on it causes exactly the deletion
this rule exists to prevent. See also
[anchor-a-citation-to-a-symbol-not-a-line-number.md](../comments-documentation/anchor-a-citation-to-a-symbol-not-a-line-number.md)
for the same discipline applied to code references.

This rule is about what to do with the finding *until* KYO-723 lands: verify before acting,
cite before re-deriving.

Nearest sibling in this file's own family, all four describing a check that reports the
wrong thing about the shipping artifact, distinguished by *which direction* it's wrong:

- [narrow-p-check-cannot-see-a-feature-gated-member.md](narrow-p-check-cannot-see-a-feature-gated-member.md)
  and [build-a-guard-with-the-features-the-artifact-ships-with.md](build-a-guard-with-the-features-the-artifact-ships-with.md)
  — **false negative**. A crate or `#[cfg]` arm the narrow command never compiles at all, so
  a real defect ships clean. The fix is adding scope to the command.
  This file — **false positive**. The narrow command compiles the file, but under a
  feature configuration nothing ships with, and reports a defect that doesn't exist in the
  configuration that does. The fix is the same shape (match the shipping feature set) but
  the failure mode it prevents is the opposite: wasted effort and a live risk of "fixing" a
  finding that was never real, rather than a defect slipping through unnoticed.
