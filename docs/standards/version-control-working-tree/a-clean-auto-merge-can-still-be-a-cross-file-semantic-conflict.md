# A clean auto-merge can still be a cross-file semantic conflict

Git merges text. When two branches touch different lines of different files it reports a
clean auto-merge, and that report is true — as a statement about text. It says nothing
about whether the *combination* compiles, or whether a test one side wrote still describes
the behaviour the other side shipped. The defect class that keeps costing review cycles
lives entirely in that gap: the conflict resolution is textbook correct, every hand-resolved
hunk is right, and the merged tree is still broken.

Three shapes, in increasing order of how much will catch them:

1. **Arity / signature drift.** A call site in a file only one branch touched now disagrees
   with a signature the other branch changed. This is at least a compile error — but only in
   the targets that compile the caller.
2. **Constructor drift.** A struct gains a required field on one side; a test constructor the
   other side wrote does not have it. Also a compile error — and, when the test module is
   feature-gated, one that a default-features check never sees.
3. **Behavioural drift.** A test asserts an error message, ordering, or outcome the other side
   changed. **No compile error at all.** The merged tree is perfectly valid Rust describing
   behaviour the assertion no longer matches. Only running that test finds it.

Shape 3 is the dangerous one, for the same reason a swallowed test body is: the tooling that
is supposed to protect you is silent. `cargo check` does not run tests. A green `cargo test`
run before the merge is evidence about the pre-merge tree. And `git merge-tree` reporting
clean is evidence about text.

**Rule:** after any merge, rebase, or cherry-pick that pulls in changes to a definition your
branch also touches — a function signature, a struct, a guard, a migration, an error string —
do not stop at "the conflicts were resolved correctly." Compile **and run** the code that
exercises the union:

- `cargo check --locked --all-targets` for every crate either side touched, **with the
  feature flags that actually compile the test modules**. In this workspace some of
  `kyomi-ui`'s test modules — including `chat_engine`'s `tests_disposal_safety` — sit behind
  `#[cfg(all(test, feature = "ssr"))]`, and a default-features check silently skips them
  while still returning green. A plain `#[cfg(test)]` module is not safe either: a bare
  `cargo check` compiles no test target at all, so nothing warns you that the test module
  does not build.
- Run the tests in files only one side of the merge wrote, against the definitions the other
  side changed. A branch-only test file is exactly where shapes 1–3 hide: nothing on the other
  side of the merge ever compiled or ran it.
- For shape 3 specifically, read the assertions in those tests against the merged production
  code and ask whether the *expected value* still holds — an error message, a guard order, a
  default. If main added a guard ahead of one your test asserts on, your assertion is now
  about a path the input no longer reaches.

```sh
# WRONG — "the text conflicts were resolved correctly, so the merge is good."
# The hand-resolved hunks really were correct. Every defect was in a file git
# auto-merged with zero conflicts.
git merge --continue
cargo check --locked --workspace            # green — and compiled no test modules
git diff --check                            # green — text only

# RIGHT — compile the union's tests, with the features that reach them, then RUN
# the ones only one side authored. A check that compiles is not a verdict.
cargo check --locked --all-targets
cargo check --locked -p kyomi-ui --all-targets --features ssr

# Files only one side touched are where the drift lives. Run their tests, not
# just the files with conflict markers.
# (During a conflicted `git merge` — before `git merge --continue`, where
# `MERGE_HEAD` exists. For a merge that already committed, `MERGE_HEAD` →
# `HEAD^2` and `HEAD` → `HEAD^1`. For a rebase, diff against the commit the
# rebase started from (`ORIG_HEAD` if it has not moved); for a conflicted
# cherry-pick use `CHERRY_PICK_HEAD`, or the commit before the pick otherwise.)
git diff --name-only <merge-base>...MERGE_HEAD   # main-side changes
git diff --name-only <merge-base>...HEAD          # your-side changes
# Then, for each definition the two sets disagree about (signature, struct field,
# guard, error string), RUN the test files that consume it. Without `--features
# ssr` the second module is cfg'd out and the filter matches nothing; `--all-targets`
# on `cargo test` is belt-and-braces, not load-bearing. Confirm the run reports a
# non-zero test count — libtest exits 0 on a filter that matches nothing, which is
# the same "green that proves nothing" this rule is about.
cargo test --locked -p kyomi-auth --all-targets watch_service::duplicate_names_tests
cargo test --locked -p kyomi-ui --features ssr chat_engine::tests_disposal_safety
```

Real precedent — six critical findings across three independent PRs in two days, none of
them in a conflicted hunk:

- (private/local review log; not tracked in this repository) **KYO-214 (2026-10-09 23:52)** — 🔴, arity. The branch-only
  `watch_service/duplicate_names_tests.rs` calls `create_watch` with 11 args; the auto-merged
  signature takes 12 (`timezone` inserted after `schedule`). rustc E0061. The conflict
  resolution in `watch_service.rs` itself was *textually flawless* — proven mechanically —
  because the defect was never in that file.
- (private/local review log; not tracked in this repository) **KYO-214 (same review)** — 🔴, behavioural/schema. The branch-only
  `name_uniqueness_migration_tests.rs` seeds a "before" state at migrations `< 39`, which
  predates main's timezone migration (`00045`); the merged `get_watch` SELECTs `timezone` and
  the test panics. `cargo check` cannot see this — it is a runtime failure in a test the
  other side of the merge never compiled or ran.
- (private/local review log; not tracked in this repository) **KYO-840 (2026-10-10, cycle 1)** — 🔴, behavioural. The branch-authored
  `oauth_refresh_rejects_unbound_token_but_browser_refresh_succeeds` asserts
  `invalid_grant: client_id mismatch`; main's #571 added an earlier `is_none()` guard that
  returns `invalid_grant: refresh token is not an OAuth client token` first. The merged
  `oauth.rs` is byte-identical to `origin/main` and correct; the assertion was written against
  the branch's pre-#571 guard order. The review note names the class exactly: *"a **cross-file
  semantic merge conflict** (test assertion vs guard-order change) that text-merge and cargo
  check both miss."*
- (private/local review log; not tracked in this repository) **KYO-800 (2026-10-10 09:40, PR #547)** — three 🔴s, constructor drift. Main-only test
  constructors (`restore_tests::edited_dashboard`, `tests_disposal_safety::build_real_engine_for_disposal_test`,
  `copilot_message_projection_tracks_frames_after_session_attachment`) omit fields the branch
  made required (`copilot_receipt`, `view_context`, `historical_preview`). Auto-merge kept both
  sides; E0063 on `--all-targets`. The last two sit behind `#[cfg(all(test, feature = "ssr"))]`,
  so a default-features `cargo check -p kyomi-ui --all-targets` passed and the requester's
  clean bill relied on it.

Sibling of
[prove-a-conflict-resolution-conserved-content.md](prove-a-conflict-resolution-conserved-content.md)
and
[prove-a-conflict-resolution-conserved-every-line.md](prove-a-conflict-resolution-conserved-every-line.md):
those two are about what a *hand-resolved* text conflict may have eaten or spliced, and they
run against the conflicted file. This rule is about the files git never flagged — the ones it
merged cleanly and should not have been trusted on. All three are needed; none of them is
subsumed by another. Related:
[a-diff-comparison-is-not-evidence-of-mergeability.md](a-diff-comparison-is-not-evidence-of-mergeability.md),
which is the same epistemic error one step earlier — reasoning about what git will do from
the diffs instead of asking it.

For shape 3's underlying habit — reading an assertion's expected value off the code it is
supposed to constrain — see
[an-expected-value-read-off-the-current-behaviour-guards-the-defect.md](../testing/an-expected-value-read-off-the-current-behaviour-guards-the-defect.md).
The merge version is worse: the expected value was right when written, and the merge is what
made it wrong.
