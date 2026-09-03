# Cite what has landed — an open PR or a stranded branch is not a durable source

Documentation in this repo cites other documentation constantly, and an unusual share of
what gets cited is *in flight*. Mined standards land in batches, several worktrees are open
at once, and an abandoned branch is tombstoned under `origin/stranded/…` rather than
deleted. A citation written against one of those branches is indistinguishable, on the
page, from a citation written against `origin/main` — and it is not the same thing. The
branch can be retitled, rebased, restranded or abandoned between the moment you cite it and
the moment anyone reads it, and the reader who follows the pointer finds nothing.

Nothing mechanical catches this. No CI job or pre-commit hook in this repo checks that a
relative markdown link resolves (confirmed during the KYO-534 cycle-2 review,
`2026-08-31` `03:52`), so a link into a file that exists only on someone else's branch is
invisible until a human follows it — by which time the branch may be gone.

The forward reference is not banned; the *silent* forward reference is. A citation that
names its own state ("in flight on PR #N") is honest and self-healing. A citation that
presents unmerged work as settled precedent borrows authority the source does not have —
and a stranded branch has none at all, since "stranded" is this repo's own word for
abandoned (`scripts/mark-branch-stranded.sh`).

**Rule:** Before citing a file, PR or commit as evidence, establish where it actually lives,
then cite it accordingly:

- **On `origin/main`** → cite it plainly, by relative link.
- **In flight** → say so in the citation itself — "(in flight on PR #N)" — and pin any
  quoted content to an immutable object (`git show <sha>:<path>`), never to a moving branch
  ref. Prefer naming the file in plain text over a relative link that does not resolve yet.
- **Only on a `stranded/` branch** → do not cite it. Drop the claim, or wait for the
  re-land ticket to merge. A four-times-stranded file is not secondary evidence.

```sh
git fetch origin
git cat-file -e origin/main:docs/standards/<section>/<rule>.md   # does it exist on main?
gh pr view <N> --json state,title                                # OPEN? still that title?
git branch -a --list '*<ticket>*'                                # or is it stranded, N times?
```

```markdown
<!-- WRONG — presents an open PR and an abandoned branch as settled precedent. -->
See a-check-in-one-arm-does-not-guard-the-others.md (landing via PR #455) for the
worked example, and a-failure-path-must-emit-what-its-siblings-emit.md on
origin/stranded/jason/kyo-463-spec-green6 for a second instance.

<!-- RIGHT — a landed sibling is cited plainly, with no state marker to go stale. -->
Nearest sibling is a-diff-comparison-is-not-evidence-of-mergeability.md, whose stated
remedy — `git merge-tree --write-tree --name-only` — is narrower than this rule's.
```

Two blocking findings in one review, plus the accepted form from the week before:

- **KYO-463** (`2026-09-03`, `12:35`) — two of the four 🟡 that blocked signing
  `quote-a-wrong-block-from-the-pre-fix-commit.md` were this exact shape. The flagship
  citation pointed at `a-check-in-one-arm-does-not-guard-the-others.md` "(landing via
  PR #455)": `gh pr view 455` showed OPEN, the file was not on `origin/main`, and the live
  branch had already retitled it — so the quoted title was stale before the citing document
  merged. (The rule did eventually land, under the different slug
  [a-guard-in-one-branch-does-not-cover-the-others.md](../error-handling/a-guard-in-one-branch-does-not-cover-the-others.md),
  which is exactly the drift the citation could not survive.) The second citation reached
  through `origin/stranded/jason/kyo-463-spec-green6`; `git branch -a --list "*463*"` showed
  four separate stranded branches for that one standard. Cycle 2 (`13:40`) kept the open-PR
  citation with its state and drift narrated, and deleted the stranded-branch citation
  outright.
- **KYO-534 cycle 2** (`2026-08-31`, `03:52`) — the accepted form, signed. The nearest
  sibling existed only on unmerged PR #439, and the file cited it with an explicit
  "(in flight on PR #439)" marker. The reviewer confirmed the PR state, read the file at
  commit `e1611128` rather than at the branch tip, and recorded that the link "genuinely
  does not resolve today — it is not a typo, it is a forward reference to an unmerged
  sibling, and the … parenthetical says so rather than leaving a silently dead link."
  **That marker has since expired**: PR #439 merged as `e566f252` and the sibling is now
  plainly on `origin/main` — which is why the RIGHT block above cites it without one. A
  state marker is a debt with a shelf life, correct when written and wrong the moment the
  PR merges; prefer the citation that never needs revisiting, and reach for the marker only
  when the sibling genuinely has not landed yet.
- **KYO-607** (`2026-09-03`) — the same discipline across repos: the standards file staged
  alongside the script cited a fix that is not on `kyomi-private@main` but on the unmerged
  `origin/jason/kyo-584-agent-skills`, and the review verified every quoted phrase at commit
  `535ab1b` instead of against a branch that could move.

Nearest sibling is
[verify-a-precedent-claim-against-its-source.md](verify-a-precedent-claim-against-its-source.md):
that rule asks whether the *claim* matches the source you cited; this one asks whether the
*source* is something a future reader can reach at all. A citation can satisfy that rule
perfectly today — every quote verbatim, checked against the branch — and be dead next week.
Distinct also from
[anchor-a-citation-to-a-symbol-not-a-line-number.md](anchor-a-citation-to-a-symbol-not-a-line-number.md),
which is about drift *inside* a file that exists, and from
[distinguish-the-nearest-sibling-rule-in-the-file-itself.md](distinguish-the-nearest-sibling-rule-in-the-file-itself.md),
whose "in-flight overlap" bullet is about *discovering* an unmerged sibling — this rule
governs how you cite it once you have found it.
