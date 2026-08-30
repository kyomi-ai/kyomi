# Verify a WebSocket event's payload key — and its nesting depth — against the emitter that writes it

A `WebSocketMessage`'s `data` is an untyped `serde_json::Value`. The server builds it with a
`json!` literal in `kyomi-auth::websocket::helpers`; the client picks it apart with
`.get("…")` inside `setup_ws_subscriptions` in `kyomi-ui`. There is no shared struct, no
schema, and no test on either side that can fail when the two spellings disagree. The
producer writes the key it was written to write, the consumer reads the key it was written to
read, and if those are not the same string the consumer silently takes its `unwrap_or`
branch — forever, on the default path, with the feature looking like it works.

Two things have to agree, not one:

- **The key string.** `send_error` writes the human-readable reason under `"error"`. A
  handler reading `data.get("message")` compiles, runs, and renders the generic
  `"An error occurred"` fallback for every server-emitted error the product has ever sent.
- **The depth it sits at.** Depth is not uniform across events in this same payload family:
  `error` carries `context_type` at the *top level* (`data.context_type`), while
  `agent_thinking` nests it at `data.event.context_type`. A path copied from the neighbouring
  handler — the obvious thing to do, since the surrounding filter code is genuinely shared —
  is wrong by construction, and its only symptom is a filter that quietly stops filtering.

The reason this survives review is that the handler cannot be unit-tested where it lives.
`setup_ws_subscriptions` is `#[cfg(target_arch = "wasm32")]`, so it never compiles into the
host test binary: reverting the fix inside it leaves the entire suite green (see
[a mutation only counts if the run could have failed](../testing/a-mutation-only-counts-if-the-run-could-have-failed.md)).
A green suite is evidence about the harness here, not about the payload contract.

**Rule:** For every `msg.data.get("…")` you add or change in a WS handler, open the emitter
in `crates/kyomi-auth/src/websocket/helpers.rs`, find the `json!` literal that builds *that
event's* `data`, and confirm both the exact key string and the depth — do not infer either
from a sibling handler in the same file. Extract the read into a named function taking
`Option<&serde_json::Value>` so it compiles into the host test binary, and pin it with a test
built from the emitter's real payload shape rather than a convenient one. Never repair a
mismatch by reading both keys: a `.or_else()` dual-key read makes a producer/consumer
disagreement work by accident and removes the only signal that the contract is broken. Pin
the single agreed key with a test that fails if the abandoned key is honoured, and say in the
extracted function's doc comment which emitter it is agreeing with.

```rust
// WRONG — key inferred from the local `ChatError` shape, and the context_type
// path copied from the agent_thinking handler two closures up. Both compile.
let error_msg = msg
    .data
    .as_ref()
    .and_then(|d| d.get("message"))          // send_error writes "error"
    .and_then(|v| v.as_str())
    .unwrap_or("An error occurred")
    .to_string();
let event_context_type = msg
    .data
    .as_ref()
    .and_then(|d| d.get("event"))            // error events are not nested
    .and_then(|e| e.get("context_type"));
```

```rust
// RIGHT — one named function per read, each agreeing with a `json!` literal
// that was opened and read, each host-testable, neither growing a second key.

/// KYO-550: `send_error` (`kyomi-auth::websocket::helpers`) writes the reason
/// under the key `"error"`. This reads that same key — and only that key. It
/// deliberately does NOT also check `"message"`: a dual-key fallback would let
/// a producer/consumer key mismatch keep working by accident.
#[cfg(any(test, target_arch = "wasm32"))]
fn error_event_message(data: Option<&serde_json::Value>) -> String {
    data.and_then(|d| d.get("error"))
        .and_then(|v| v.as_str())
        .unwrap_or("An error occurred")
        .to_string()
}

/// KYO-501: `error` events carry `context_type` at the *top level*
/// (`data.context_type`), unlike `agent_thinking`'s, which nests it at
/// `data.event.context_type`.
#[cfg(any(test, target_arch = "wasm32"))]
fn error_event_context_type(data: Option<&serde_json::Value>) -> Option<&str> {
    data.and_then(|d| d.get("context_type")).and_then(|v| v.as_str())
}
```

Both halves of one payload, two tickets, three reviews — cited by symbol and log entry
because most were logged against in-flight branch state (see
[anchor a citation to a symbol, not a line number](../comments-documentation/anchor-a-citation-to-a-symbol-not-a-line-number.md)):

- **KYO-501** (review log `2026-08-29`, `08:33`) — the depth half, plus the discovery of the
  key half. The fix pulled `error_event_context_type` out of the subscription closure
  specifically so the top-level-vs-nested read became host-testable, and the reviewer
  verified it by opening `send_error` rather than trusting the doc comment: *"`send_error()`
  sets `data["context_type"]` at the top level (not nested) — matches
  `error_event_context_type`'s read exactly."* The same review recorded, as an out-of-scope
  discovery, that the neighbouring line read the wrong key entirely: *"the `error` handler in
  `chat_engine.rs` reads `msg.data.get("message")` for the display string, but `send_error()`
  … puts the text under the `"error"` key, not `"message"`. As written, every server-emitted
  chat error likely displays the generic `"An error occurred"` fallback instead of the real
  message."* That review also proved the handler's untestability empirically — reverting the
  fix left all 508 tests green, disclosed and ticketed as KYO-549 rather than papered over.
- **KYO-550** (review log `2026-08-30`, `02:55` initial and `21:15` rework; shipped
  `e566f252`) — the key half, fixed, with the no-dual-key discipline pinned by a test rather
  than left to a comment. The reviewer's mutation is the rule in one line: *"reintroducing
  `.get("error").or_else(|| d.get("message"))` causes
  `error_event_message_does_not_fall_back_to_the_message_key` to fail exactly as expected
  (`left: "should not be read", right: "An error occurred"`)."* The emit site was re-read
  from source in both the initial review and the rework, not carried over.
- **KYO-559**, filed out of the `02:55` review, is the same family one step further: a
  `chat_page.rs` handler that ignores its payload entirely and carries no session filter.

Sibling of
[verify-config-keys-against-the-driver-that-reads-them.md](../data-state-management/verify-config-keys-against-the-driver-that-reads-them.md):
that rule covers the `connection_config`/`credentials` maps, whose reader lives in another
repo (`~/repos/kyomi-connect`) and whose failure surfaces as a provider error at connect
time. This one covers an event payload emitted and consumed inside this workspace, where the
mismatch never errors at all — it substitutes a plausible default — and where the nesting
depth is itself part of the contract and varies between events in the same family. Distinct
from [empty-on-failure-must-not-look-like-a-real-result.md](../error-handling/empty-on-failure-must-not-look-like-a-real-result.md),
which is about a value degraded by a *failure*: here nothing failed, the payload arrived
complete and correct, and the reader looked in the wrong place. See also
[propagate-predicate-changes-to-every-copy.md](../code-organization/propagate-predicate-changes-to-every-copy.md)
for the adjacent defect in the same handlers — the `error` subscription being the one left on
an inline check when its three siblings moved to the shared default-deny filter.
