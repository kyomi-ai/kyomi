# Every identifier in a documentation code example must resolve in the codebase

A standards rule's WRONG/RIGHT blocks are the part agents copy. Prose is read and paraphrased; a code block is pasted. So a fabricated type, signal, or method inside an example is not a cosmetic slip — it is an instruction to write code that does not compile, shipped under the authority of a document that exists to be followed.

Nothing checks these blocks. They are markdown: not compiled, not linted, not covered by `cargo check`. The only thing standing between a plausible-looking `DiscoveryStatus::Idle` and an agent writing it verbatim is whether the author grepped for it.

The failure is systematically likelier in a standards doc than in a normal doc comment, because the author is writing *from a review finding or a memory of the code* rather than from the file open in front of them — and an invented enum variant reads more idiomatic than the real `String` signal it replaced.

**Rule:** Before committing a documentation file that contains a code example, grep the codebase for every identifier the example names — types, enum variants, signals, functions, fields — and confirm each one resolves. Quote the real thing even when it is uglier than the abstraction you would have designed. Do the same for prose citations of field and signal names. A review finding on one claim tells you nothing about the others; check all of them (see [re-derive-enumeration-comment-from-source.md](re-derive-enumeration-comment-from-source.md)).

```rust
// WRONG — DiscoveryStatus does not exist anywhere in the workspace.
// `grep -rn "enum DiscoveryStatus\|DiscoveryStatus::" crates/` → zero hits.
set_discovery_status.try_set(DiscoveryStatus::Idle);

// RIGHT — the real signal holds a String, so the real call is a string.
// Verified against crates/kyomi-ui/src/pages/settings/datasources.rs:1314.
set_discovery_status.try_set("idle".to_string());
```

Mechanical check before staging any docs change carrying a code block:

```sh
# For each identifier the example names:
grep -rn "DiscoveryStatus" crates/ || echo "FABRICATED — do not ship"
```

Flagged three times in three consecutive review cycles on one 68-line file, `docs/standards/data-state-management/teardown-clears-the-whole-derived-state-group.md` (KYO-433, 2026-08-23). Cycle 1 caught two misquoted prose citations — a signal named `connection_test_result` that is really `test_result`, and a site count of "four" that is really five. Cycle 2, after both prose fixes landed, found the fabricated `DiscoveryStatus::Idle` in *both* the WRONG and RIGHT blocks — untouched by cycle 1 because cycle 1's findings were about prose. The reviewer's own note: *"verify every identifier in an illustrative code block actually resolves in the codebase, not just the prose citations."* Three cycles on a documentation file that contained no code.
