# A contract's consumer can live where no grep of this repo reaches

Everything under `scripts/` and `.github/workflows/` publishes a contract: an exit-code
table, a stdout heading, a flag name, a CI job's display name, the order a check's findings
print in. The consumers of those contracts are frequently not source files, and three of the
places they live cannot be reached by any command this repo can run against itself:

- **GitHub's branch-protection settings.** The required-status-check list is server-side
  state keyed on a job's `name:`, not on its `jobs:` key. Rename the job and the required
  check simply never reports again — every PR sits pending forever on a check that no longer
  exists. Nothing in the tree records the list; only `gh api` can read it.
- **The agent skill docs, which are outside every repository.** `~/.claude/skills/*/SKILL.md`
  and `.claude/build-test.md` restate these scripts' exit codes and step ordering verbatim,
  and they are what an autonomous run actually obeys. `scripts/mine-review-logs.sh`'s own
  header already says why this is different in kind from an in-repo consumer: those files
  *"are themselves gitignored or outside any repo, so an edit there is invisible to every
  other machine and unverifiable by CI."* Canonical copies now live in
  `~/repos/kyomi-private/skills/` (KYO-568/KYO-584), which is a second repository — still not
  one a workspace grep touches.
- **Prose in this repo that encodes an ordering or a heading**, e.g. a sample-output block in
  `scripts/README.md`. Grep-findable, but only if you grep for the *output string* rather
  than for the identifier you changed.

The failure has no compile step and no test to fail. The self-test suite passes — it exercises
the script, not the documents describing it. CI is green. The break surfaces later as a PR
that can never merge, or as an agent that read a stale exit-code table and acted on it, which
is exactly the class of silent-false-completion these scripts exist to prevent.

Widening a contract is as much a change as renaming one. A new failure path that lands under
an already-documented code (`1 — error`) leaves the out-of-repo table accurate; a new *code*,
a new heading, or a new precondition does not, and the difference is only knowable by opening
the document.

**Rule:** When you change a script's exit codes, its stdout headings or their ordering, its
flag names, or a CI job's `name:`, enumerate the consumers that no grep of `apps/ crates/
enterprise/` can find, and say in the PR which ones you checked:

```bash
gh api repos/kyomi-ai/kyomi/branches/main/protection \
  --jq '.required_status_checks.contexts'            # is the old job name required?
grep -rn "<script-name>\|<heading you changed>" \
  ~/repos/kyomi-private/skills/ ~/.claude/skills/ .claude/ scripts/README.md
```

Then open each hit and read it, rather than asserting what it says: a claim *about* an
out-of-repo document is exactly as unverifiable as the document itself, and has been wrong
here. If the contract widened rather than moved, state which documented code the new path
falls under.

```bash
# WRONG — the sweep is the tree, so it stops at the repo boundary.
#   $ grep -rn "worktree-lifecycle-selftests" .        # 2 hits, both in ci.yml
#   "Renamed the job; nothing else references it."
# The `jobs:` key is not what branch protection keys on — `name:` is — and no
# file in any repo carries that list.
  worktree-lifecycle-selftests:
    name: Worktree Lifecycle & Script Self-Tests      # was: the jobs-key default

# WRONG — a new exit path added, the out-of-repo table never opened.
#   "0 tombstoned or already tombstoned, 1 error, 2 usage error"
#     — ~/repos/kyomi-private/skills/backlog-fast/SKILL.md, which the autonomous
#       loop obeys and which this PR did not read.

# RIGHT — the enumeration names each unreachable consumer and its verdict.
#   required checks = ["Clippy","Lint Suppression Policy","Trivy Security Scan"]
#     -> the renamed job is not required; the rename cannot wedge a PR.
#   backlog-fast/SKILL.md exit table -> still accurate: both new paths
#     (failed local rename after the remote was already tombstoned; sweep
#     rename failure) are genuine errors and fall under the documented `1`.
```

Real precedent — five tickets across the `2026-08-30`–`2026-09-03` window, every one of them
caught by a reviewer leaving the repo, never by a build or a suite:

- **KYO-629** (review log `2026-09-03`, `20:35`) — the `worktree-lifecycle-selftests` job
  gained a `name:`. The reviewer went to the setting rather than the tree:
  > `gh api repos/kyomi-ai/kyomi/branches/main/protection` confirms required checks are
  > exactly `["Clippy", "Lint Suppression Policy", "Trivy Security Scan"]` — the job rename
  > cannot wedge a PR on a stale required-check name.

  It was safe. It was safe by luck of which three checks are required, and the only way to
  know that was the API call.
- **KYO-596 rework** (review log `2026-09-02`, `20:31`) — `mark-branch-stranded.sh` grew two
  new failure paths. The contract lives outside the repo, and the check was whether the
  widening still fit it:
  > Checked the exit-code contract against `~/.claude/skills/backlog-fast/SKILL.md` Step 0.5
  > (outside this repo, per instructions not to edit it): it documents "0 tombstoned or
  > already tombstoned, 1 error, 2 usage error" — still accurate, since every new exit path
  > added by this rework … is a genuine error and correctly falls under exit 1.
- **KYO-607** (review log `2026-09-03`, `09:40`) — the case where the claim about the
  out-of-repo consumer was simply false. `check-ticket-in-flight.sh`'s header justifies
  keeping its conceptual check numbering because the numbers are *"used everywhere in this
  file, in `scripts/README.md`, and in both skill files"*:
  > The check numbers 1–4 do appear in this file and in `scripts/README.md` … but neither
  > `backlog/SKILL.md` nor `backlog-fast/SKILL.md` numbers them — both list the four checks in
  > that order in prose. Worse for a grepping reader, `backlog-fast` *does* use "check 1 / 1b
  > / 2 / 3" for its own unrelated Step 0.5 stranded-claim sequence.

  The don't-renumber conclusion survived; the stated evidence for it did not. A reader who
  greps the skill files for "check 4" finds nothing, or finds someone else's check 3.
- **KYO-567** (review log `2026-08-30`, cycles at `15:40`, `16:51`, `16:57`) — three separate
  review cycles each re-read the same three out-of-repo documents to confirm the 8-hour
  staleness threshold, the tombstone durability rule, and the rejected open-a-PR alternative
  were stated identically in the code and in all three. Three cycles, because there is no
  cheaper way and nothing automated to inherit.
- **KYO-628** (review log `2026-09-03`, `17:15`) — inserting a step into
  `skills/merge-sweeper/SKILL.md` renumbered its successor, so every `Step N` cross-reference
  inside the document, plus a "five buckets" tally, had to move with it. Verified with
  `grep -n "Step [0-9]"` over the whole file, not by reading the diff.

**The fix for the class, where it has been applied:** KYO-568 and KYO-584 vendored the three
canonical agent docs into `~/repos/kyomi-private/skills/`, added `link-agent-skills.sh` to
symlink `~/.claude/skills/*` at them, and wired a CI job that fails when the tracked copy and
the live file disagree. That converts an unreachable consumer into a reviewable one. It does
not remove the obligation — the docs still live in a different repository, and branch
protection still lives in no repository at all.

Sibling of
[enumerate-consumers-from-the-type-not-from-the-diff.md](enumerate-consumers-from-the-type-not-from-the-diff.md):
that rule is the same discipline for consumers that *are* source files, and its prescribed
command is a grep over `apps/`, `crates/`, `enterprise/` and `~/repos/kyomi-connect`. This
rule covers the consumers that command cannot reach by construction — a server-side setting
and a file outside every repo — where the remedy is a different command (`gh api`) or a
different tree entirely, and where the producer's identifier is often not even the string to
search for (branch protection stores `name:`, not the `jobs:` key you renamed).

Distinct from
[../build-toolchain/narrow-p-check-cannot-see-a-feature-gated-member.md](../build-toolchain/narrow-p-check-cannot-see-a-feature-gated-member.md):
there the consumer is in this workspace and a wider invocation (`--workspace`, `--features
slack`) reaches it. Here no invocation of any tool in this repo reaches it at any width.

See also
[../comments-documentation/name-the-invariant-not-a-count.md](../comments-documentation/name-the-invariant-not-a-count.md)
and its concurrency-flavoured sibling
[../comments-documentation/an-ordinal-in-a-comment-collides-with-every-concurrent-addition.md](../comments-documentation/an-ordinal-in-a-comment-collides-with-every-concurrent-addition.md)
— both are about a restated fact rotting, but within one file a reader can at least re-derive
it; the failure here is that the restatement sits somewhere the reader will never look, and
[../comments-documentation/a-resolving-identifier-is-not-a-verified-claim.md](../comments-documentation/a-resolving-identifier-is-not-a-verified-claim.md)
for the KYO-607 half specifically, where the cited document existed and the claim attached to
it did not hold.

Mined from the `2026-08-30` through `2026-09-03` review logs (KYO-567, KYO-568, KYO-584,
KYO-596, KYO-607, KYO-628, KYO-629).
