# A redirection probe in a bare `if` is not `set -e`-exempt — it aborts the script instead

`set -e`'s well-known exemption for `if`/`while`/`&&`/`||` conditions covers a command's
*exit status*. It does not cover a redirection that fails while the shell is still setting the
command up. Under POSIX `sh` (and bash run as `sh`, or in `--posix` mode), a redirection error
on a simple command is treated as a shell-fatal error independent of where that command sits —
including directly inside an `if` condition — and the script exits immediately, before the
`if` body ever runs. Non-POSIX bash is more lenient here and will happily run the `if` body, so
the bug hides on a developer's ordinary `bash script.sh` and appears only under `sh script.sh`,
a strict `#!/bin/sh` shebang, or a POSIX-mode invocation — exactly the environments a portable
installer or CI step is written for.

A script that probes `/dev/tty` (or any other redirection target that may not exist — a FIFO,
a device node, a path under a mount that may be absent) to decide whether it can prompt
interactively is the direct victim: the whole point of the probe is to turn "no controlling
terminal" into a clean, actionable error message. Written as a bare command, it instead turns
that exact condition into an unhandled, unformatted interpreter error, with the guard's own
`fatal()`-style message never printed.

**Rule:** Never place a redirection whose success is in doubt directly on a command inside an
`if`/`while` condition. Push the redirection into a child process — `sh -c '...'` — so a
redirection failure becomes that child's ordinary non-zero exit status, which the `if`
exemption *does* cover. Verify the guard under the same shell the shebang declares (`sh
script.sh`, not just `bash script.sh`), since bash's default leniency will hide the bug in
casual testing.

WRONG — the redirection is set up directly on the guarded command:

```sh
set -e
if ! : < /dev/tty 2>/dev/null; then
    fatal "This installer needs an interactive terminal. Run it without piping stdin."
fi
```

Under `sh` with no controlling terminal, this does not print the `fatal` message. It prints a
raw interpreter error (`sh: 1: cannot open /dev/tty: No such device or address`, wording varies
by shell) and exits — the `if` body is never reached.

RIGHT — the redirection happens inside a child process, so only its exit status crosses back:

```sh
set -e
if ! sh -c ': < /dev/tty' 2>/dev/null || ! sh -c ': > /dev/tty' 2>/dev/null; then
    fatal "This installer needs an interactive terminal. Run it without piping stdin."
fi
```

Flagged in **KYO-641** (`2026-09-04`, `20:40`) on `deploy/install.sh`: `prompt()`'s `/dev/tty`
reads/writes had no guard at all, so a headless run (`curl | sh < /dev/null`, a CI job, a
`docker build RUN`) hit a raw, unformatted interpreter error at the first prompt rather than a
clean message. The `21:15` re-review verified the fix empirically rather than trusting the
diff's own comment: reproduced the bare form aborting the whole script (`exit=1`, guard body
never runs) versus the `sh -c`-wrapped form catching the failure and continuing (`exit=0`,
guard body runs, `fatal` fires cleanly) — confirming the indirection is load-bearing, not
defensive over-caution. The review could not install `dash` in its sandbox to test the literal
target shell directly, and used bash's own `--posix` mode as the closest available proxy for
the documented POSIX semantics — a residual verification gap worth closing with a real `dash`
or `posh` run wherever one is available, but not one that changes the conclusion.

This is a different failure shape from
[a-guard-in-one-branch-does-not-cover-the-others](a-guard-in-one-branch-does-not-cover-the-others.md):
that rule is about a check that exists in one arm and is simply never reached from another. Here
the check is reached and written correctly at the shell-syntax level — the language's own
`set -e` exemption just doesn't extend as far as the author assumed, so the guard fires but
crashes instead of reporting.
