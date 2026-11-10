# Tokio worker starvation reproduction

A standalone axum HTTP server and an observer outside the Tokio runtime. It needs
no application build, database, model download, or production configuration.
The listener uses an ephemeral loopback port and is stopped after each scenario.

From the repository root, build through the development build queue:

```sh
scripts/dev-build cargo check --locked --manifest-path scripts/tokio-starvation/Cargo.toml
scripts/dev-build cargo clippy --locked --all-targets --manifest-path scripts/tokio-starvation/Cargo.toml -- -D warnings
scripts/dev-build cargo build --locked --manifest-path scripts/tokio-starvation/Cargo.toml
scripts/tokio-starvation/target/debug/tokio-starvation-repro
scripts/tokio-starvation/target/debug/tokio-starvation-repro 30 busy
scripts/tokio-starvation/target/debug/tokio-starvation-repro 30 sleep 8 timer
```

If this checkout predates the local `scripts/dev-build` runner, invoke the runner
from the canonical checkout with these same Cargo arguments. Run the executable
directly after building so measurements do not hold the compilation queue.

Arguments are `[seconds=30] [sleep|busy] [workers=1,2,4,8] [immediate|timer]`. Each worker count runs
with zero, one, N−1 and N blockers (duplicates removed). Each blocker is an
ordinary spawned async task that deliberately never yields while sleeping or
spinning for the requested duration. With `timer`, each task first awaits a
100ms Tokio timer so its blocking section is launched by a driver wakeup after
workers have an opportunity to park. The default `immediate` launches directly. Sleeping occupies workers without consuming
CPU; spinning also consumes CPU. A full matrix takes about 4.5 minutes at the
30-second default. Use a shorter duration for a smoke run, or e.g. `30 busy 8`
for a focused run.

The observer first checks a fresh HTTP connection, launches blockers, waits for
all of them to enter their blocking section, then measures another fresh HTTP
connection. It joins the blockers and checks recovery. CSV columns are worker
count, blocker count, mode, launch style, requested seconds, baseline latency, time elapsed
since the first blocker entered when the observer launched the probe, latency
during blocking, and recovery latency. All latency columns use milliseconds.
Large entry spread means the blockers may not overlap for the requested period.
Startup stderr records `available_parallelism`, the actual worker count of an
unconfigured runtime, and `TOKIO_WORKER_THREADS`.

A non-200 response, network error, readiness timeout or task panic fails the run;
independent scenarios continue after returned errors. Latency is deliberately
reported without timing assertions: this is a measurement tool, not a scheduling
contract or a deterministic CI gate. A responsive minimal server cannot prove
that a historical application outage had a particular cause. Record run output,
versions and environment alongside any investigation conclusions.

This separate Cargo workspace pins the application's Tokio and axum versions at
introduction. Its lockfile is independent of the production lockfile; update both
pins explicitly when comparing another runtime release.
