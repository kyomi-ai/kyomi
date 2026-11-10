# Outbound MCP client

`kyomi-mcp-client` is independent of the durable `agent-runtime` kernel and of
product identity, grants and credential storage. Consumers supply one connection's
explicit remote authorization and enforce owner/grant/privacy policy themselves.
Use `SafeHttpClient` for **all** remote HTTP, including OAuth metadata, registration,
token/revocation and resource downloads. Never create a separate unrestricted client.
`HttpRequest` accepts form content types for OAuth and exposes bounded authentication
challenge and retry-after response headers. It does not accept incoming cookies,
proxy settings, arbitrary headers or cross-authority redirects.

Default egress permits HTTPS public unicast only. Each request validates every DNS
answer, pins the connector to those answers and verifies the actual peer address.
`DeploymentPolicy::allow_private_https_origins` is for trusted administrator startup
configuration only: do not expose it through connection/user DTOs. It allows an exact
HTTPS origin, retaining certificate checks. HTTP/loopback fixture access exists only
in this crate's unit-test compilation; integration tests inject `HttpTransport`.

Supported revisions are explicit:

- `2026-07-28`: per-request metadata, standard and schema parameter headers,
  stateless `server/discover`, no initialize/session/DELETE/cancel notification.
  Dropping the request future closes the HTTP response stream to cancel.
- `2025-03-26`: initialize, empty client capabilities, initialized notification,
  optional session header, invocation-bound cancellation notification and session DELETE.
  Expired sessions fail; they are never recreated while replaying an interrupted call.

No stdio, old HTTP+SSE transport, server requests, sampling, elicitation, roots, tasks,
prompt import, subscriptions, SSE reconnection or automatic mutation retry is enabled.
Tools must first be listed to obtain the validated header schema. The caller still
must check exact revision-bound grants before invoking them. Invalid modern header
annotations are excluded. Remote results and JSON-RPC errors are typed outcomes;
remote text is untrusted and should not be logged as credentials or instructions.

`call_tool_cancellable` accepts a one-shot cancellation receiver tied to exactly one
live invocation. It sends the matching legacy cancellation notification or drops the
modern response stream; callers cannot cancel initialization or arbitrary request IDs.
All logical operations share one total deadline, including lock waits, control POSTs,
catalog pagination, retries and backoff.

Every `Error` reports `DispatchCertainty`. Only `NotSent` proves the action was not
submitted. `MayHaveBeenSent` and `ResponseReceived` errors both require unknown-write
handling if no complete result was persisted. A received HTTP status/body limit is
not proof that a mutation failed. `tools/call` has exactly one submission; only bounded
catalog reads retry configured transient statuses/transport errors. No Kyomi/native
tool shares the per-client concurrency semaphore.

## Compatibility decision (2026-10-17)

Reviewed official Rust SDK `rmcp` **3.5.1**, especially features `client`,
`transport-streamable-http-client` and `transport-streamable-http-client-reqwest`.
It supports both eras, but its worker owns session recovery/ordinary POST retries,
SSE resumption and body parsing. A custom HTTP implementation alone does not expose
our required pre-send certainty, total body/catalog budget and unsupported-capability
boundary at the client port. This release uses a small explicit JSON-RPC compatibility
adapter over the existing locked `reqwest` **0.12.28**, with `retry::never()`, redirect
and proxy handling disabled. No `rmcp` features/dependency or subprocess code is linked.
This is a bounded feature subset, not a claim of full general-purpose SDK conformance.

Primary references checked for the two separate semantics:

- [Modern HTTP](https://modelcontextprotocol.io/specification/2026-07-28/basic/transports/streamable-http)
- [Modern versioning](https://modelcontextprotocol.io/specification/2026-07-28/basic/versioning)
- [Legacy HTTP](https://modelcontextprotocol.io/specification/2025-03-26/basic/transports)
- [Official Rust SDK](https://github.com/modelcontextprotocol/rust-sdk)
- [SDK transport source](https://github.com/modelcontextprotocol/rust-sdk/blob/main/crates/rmcp/src/transport/streamable_http_client.rs)

The sibling Trakkt server's `apps/server/src/routes/mcp.rs` independently confirms
its pinned `2025-03-26` initialize/result, session and 202 notification shapes.
`conformance.rs` encodes those shapes without calling a real account. CI executes
`cargo test --locked --no-fail-fast -p kyomi-mcp-client`, including isolated actual
HTTP socket fixtures for the production egress path. There are no browser/UI changes.
