# A name-filtered test run that matches nothing proves nothing — confirm the count

`cargo test` and `cargo test -- <FILTER>` share a failure mode that is invisible
in the exit code: libtest exits `0` when a name filter matches **zero** tests.
A mistyped module path, a renamed test function, or a filter scoped one
namespace too deep all produce a green run that executed nothing. The output
carries no warning — it prints `running 0 tests` and `test result: ok. 0
passed; 0 failed; 0 ignored`, which a reader skimming for `test result: ok` will
accept the same way they accept a real 40-test green.

This is a different mechanism from
[a-cargo-run-that-compiled-nothing-verified-nothing.md](../build-toolchain/a-cargo-run-that-compiled-nothing-verified-nothing.md),
which is about a fingerprint hit replaying an earlier verdict for code that was
never compiled. Here the binary really did run; the selection just found no
targets. Both end in a clean line that means nothing, and neither is caught by
the exit code.

It is particularly sharp in standards documents and review evidence, where the
exact filter string is copied from prose rather than from a terminal — a filter
that *looked* right in the doc block and matched nothing on disk.

**Rule:** When you cite a name-filtered test run as verification, confirm the
run reports a non-zero test count and name the tests you expected to execute.
Do not treat `test result: ok` from a filtered run as evidence unless you have
seen `running N tests` with `N > 0`. If the count is zero, the filter is wrong
— fix the filter, do not record the run.

```sh
# WRONG — exit 0, "test result: ok", ran nothing
# The module is `tests_disposal_safety`; dropping the `tests_` prefix is a
# natural abbreviation slip. libtest's TESTNAME is a substring match, so a
# filter that merely *looks* like a typo can also be a live match — see below.
$ cargo test -p kyomi-ui --locked --features ssr -- chat_engine::disposal_safety
    Running unittests src/lib.rs
    running 0 tests
test result: ok. 0 passed; 0 failed; 0 ignored
# recorded as "disposal tests pass" — they never ran

# RIGHT — check the count before citing
$ cargo test -p kyomi-ui --locked --features ssr -- tests_disposal_safety
    running 12 tests
test result: ok. 12 passed; 0 failed; 0 ignored
# 12 > 0 — the named module executed. Cite "12 passed" alongside ok.
```

The same discipline applies to a filter that is expected to skip without a
database: a `postgres_` prefix run on a box with no `DATABASE_URL` reports a
small positive count of tests that each took the skip path, which is
[skipped-test-must-fail-loudly.md](skipped-test-must-fail-loudly.md)'s problem
rather than this one — but a zero count means neither problem happened, because
nothing was selected at all.

Real precedent — KYO-432 cycle 4 (review log `2026-10-16`, `06:06`, finding 3,
🟢): the RIGHT block's newly added `cargo test` invocations were name-filtered,
and the reviewer noted *"libtest exits 0 on zero matches, so a reader who copies
the shape with a wrong module path gets a green RIGHT run that executed nothing
— the same 'green that proves nothing' class the standard exists to name."*

The exemplars' counts are the demonstration, not the citation. That review log
recorded `chat_engine::tests_disposal_safety` → 6 tests; a brace-matched count
of `#[test]` inside the module (`crates/kyomi-ui/src/components/chat/chat_engine.rs:1541`,
which also holds one non-test helper fn) is **12**. The log was quoted without
recounting the artefact — which is
[../comments-documentation/quote-the-artefact-not-the-log-that-quotes-it.md](../comments-documentation/quote-the-artefact-not-the-log-that-quotes-it.md)
applied to a review log. Counts in this document are recomputed from source.
`watch_service::duplicate_names_tests` → 2 holds (out-of-line module at
`crates/kyomi-auth/src/watch_service/duplicate_names_tests.rs`, two
`#[tokio::test]`s, declared at `watch_service.rs:4278`).

Substring matching cuts the other way too. The filter this block first used —
`chat_engine::tests_disposal` — *looks* like a truncated path and is instead a
**live** substring of every test in the module, so it would run all 12 and print
a green that means something different from what the reader intended. Confirming
`running N tests` with `N > 0` catches both directions: a filter that selects
nothing, and a filter that silently selects more than you thought.

Sibling of
[../build-toolchain/a-cargo-run-that-compiled-nothing-verified-nothing.md](../build-toolchain/a-cargo-run-that-compiled-nothing-verified-nothing.md):
that rule asks *did cargo compile the code* (answer: the elapsed time); this one
asks *did the filter select any tests* (answer: the running count). A run can
pass one and fail the other.
