# A validation written inside one branch guards only that branch — and it is usually not the common one

A `case`/`match` that dispatches on the *shape* of an input, then validates the input inside
one arm, reads as a validated function. It is not. Every other arm reaches the body with the
input unchecked, and the arm that gets skipped is disproportionately the one the documentation,
the defaults, and every usage example actually take — because the interesting-looking arm is
the one that needed extra work, so that is where the author was concentrating when the check
occurred to them.

The result fails **open**, and does so on the happy path. A script whose header promises it
"fails closed: anything it cannot verify is an error, never a silent success" can exit `0`
having verified nothing at all, and the test suite will agree with it, because tests are
written against the arm the author was thinking about too.

**Rule:** When a check exists to establish a precondition for the whole body, hoist it out of
the branch — validate before dispatching, or after the branches converge. If it genuinely must
live inside a branch, then every sibling arm needs the check too, including the catch-all;
enumerate the arms and say what establishes the precondition in each. A `*) : ;;` /
`_ => {}` arm is the shape to look at first: it is where "nothing to normalise here" silently
becomes "nothing to check here". Then add a test that drives the *skipped* arm — a suite in
which every case happens to enter the validating branch proves nothing about the guard.

WRONG — existence is only checked when the path needed normalising:

```sh
case "$KYOMI_REPO_PATH" in
  /*) : ;;                                          # absolute: nothing to normalise…
                                                    # …and so nothing is checked
  *)  cd "$KYOMI_REPO_PATH" || exit 1               # relative: cd doubles as the check
      KYOMI_REPO_PATH="$PWD" ;;
esac
```

`--kyomi-repo /tmp/does-not-exist` — an absolute path, which is both the documented default
(`$HOME/repos/kyomi`) and the form every example in the script's own header uses — takes the
no-op arm, is never checked, and the script exits `0` after creating symlinks under a brand-new
directory tree that should have been an error.

RIGHT — the precondition is established once, for every arm:

```sh
case "$KYOMI_REPO_PATH" in
  /*) : ;;
  *)  KYOMI_REPO_PATH="$PWD/$KYOMI_REPO_PATH" ;;
esac

[ -d "$KYOMI_REPO_PATH" ] || {
    echo "ERROR: --kyomi-repo does not exist: $KYOMI_REPO_PATH" >&2
    exit 1
}
```

Flagged in KYO-584 (2026-09-02) on `scripts/link-agent-skills.sh:154-160` in `kyomi-private`,
and reproduced live rather than argued from reading. All ten tests in the accompanying suite
`mkdir -p`'d the repo directory before invoking, so the gap was untested as well as unhandled —
the coverage shape
[cover-the-path-the-criterion-names-not-an-adjacent-one](../testing/cover-the-path-the-criterion-names-not-an-adjacent-one.md)
warns about, arriving via an unexercised branch rather than an adjacent one.

This is the branch-level form of the fail-closed discipline KYO-511 established for whole
commands (see
[empty-on-failure-must-not-look-like-a-real-result](empty-on-failure-must-not-look-like-a-real-result.md)):
there, a check that could not be completed had to exit non-zero rather than look clear; here, a
check that was never *reached* must not be mistaken for one that passed.
