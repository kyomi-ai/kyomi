# Every `openssl pkeyutl` call on an Ed25519 key needs `-rawin`, proven on the oldest OpenSSL it runs on

This repo's review and merge gates sign and verify with Ed25519 keys through
`openssl pkeyutl`. That call behaves differently depending on the OpenSSL version. On
OpenSSL 3.2 and later a flagless `pkeyutl -sign` / `-verify` works on an Ed25519 key. On
OpenSSL 3.0.x it fails outright with
`evp_pkey_signature_init: operation not supported for this keytype`. 3.0.x is what Ubuntu
24.04 ships (3.0.13), and Ubuntu 24.04 is `ubuntu-latest`, the CI runner. With `-rawin` the
call works on both. Signatures are byte-identical with and without the flag, so adding it
changes no signed bytes, invalidates no existing approval, and needs no coordinated switch.

The flagless form is the natural one to write. It passes on a current developer box (3.5.x
signs fine either way) and then fails on the older OpenSSL that CI, and any deploy target on
a long-term-support distribution, actually runs. The flagless call has turned up at three
separate call sites so far, and each was fixed on its own.

The mechanical point is the flag. The more general point is **where** the proof runs. When
a call's behaviour depends on the version of the tool, the version on your machine is
evidence only for that version. Nearest siblings:

- [a-tool-claim-needs-a-reproduction-not-a-citation.md](a-tool-claim-needs-a-reproduction-not-a-citation.md)
  says to reproduce a tool's behaviour rather than cite its documentation. This rule adds
  *which version* to reproduce it on. A faithful reproduction on OpenSSL 3.5.5 is still
  silent about 3.0.13.
- [verify-lint-fixes-on-the-toolchain-that-produces-them.md](verify-lint-fixes-on-the-toolchain-that-produces-them.md)
  is the mirror image. There, the local tool is *older* than CI's and cannot produce the
  failure. Here, the local tool is *newer* and no longer produces it.

**Rule:** Every `openssl pkeyutl -sign` or `-verify` on an Ed25519 key passes `-rawin`: in
scripts, git hooks, test suites, and fixture generators. Signer and verifier must match.
Copy the explanatory comment that sits next to the call in `scripts/sign-review.sh`, too. It
records the measured version matrix and says "do not remove it as \"redundant\"", which is
what stops a later cleanup from dropping the flag on a box where the flagless form happens
to work.

More generally, prove any version-sensitive crypto or tooling call on the *oldest* version
it will run on, not on your own machine. For OpenSSL that means a throwaway `ubuntu:24.04`
container. Mutation-test the proof there: remove the flag, confirm the suite fails *in that
container*, then restore it.

The lasting guard is a behavioural self-test registered in CI's `ubuntu-latest` job, not a
grep for the string `-rawin` (grep-based lints are ruled out in this repo). A script with no
such suite is unguarded, however many of its siblings are covered. So when you add a signer
or verifier, give it a suite in that job. Also read every other `pkeyutl` call site by hand,
including ones outside this repo that verify what this repo signs.

```sh
# WRONG: scripts/sign-verification.sh:33 on main at the time of writing, verbatim.
# It passes on OpenSSL 3.5.x, and cannot produce a signature at all on the CI runner.
SIGNATURE=$(openssl pkeyutl -sign -inkey "$KEY_FILE" -in "$SHA_FILE" | base64 -w 0)

# RIGHT: scripts/sign-review.sh:334 on main (abe205f8, PR #516); see the comment above it.
SIGNATURE=$(openssl pkeyutl -sign -rawin -inkey "$KEY_FILE" -in "$HASH_FILE" | base64 -w 0)

# Prove it where it can fail, not where it cannot:
podman run --rm -v "$PWD:/w" -w /w ubuntu:24.04 bash -c \
  'apt-get -qq update && apt-get -qq install -y openssl git >/dev/null && openssl version && bash scripts/<suite>-test.sh'

# Find every call site to read by hand. This is a one-off manual check, not a lint:
grep -rn 'pkeyutl' scripts/ .githooks/ .github/ ~/.local/bin/gh
```

Real precedent: the same defect at three call sites.

- **KYO-712** (landed on `main` in `abe205f8`, PR #516). `scripts/sign-review.sh` signed
  with the flagless call. **The CI self-test caught it**, not a local run. The PR's first
  CI run ([actions run 34575216271](https://github.com/kyomi-ai/kyomi/actions/runs/34575216271),
  2026-09-11, head `7edb861c`) failed the `Worktree Lifecycle & Script Self-Tests` job
  (`worktree-lifecycle-selftests` in `.github/workflows/ci.yml`, `runs-on: ubuntu-latest`,
  step "Run worktree lifecycle self-test suites"). `scripts/sign-review-test.sh` failed with
  `✗ first sign (fully staged) succeeds — expected exit 0, got 1` followed by the
  `evp_pkey_signature_init` error. The job summary read "2 of 12 worktree lifecycle
  self-test suite(s) failed: scripts/sign-review-test.sh scripts/pre-commit-hook-test.sh".
  The review-log entry `KYO-712 rework: ci.yml rebase resolution + -rawin for OpenSSL
  3.0.x` (`2026-09-12`) records the rework that followed. The reviewer "built an
  `ubuntu:24.04` (OpenSSL 3.0.13, matches `ubuntu-latest`) container via `podman`,
  confirmed flagless `pkeyutl -sign` fails with exactly the quoted error ... and `-rawin`
  succeeds." On `main` today, both suites are in that step's `suites=(...)` array and guard
  the review gate from both sides:
  - `scripts/sign-review-test.sh` checks that the `-rawin` signature verifies
    (`pkeyutl -verify -rawin`).
  - `scripts/pre-commit-hook-test.sh` runs the real `.githooks/pre-commit` (line 120,
    `pkeyutl -verify -rawin`) against the real `scripts/sign-review.sh`.
- **KYO-747** is the review-log entry `KYO-747: sign-verification.sh -rawin fix (Ed25519
  pkeyutl on OpenSSL 3.0.x)` (`2026-09-23`). `scripts/sign-verification.sh`, the merge-gate
  signer, has the same flagless call KYO-712 fixed in the review gate. It was not caught
  the same way because, on `main`, no suite in the CI job exercises it: `git grep
  sign-verification origin/main -- scripts .github .githooks` finds only the script itself.
  That is the "a script with no suite is unguarded" clause above, in practice. At the time
  of writing, KYO-747's fix had not landed on `main`. That entry records a mutation test in
  the same 3.0.13 container: with `-rawin` removed, "12 passed, 6 failed". Its Notes ask
  for this rule: "Third occurrence of the exact same `pkeyutl -sign`-without-`-rawin`
  defect ... Worth a standards-doc entry ... so the next agent adding a signer doesn't
  reintroduce it a fourth time."
- **The third site** is the verifier in the `~/.local/bin/gh` wrapper, which is outside
  this repo. Its `pkeyutl -verify` call (line 110) still has no `-rawin` at the time of
  writing. The KYO-747 entry records that it was left out of that ticket's scope as a
  documented manual step. It is also why the sweep has to reach past `scripts/`: the call
  that verifies a signature does not have to live in the repo that produces it, and no
  suite in this repo can guard it.
