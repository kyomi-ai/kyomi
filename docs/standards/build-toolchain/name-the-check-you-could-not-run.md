# Name the check you could not run, and make no claim in its place

A verification report is read as a statement about coverage, not as a list of commands that
happened to succeed. So a check that never ran is indistinguishable, to the reader, from one
that passed — the tool is not installed on this box, the MCP server is unauthenticated in
this session, the credential for the live provider does not exist here. Nothing in the
report says so, and the absence of a line about `shellcheck` reads exactly like the absence
of a `shellcheck` failure.

The second failure direction is subtler and more common in this repo, because agents are
good at reasoning: the check is replaced by a substitute — an emulation of the other
shell's regex engine, a hand-trace of the algorithm, an inference from the tool's
documentation — and the substitute is written up in the same voice as an execution. The
reader then cannot tell which sentences are observations and which are arguments, which is
the one distinction the report exists to preserve. The same thing happens in permanent
comments: an incident's *correlation* gets written down as its *mechanism*, and a
plausible causal story ships as established fact.

**Rule:** when a verification step cannot run in your environment, say so explicitly, in the
report and in any comment that would otherwise imply it ran: name the check, name why it did
not happen, and state that you make no claim about it. If you substitute a proxy — an
emulation, a hand-trace, a different shell, a partial fixture — label it as a proxy at the
point you present it and say what it does and does not establish. Repeat the disclosure in
every re-review cycle; a cycle that quietly drops the line reads as though the gap closed.
Where the missing evidence is a *mechanism* you could not isolate, write down the
observation you do have and the candidate explanations you did not rule out, rather than
picking one and asserting it.

```
WRONG — the reader has no way to tell these two lines apart:

  - `bash -n` clean on both new files.
  - The PowerShell strip leaves `--cfg=has_std` intact on CRLF input.

(the first was executed; the second was emulated in Python because `pwsh`
is not installed, and nothing in the report says which is which)

RIGHT — the substitute is labelled, and the gap is stated as a gap:

  - `bash -n` clean on both new files. GNU `sed` strip re-run against the real
    file, LF and CRLF: `--cfg=has_std` retained, block removed as one unit.
  - The chained pwsh `-replace` was assessed by a Python emulation of the same
    regex semantics, not by execution — `pwsh` is NOT installed on this box.
    Establishes the regex is well-formed and cannot over-match; does not
    establish PowerShell's own behaviour.

  **Not verified — stated plainly:** `shellcheck` is not installed on this box,
  so the shell lint pass did not happen and I make no claim about it.
```

Real precedent — four tickets across three review-log days, all of them the disclosed
(passing) form, which is why the practice is worth pinning before it erodes:

- **KYO-703**, review log `2026-09-09`, heading *"KYO-703: remove the PR-listing ceiling
  from check-ticket-in-flight.sh"* — the disclosure survives all three cycles verbatim
  rather than being dropped once it has been said:
  > **Not verified — stated plainly:** `shellcheck` is **not installed on this box**, so the
  > shell lint pass did not happen and I make no claim about it. `bash -n` was clean on
  > every mutated copy.

  Cycle 2 repeats it as "unchanged from cycle 1"; cycle 3 as "unchanged across all three
  cycles… No shell lint pass has happened at any point."

- **KYO-677**, review log `2026-09-09`, heading *"KYO-677 enable `--cfg=leptos_debuginfo`
  for dev wasm builds only"* — the labelled-proxy case: the PowerShell strip was checked by
  "a Python emulation of the chained pwsh `-replace` (pwsh is NOT installed on this box —
  regex assessed by emulation + reasoning …, not by execution)", and cycle 2 restates it in
  a parenthetical rather than letting the stronger cycle-2 result ("byte-identical output")
  imply an execution: "(`pwsh` is still not installed on this box — emulation, not
  execution.)"

- **KYO-632**, review log `2026-09-04`, heading *"KYO-632 stale-tooling-guard (PR #486,
  post-rebase re-review, cycle 2)"* — states the rule in one clause, and adds the fact that
  makes the gap tolerable rather than leaving the reader to guess at it:
  > shellcheck not installed on this machine — could not run it; noting as unverified rather
  > than assuming clean. No shellcheck step exists in CI either.

- **KYO-607**, review log `2026-09-03`, heading *"KYO-607: recycled pre-restart ticket keys
  in check-ticket-in-flight.sh"* (cycle 3) — the same discipline for a data source rather
  than a binary, and it names where the evidence *did* come from instead:
  > Could not re-verify KYO-299's Trakkt `created_at` this cycle — every Trakkt MCP server is
  > unauthenticated in this session; cycle 2 verified it at source … and it is a README-only
  > claim with no bearing on the executable behaviour.

The reviewer of the KYO-644 pre-work standard (`2026-09-05`, heading *"Standards-mining
commit: redirection-probe-in-bare-if rule (KYO-644 pre-work)"*) went looking for a
`dash`/`posh` binary specifically to close a gap the rule file had disclosed about itself,
found neither installed, and concluded that "the residual gap the file discloses is real and
its caveat is stated honestly, not glossed over" — closing with "a good pattern to hold other
mined standards to." This rule is that.

The one MAJOR finding in this window shows the cost of the opposite habit, and where the
boundary with the sibling rule sits. In **KYO-644** (`2026-09-05`, heading *"KYO-644: chunk
+ offload catalog embedding batches off the async runtime"*) three doc comments asserted as
established fact *why* one occupied tokio worker made the whole HTTP server unresponsive,
when "the mechanism connecting one occupied thread to *total* server unresponsiveness was
never conclusively isolated — two candidate explanations were left open." The remedy that
shipped was not a reproduction, because none was available: the comments were rewritten to
state the correlation, name both candidate mechanisms, and add "The exact mechanism linking
one occupied thread to whole-pool unresponsiveness was not conclusively isolated."

That is the discriminator against
[a-tool-claim-needs-a-reproduction-not-a-citation.md](a-tool-claim-needs-a-reproduction-not-a-citation.md):
that rule's remedy is to go and run it, and it applies whenever running it is possible —
which, for the questions it covers, it almost always is. This rule governs what you owe the
reader when running it is *not* possible in this environment, and its remedy is disclosure
rather than execution. Distinct from
[a-cargo-run-that-compiled-nothing-verified-nothing.md](a-cargo-run-that-compiled-nothing-verified-nothing.md)
and
[verify-lint-fixes-on-the-toolchain-that-produces-them.md](verify-lint-fixes-on-the-toolchain-that-produces-them.md):
in both of those the command ran and returned green while being incapable of failing — a
vacuous observation; here no command ran at all, and the defect is reporting as though one
had. Sibling of
[../version-control-working-tree/state-the-acceptance-criterion-you-did-not-meet.md](../version-control-working-tree/state-the-acceptance-criterion-you-did-not-meet.md),
which requires the same disclosure one level up — that rule is about a ticket criterion
nobody implemented and wants a ticket ID; this one is about a check nobody could execute and
wants the environment fact that prevented it.
