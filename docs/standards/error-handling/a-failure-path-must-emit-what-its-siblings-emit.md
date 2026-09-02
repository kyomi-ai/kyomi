# A failure path must emit what its siblings emit

Every failure path has two outputs: the value it returns, and the diagnostic it leaves
behind for whoever has to explain the outage. Only the first one is defended. It has a
type, a test, and a reviewer reading the `Err` arm. The second is a string literal that
nothing type-checks, nothing asserts on, and no CI job notices the absence of — so it is
the half that quietly goes missing while the diff reads as behaviour-preserving.

It goes missing in four shapes, and the review log for this window has one of each:

- **A consolidation homogenises it.** Two paths are correctly routed through one shared
  helper, whose message replaces both callers' distinct ones. The error *detail* survives —
  often the diff's doc comment says so, carefully and correctly — but the message string an
  operator greps or alerts on does not.
- **A re-implementation drops it.** A small helper is re-derived inline instead of called,
  and the inline copy is the version from before the ticket that added the log.
- **A path leans on the language's implicit failure.** `set -e`, a bare `?` — the run does
  stop, fail-closed and correct, but silently, while every other failure arm in the same
  file announces itself.
- **It goes to the ephemeral channel, not the durable one.** The warning is on stderr, in a
  run nobody is watching, when the artefact a human reads later is the stdout summary.

None of these is caught by asking "does it still fail correctly?" — all four do. They are
caught by asking "does it still say the same thing?"

**Rule:** when you add, move, consolidate or re-implement a failure path, hold it against
its sibling failure paths in the same file, trait or script and compare three things:
the **channel** it writes to (`tracing::error!` vs stderr vs the durable stdout summary),
the **level**, and whether the **message string is still distinguishable** from its
siblings'. Preserving the error's `Display` detail is not the same as preserving the
message: the detail is what you read once you have found the line, the message is how you
find it. If the path used to emit something and now does not, that is a behaviour change —
put it in the PR body per
[../version-control-working-tree/declare-the-change-the-ticket-did-not-ask-for.md](../version-control-working-tree/declare-the-change-the-ticket-did-not-ask-for.md),
or keep the diagnostic. And pin it with an assertion, because it is assertable:
`kyomi-test-tracing` is already a `[dev-dependencies]` entry in both `kyomi-auth` and
`kyomi-ui`, and a shell suite can assert on the summary text the script prints.

Two of the three WRONG blocks below are marked as reconstructions: those versions were
fixed before merge, so there is no commit to quote them from. Everything else — both RIGHT
blocks and the third WRONG block, which is the shipped `if`/`elif` chain with its
catch-all removed — is quoted from `main`, with elisions marked.

```rust
// WRONG (reconstructed) — a local re-derivation of the crate's own gate
// helper. It fails closed, and its four tests cover all four arms
// (present/absent/error/loading); none of them asserts on a log, so nothing
// catches that the `tracing::warn!` KYO-240 added is gone for this one gate
// and "every permission-gated surface vanished" now has no trail.
fn can_transfer_ownership_from(user_ctx: Option<Result<UserContext, ServerFnError>>) -> bool {
    user_ctx
        .and_then(|r| r.ok())
        .map(|ctx| ctx.permissions.contains(&Permission::TransferOwnership))
        .unwrap_or(false)
}

// RIGHT — call the helper that already owns the diagnostic. `permissions_from`
// logs the failed fetch and deliberately stays silent while merely loading,
// and both halves are pinned by tests (`permissions_from_failed_fetch_yields_
// no_permissions_and_logs`, `..._loading_yields_no_permissions_without_logging`)
// in crates/kyomi-ui/src/utils/permissions.rs.
let perms = use_permissions();
let can_transfer_ownership = Memo::new(move |_| perms.can(Permission::TransferOwnership));
```

```sh
# WRONG (reconstructed) — fails closed, silently, in a script whose every
# other failure arm names the problem first. The operator gets a bare
# non-zero status.
CLAUDE_HOME_PATH="$(mkdir -p "$CLAUDE_HOME_PATH" && cd "$CLAUDE_HOME_PATH" && pwd)"

# WRONG — the rename-failed warning exists, but only on stderr, while the
# block the script's own header calls "pasteable into a Trakkt release
# comment" ends after three explicit arms. The reachable fourth state
# (branch exists, not checked out, `git branch -m` failed) matches none of
# them, so the durable record looks identical to a clean run.
if [ "$LOCAL_RENAMED" -eq 1 ]; then
    echo "  - Local:   renamed local branch '${BRANCH}' -> '${STRANDED_BRANCH}'"
elif [ -n "$LOCAL_BRANCH_CHECKED_OUT_AT" ]; then
    echo "  - Local:   left in place, checked out at ${LOCAL_BRANCH_CHECKED_OUT_AT} ..."
elif [ "$LOCAL_BRANCH_EXISTS" -eq 0 ]; then
    echo "  - Local:   no local branch of that name in this repo"
fi

# RIGHT — a catch-all `else` (verbatim from scripts/mark-branch-stranded.sh,
# remedy lines elided), so the state reaching none of the named arms reprints
# the warning into the durable summary rather than only into stderr. Test 2b
# forces the rename failure and asserts on this stdout text.
else
    echo "  - Local:   NOT renamed — 'git branch -m' failed for local branch '${BRANCH}'."
    echo "             Remote is correct; other workers are unaffected. This machine has a"
    # ... three lines naming the false-HIT consequence and both remedies, elided ...
fi
```

Four findings, four tickets, two languages, three days — three of them filed by the
reviewer under the same anti-pattern category, *Missing Error Context*:

- **KYO-557** (review log `2026-08-31`, `00:00`, 🟢) — the consolidation case, and the one
  still live in `crates/kyomi-ui/src/server_fns/mod.rs` today. Routing `into_sfn_sqlx`
  through `into_sfn_core` was right, and the doc comment correctly argues the sqlx `Display`
  detail survives (`Error::Sqlx` is `#[error(transparent)]`). What did not survive was the
  message: *"changed the log line from `\"server function db error\"` to `\"server function
  error\"` for sqlx failures … the distinct string that let ops grep/alert on DB errors
  specifically is gone."* The reviewer's own framing is why this class stays open — *"not a
  regression today, just a minor loss of log filterability."*
- **KYO-231** (`2026-09-02`, `12:56`, 🟡, filed as *Copy-Paste*) — the re-implementation
  case, the WRONG block above. `can_transfer_ownership_from` in
  `crates/kyomi-ui/src/pages/settings/team.rs` re-derived
  `crate::utils::permissions::permissions_from`, and *"also silently drops the
  `tracing::warn!` on fetch failure that `permissions_from` has — a real (if minor)
  observability regression for this specific gate."* Cycle 2 (`13:05`) deleted the
  re-implementation outright and read the shared helper instead.
- **KYO-584** (`2026-09-02`, `00:45`, 🟢, `scripts/link-agent-skills.sh` in
  `kyomi-private`) — the implicit-failure case. The `mkdir -p … && cd … && pwd` resolution
  *"relies on bare `set -e` to fail closed … every other failure path in the script prints a
  descriptive `ERROR:` message first; this one exits silently with whatever raw status
  `mkdir`/`cd` produced."* Fails closed, verified; just mute among announcers.
- **KYO-567** (`2026-08-30`, `16:51`, 🟡, the `Local:` summary block in
  `scripts/mark-branch-stranded.sh`) — the
  wrong-channel case, and the most expensive, because the missing line was the actionable
  one: *"The one piece of information a release operator most needs to act on later … is
  present only in the live stderr stream at the moment of the run, and is dropped from the
  persisted/paste-worthy record."* The reviewer also noted *"It is also untested."* Cycle 3
  (`16:57`) closed it with a catch-all `else` that reprints the warning and both remedies
  *"in the pasteable stdout summary, not just on stderr"*, and a regression test that forces
  the rename failure and *"asserts on the stdout summary text specifically."*

Nearest sibling is
[ok-discards-errors-without-logging.md](ok-discards-errors-without-logging.md): that rule is
about an error thrown away by `.ok()` that never had a diagnostic, and its remedy is a
judgement call — add one. This rule is about a diagnostic that *existed*, in the pre-refactor
code or in the sibling arm three lines down, and stopped existing while the return value
stayed identical; the remedy is a diff against something concrete, not a judgement call.
Distinct from [one-outcome-one-report.md](one-outcome-one-report.md), which is about
*user-facing* channels and how many of them speak for one event; this is the operator-facing
channel and whether the one that should speak still does. Sibling of
[../code-organization/preserve-side-effect-and-error-ordering.md](../code-organization/preserve-side-effect-and-error-ordering.md):
that rule says a "no behaviour change" refactor must preserve which side effects run and in
what order — this one adds what they *say* when they fail. The testing half is
[../testing/cover-the-path-the-criterion-names-not-an-adjacent-one.md](../testing/cover-the-path-the-criterion-names-not-an-adjacent-one.md)
and [../testing/grep-the-crate-before-writing-it-cannot-be-tested.md](../testing/grep-the-crate-before-writing-it-cannot-be-tested.md):
asserting the row is absent is not asserting the failure was announced, and the capture
harness for doing so is already in the tree.
