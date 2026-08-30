# A claim about how an external tool behaves needs a reproduction, not a citation

Plenty of this repo's correctness arguments are not about our code at all. They are
sentences about `curl`, `git`, `bash`, `journalctl` — *does `--max-time` bound the whole
download or one attempt? do two nearby edits on separate branches conflict? does a
non-fast-forward push get rejected, or silently accepted?* The answer decides whether the
change works, and none of it is visible in the diff.

The tempting evidence is the tool's own documentation, and it is not enough. Two failures
show up repeatedly, and the second is the nastier one:

- **The doc was paraphrased from memory and says something else.** A flag gets described as
  a cumulative budget when the man page says its timer resets per attempt. The fix still
  ships, still passes CI, and quietly does not do the thing its own comment claims it does.
- **The doc was quoted *correctly* and the conclusion drawn from it is still wrong.** A
  correct quote plus your own inference is still your inference, and the inference is the
  claim a reader will act on. Flag semantics compose — two time bounds interact, a merge
  algorithm's adjacency rule is not its diff renderer's context window — and composition is
  exactly where a plausible mental model diverges from behaviour.

What makes this class worth its own rule is how cheap the alternative is. Every one of the
questions above is settled by a scratch directory in under a minute: three commits and one
`git merge-tree`, one `curl` at a dead port, one `python3 -c` for a string length. There is
no reason to ship an inference about something you can simply run.

**Rule:** When the correctness of a change — or the truth of a comment, doc, or PR claim —
rests on how an external tool behaves, reproduce that behaviour in a throwaway directory and
write down what you observed, not what you cited. Quote the man page by all means, but any
arithmetic or conclusion *derived* from the quote is a new claim and needs its own check. If
a fact is mechanically computable — a byte count, a string length, a timeout product —
compute it at the moment you write it rather than estimating it.

```bash
# WRONG — the comment states a mechanism and a bound, both inferred from a
# half-remembered man page. Nothing in CI disagrees; the real bound is ~4x larger.
#   /// `--max-time` is an overall budget for the download including all of its
#   /// retries, so 300s caps the worst case at 300s per file.
CURL_ARGS=(--retry 3 --retry-all-errors --max-time 300)

# RIGHT — run it, find the flag that actually caps the total, and record the
# observation rather than the inference.
#   $ man curl | grep -A4 -- '--retry-max-time'
#     "The retry timer is reset before the first transfer attempt. ... To limit
#      a single request's maximum time, use --max-time."
#   /// `--max-time` bounds ONE attempt; `--retry-max-time` bounds the retry
#   /// series. Both are set, so the hard per-file bound is their sum.
CURL_ARGS=(--retry 3 --retry-all-errors --max-time 120 --retry-max-time 600)
```

Real precedent, `2026-08-29` and `2026-08-30`:

- **KYO-510 (`21:21`, 2026-08-29)** — 🟡, and a functional gap rather than a wording one.
  `CURL_MAX_TIME_SECS`'s doc comment called `--max-time` "an overall time budget ...
  including all of its retries"; `man curl` says the counter is reset on each retry, so the
  real worst case was "1 initial + 3 retries × up to 300s each", "~3600s for all three
  files" — undermining the stated design goal of bounding CI hang time. The fix was a flag
  the inference had hidden entirely: `--retry-max-time`.
- **KYO-510 cycle 2 (`21:29`, 2026-08-29)** — 🟢, same ticket, the other half of the failure.
  This time the man-page text in the comment was "verbatim-accurate" and only the comment's
  "own downstream arithmetic conclusion" was off: it called the bound "approximate, not a
  hard one" at ~30 minutes, when `--retry-max-time` plus `--max-time` give a deterministic
  720s per file, ~36 minutes for three. The review's own note is the rule in one sentence —
  "cycle 1 had the mechanism backwards ... cycle 2 has the mechanism right but the
  residual-overrun arithmetic is off by one `--max-time` term."
- **`a-diff-comparison-is-not-evidence-of-mergeability.md` (`20:35`, 2026-08-30)** — 🟡 on a
  standards doc, which is where this stings most. Its rationale explained git's three-way
  merge as patch-context matching: "`git diff`'s default is ±3 ... must match exactly for
  the hunk to apply." The reviewer disproved it in an isolated repo — two single-line edits
  conflict "only when adjacent (0-line gap) or overlapping; a single unchanged line between
  them already merges cleanly." The verdict is worth keeping: "a standards doc whose job is
  precision about git internals should not state a falsifiable mechanism that doesn't hold
  up under test." Cycle 2 rewrote the mechanism and re-verified it with a fresh repro, and
  the `21:15` KYO-550 rework review reproduced all three mechanisms *again* in a new scratch
  repo rather than reusing either earlier one.
- **KYO-546 (`16:52` cycle 2, fixed at `16:48` cycle 3, 2026-08-29)** — 🟢, the trivial end
  of the same class, and it still cost a review cycle: two comments described
  `kyomi-merge-sweeper-cron.sh` as "28 chars" when `python3 -c "print(len(...))"` returns
  27. A comment-only third cycle, for a fact that was one command away when it was written.

The habit that would have prevented all four shows up in the same window, applied to
questions where the plausible answer was the wrong one:

- **KYO-500 (`17:22`, 2026-08-29)** — the reviewer wrote a throwaway test (never staged) to
  probe whether a `Signal::derive`'s *own* registration owner, rather than only the signals
  it reads, decides disposal safety. It panicked, showing that 55 `lint-allow`
  justifications resting on "single-source ⇒ no disposal hazard" were "not a *rigorous
  proof*." No amount of re-reading the diff produces that answer.
- **KYO-567 (`15:40`, 2026-08-30)** — the "died between push and delete" retry path had no
  individually-named test, so the reviewer reproduced it "against a throwaway bare remote
  not used by the test suite" and confirmed idempotency directly. In cycle 2 (`16:51`) the
  "different-shas" safety claim was likewise re-derived from git's own fast-forward
  rejection rather than taken on faith.

Nearest sibling is
[a-diff-comparison-is-not-evidence-of-mergeability.md](../version-control-working-tree/a-diff-comparison-is-not-evidence-of-mergeability.md)
(in flight on PR #439), whose own cycle-1 review is the third precedent above: that rule's
remedy is narrow and mechanical — a merge-safety claim is settled by `git merge-tree` and
by nothing you can read off two diffs — where this one generalizes the failure that review
exposed, a causal claim about a tool's mechanism written out in prose and never run, to any
external tool you invoke. Sibling of
[read-the-locked-dependency-source-before-resting-on-its-semantics.md](read-the-locked-dependency-source-before-resting-on-its-semantics.md):
there the subject is a *linked library* whose source you can open at the version
`Cargo.lock` resolved, and the remedy is to read it and cite crate, version and item. Here
the subject is a *tool you invoke*, whose behaviour emerges from flag and algorithm
interaction — reading its documentation is where this rule starts, and is not where the
evidence comes from. Distinct from
[a-cargo-run-that-compiled-nothing-verified-nothing.md](a-cargo-run-that-compiled-nothing-verified-nothing.md):
that rule is about a run that happened and observed nothing; this one is about a claim for
which no run happened at all. The mechanically-computable half touches
[../comments-documentation/name-the-invariant-not-a-count.md](../comments-documentation/name-the-invariant-not-a-count.md),
which says not to reach for a tally when a *property* is what makes the code safe — this
rule covers the case where the number genuinely is the claim, and says derive it from the
tool at the moment you write it.
