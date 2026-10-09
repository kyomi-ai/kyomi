# The duplicate sweep expires — re-run it at every rebase, and delete rather than cross-reference

[Name the nearest sibling rule inside the new rule](distinguish-the-nearest-sibling-rule-in-the-file-itself.md)
tells you to sweep the whole corpus before writing a rule file, and to include the branches
that haven't merged yet. Do that and you have proved one thing: no duplicate existed *at the
moment you looked*. That proof has a shelf life, and standards branches routinely outlive it.
A mined-standards commit is written in an hour, reviewed the same day, and then sits on a
branch for two or three days while the ticket it rode in on gets reworked — and the corpus it
was checked against keeps moving underneath it, because every other `/backlog-fast` run is
mining the same fortnight of `docs/review-logs/` and converging on the same loud incidents.

Rebase does not tell you. The KYO-375 one-file-per-rule layout exists so two concurrent rule
additions never collide, and it works exactly as designed: a new file at a new slug is a
clean add against a new file at another new slug, so the rebase is silent and CI is green
while a second — or third — copy of one lesson glides onto `main`. Every mechanism that would
normally catch this is looking somewhere else. Git sees no conflict. The reviewer who signed
your file swept the corpus honestly and found nothing. The reviewer of the *other* file did
the same. Both were right when they looked.

The remedy at that point is not a cross-reference. The sibling rule's "partial overlap →
state the axis" branch is for a file that is genuinely additive; a file whose whole rule and
whose flagship incident already exist on `main` is not additive, and adding a disambiguating
sentence to it just makes the third copy better-annotated. Delete it. Dropping a file you
wrote and got signed is a complete, correct outcome, and it is much cheaper than the
consolidation ticket the merged pair leaves behind.

**Rule:** Re-run the corpus sweep against the *new* base every time a standards branch is
rebased, re-reviewed, or force-pushed after conflict resolution — not only when the file was
authored. Diff the corpus between your merge base and `origin/main`, and grep the corpus for
your rule's vocabulary again. If a duplicate landed while you were in flight, `git rm` your
file and say so in the PR body. Reviewers of a rebased standards diff should treat "was this
still non-duplicative *today*" as a distinct check from "was it non-duplicative when it was
written", because a prior clean signature does not answer it.

```sh
# WRONG — the sweep ran once, at authoring time, and was never repeated. The rebase
# is silent because a new filename can never conflict with a different new filename.
ls docs/standards/*/                                       # day 1: clean
git rebase origin/main && git push --force                 # day 3: no conflict, CI green
```

```sh
# RIGHT — re-derive the answer against the base you are actually merging into.
git fetch origin
git diff --name-only "$(git merge-base @ origin/main)" origin/main -- 'docs/standards/**'
grep -rlni 'guard\|branch\|arm\|sibling' docs/standards/   # your rule's vocabulary, again
# If a duplicate landed, the file is the finding — not its cross-reference section:
git rm docs/standards/<section>/<your-slug>.md
```

Real precedent — four documented instances of one race, and the two most recent were both
resolved by deleting the file:

- **KYO-558's `rebase onto origin/main (PR #478, trivy-secret conflict merge)` review** is
  the case that names the gap. The branch's `a-guard-fixed-for-one-branch-does-not-protect-its-siblings.md`
  had already passed its own clean review under KYO-558's
  `coding standards: disposal-cfg-test-module docs` entry, which "explicitly swept
  `error-handling/` for topical overlap and found none". Between that signature and the
  rebase, two other PRs mined the identical finding onto `main` under different slugs —
  `error-handling/a-check-in-one-arm-does-not-guard-the-others.md` (PR #455) and
  `error-handling/a-guard-in-one-branch-does-not-cover-the-others.md` (PR #476) — leaving the
  branch about to add a third. The reviewer's diagnosis is the mechanism in one sentence:
  "undetected because a new-file add against a new filename never surfaces as a git conflict,
  unlike the `trivy-secret.yaml` collision that was correctly caught and merged." The
  `re-review cycle 2` entry records the resolution — the file was dropped entirely (commit
  `e3b19e1f`) "not by tracking it" — and states the boundary this rule exists to cover:
  the existing sibling-disambiguation convention "only catches a duplicate that already
  exists in the tree at review time, not one that lands on `main` later on a still-open
  branch." KYO-665 was filed for a mechanical duplicate check; this is the manual step until
  one exists.
- **KYO-658's `pre-work: new standard "a review finding names a sample, not the population"`
  review** — 🟡, same fortnight, same outcome. The proposed file overlapped
  `../code-organization/close-the-class-by-making-the-wrong-call-uncallable.md` and
  `../data-state-management/teardown-clears-the-whole-derived-state-group.md`, both already
  citing the identical KYO-468 four-cycle incident. The reviewer accepted that the claimed
  axis was "a real axis in principle" but not demonstrated, since the file "reuses the same
  incident and the same code example as both siblings" — and the file was dropped rather
  than annotated.
- **KYO-534's `New standard: a-tool-claim-needs-a-reproduction-not-a-citation.md` review**
  is the authoring-time half the sibling rule already covers, included here because it shows
  how narrow the visible surface is: the nearest sibling existed only on unmerged PR #439 and
  was "invisible to a plain `ls`". That file was fixable with one sentence *because* the
  sibling had not landed yet. Once it lands, the same overlap costs a deletion.
- **KYO-533's `land six orphaned standards files` review** is the first instance and the
  sibling rule's own flagship — a duplicate that was already in the tree, caught by the
  authoring-time sweep working correctly.

The residue is visible in the corpus today: `error-handling/` still carries both files from
the KYO-558 narrative, and KYO-666 tracks consolidating them. That is what one missed re-check
costs after the branch merges — a second ticket, and a section where `ls` shows two entries
for one lesson.

The same structural blind spot is documented in a different domain by
[../security/no-real-world-identifiers-in-a-public-repo.md](../security/no-real-world-identifiers-in-a-public-repo.md),
where a sanitisation pass and a re-introduction were in flight concurrently and "never saw
each other" — the sanitising branch's clean grep was true when it ran and false by the time
it merged. Sibling of
[distinguish-the-nearest-sibling-rule-in-the-file-itself.md](distinguish-the-nearest-sibling-rule-in-the-file-itself.md):
that rule is about the sweep you owe before the file exists, and its remedy is a
cross-reference. This one is about the sweep expiring while the file waits to merge, and its
remedy is deletion — a cross-reference cannot help once the lesson is already on `main`.
