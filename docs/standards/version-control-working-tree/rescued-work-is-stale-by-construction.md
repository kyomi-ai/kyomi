# Rescued work is stale by construction — recover it byte-identically, then re-verify it against current `main`

This repo now has a whole workflow for reviving abandoned work: `scripts/mark-branch-stranded.sh`
tombstones a dead run's pushed branch as `stranded/<branch>`, `scripts/mark-worktree-stranded.sh`
preserves its worktree, and a follow-up ticket lands the contents. Six of them ran in ten days
(KYO-468, KYO-533, KYO-577, KYO-585, KYO-595, KYO-602). Every artifact they recover has the same
two properties, and the two pull in opposite directions.

It is **content someone already wrote**, so re-authoring it from memory quietly substitutes a new,
unreviewed draft for the thing the ticket claims to be landing — and the substitution is invisible,
because the replacement is plausible and nobody has the original to compare against.

It is also **content written against a base that has since moved**. Every `file:line` pointer,
every "still present on `main`" claim, every "no such helper exists here" excuse, every cited
ticket status and review-log timestamp in it was true against a commit that is now days or weeks
behind. Nothing about a stranded branch decays visibly; the text reads exactly as confident on the
day it lands as on the day it was written.

So byte-identity and correctness are two separate checks, and passing the first is not evidence for
the second. A rescue that reproduces the stranded blob exactly can still land a claim that a PR
merged in the meantime already disproved.

**Rule:** land rescued work in two steps, and say in the PR body that you did both.

1. **Recover it, don't rewrite it.** `git fetch` the tombstone first (a stranded branch usually
   exists only on the remote, or only in a worktree that is itself about to be deleted), then
   `git checkout <stranded-ref> -- <path>` or `git show <ref>:<path>`. Prove the result:
   `git diff <ref> -- <path>` empty, or an `md5sum`/`git hash-object` match against the preserved
   commit. If you deliberately changed something during the rescue, that is a content edit and
   needs its own justification — not silence inside a "recovered as-is" claim.
2. **Then re-verify every factual claim against `main` at the sha you are branching from** — not
   against the state the text was written for. Line pointers, "X is still present/absent", cited
   ticket status, cited review-log entries, and any command the text tells a reader to run.
   Anything that moved gets corrected in the rescue PR. That is not scope creep; it is the
   difference between landing the work and landing a fossil.

```bash
# WRONG — byte-identity treated as the whole check. The blob matches the
# stranded commit exactly, and every claim inside it is a week old.
git checkout origin/stranded/jason/kyo-463-spec-green4 -- docs/standards/testing/foo.md
git diff origin/stranded/jason/kyo-463-spec-green4 -- docs/standards/testing/foo.md  # empty
gh pr create --title "land orphaned standard"                                        # ships stale claims

# RIGHT — conservation first, then a fresh pass over what the text asserts.
git fetch origin 'refs/heads/stranded/*:refs/remotes/origin/stranded/*'
git checkout origin/stranded/<branch> -- <path>
git diff origin/stranded/<branch> -- <path>          # must be empty: recovered, not re-authored
git log --oneline -1 origin/main                     # the base every claim must now be true against
# then, per claim: does this symbol still exist, at this path, with this shape?
grep -rn "bigquery_default_project_field_is_gone_billing_project_survives" crates/
gh pr view 410 --json state,mergedAt                 # a "still present on main" claim is a merge check
```

Real precedent — six rescues in ten days, one of which shipped a stale claim:

- **KYO-602** (`2026-09-02`, `14:20` / `14:45`) — the flagship. The rescued E2E spec
  `scripts/e2e-regression/bigquery-create-modal.cjs` carried a header list of what was *"Still
  present on `main` and not affected"*, and `Default Project` was on it. The field had been removed
  by KYO-415 (PR #410, merged) before the rescue, provable without a browser by a merged unit test,
  `bigquery_default_project_field_is_gone_billing_project_survives`
  (`crates/kyomi-ui/src/pages/settings/datasources/tests/auth_mode_sections.rs`). 🟡, not signed —
  and pointedly, the spec's own STALE ASSERTIONS block had already caught four other stale claims
  and missed this fifth one. Cycle 2 fixed it in-diff rather than deferring, and the reviewer
  confirmed the downstream ticket had been corrected too: *"the follow-up ticket now starts from
  four browser-dependent stale assertions, not five."*
- **KYO-585** (`2026-09-02`, `15:40`) — both halves done right. *"Both staged files are
  byte-identical to `origin/stranded/jason/kyo-463-spec-green4` (`git diff` against that ref,
  empty). Recovered via `git checkout <ref> --`, not re-authored."* Then: *"Accuracy against current
  `main` (`a29b4d0a`), not the 2026-08-31 vintage the files were written against"* — the cited
  clippy invocation, the `chat_engine.rs` cfg shape and `mark-branch-stranded.sh`'s
  verify-before-destroy sequence all re-confirmed at tip. Two further files the mining run had
  staged were dropped from the rescue because they would have collided with an open PR.
- **KYO-595** (`2026-09-02`, `12:15`) — `git diff FETCH_HEAD -- <path>` against
  `refs/heads/stranded/jason/kyo-463-spec-green5` (fetched fresh) empty: *"nothing was rewritten
  during the rescue."* The second half then re-established each claim against the live tree —
  `kyomi-test-tracing` really is in `crates/kyomi-auth/Cargo.toml`'s `[dev-dependencies]`,
  `validate_session_absent_does_not_log_error` really is at `mcp_session_manager.rs:480` — and
  confirmed the cited ticket was still unmerged, which is why its citations had to stay
  symbol-anchored rather than line-anchored.
- **KYO-577** (`2026-09-01`, `05:10`) — *"Verified every factual claim against the current worktree
  rather than trusting the rescued text."* The drift was real but benign: the rescued standard
  recorded `enterprise/kyomi-slack/src/routes.rs:1837` as the review log had it, while the same
  construction sits at `:1819` on current `main`. The landed file carries both, deliberately.
- **KYO-533** (`2026-08-30`, `00:33` / `00:40`) — six standards files orphaned by five dead
  `/backlog-fast` runs, landed together. Three factual corrections had already been made to the
  rescued text before review (a wrong log-entry attribution, a wrong error-variant claim about
  BigQuery's `list_projects`/`list_active_projects`, a wrong cycle count), and the reviewer
  re-derived them rather than trusting the report. The single 🟡 was the same shape: the rescued
  file never distinguished itself from its nearest sibling — exactly the check that goes stale
  while a file sits in a dead worktree and the corpus around it keeps growing.
- **KYO-468** (`2026-08-29`, `21:20`, `21:49`, `23:40`) — the conservation half, done by hand
  before the tombstone tooling existed: the rescued standards file confirmed `md5`
  `3503c54254bb19ca6de83318dadc636d`-identical to unpushed commit `40f8642a` in a dead worktree,
  via `git show`.

Distinct from [prove-a-conflict-resolution-conserved-content.md](prove-a-conflict-resolution-conserved-content.md)
and [prove-a-conflict-resolution-conserved-every-line.md](prove-a-conflict-resolution-conserved-every-line.md):
those are about conserving *both parents'* content through a hand-resolved merge. This is a single
parent, conserved trivially, whose problem is that the base underneath it moved. Distinct from
[verify-tree-is-current-before-concluding.md](verify-tree-is-current-before-concluding.md): that
rule stops you concluding something is absent from a checkout you never fetched — here the tree is
current and the *text* is what is behind. Distinct from
[verify-the-replacement-before-destroying-the-original.md](verify-the-replacement-before-destroying-the-original.md),
which governs the tombstoning step that preserved the branch in the first place; this rule governs
coming back for it. And the obligations in
[../comments-documentation/verify-a-precedent-claim-against-its-source.md](../comments-documentation/verify-a-precedent-claim-against-its-source.md)
and [../comments-documentation/anchor-a-citation-to-a-symbol-not-a-line-number.md](../comments-documentation/anchor-a-citation-to-a-symbol-not-a-line-number.md)
apply in full here, not less: rescued text is the text most likely to cite something that has since
moved.
