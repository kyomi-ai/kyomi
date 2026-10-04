# agent-runtime

An application-independent durable conversation foundation. Kyomi is one adapter;
this crate has no Kyomi, SQLx, HTTP, Redis, provider or product-tool dependency.
No publication or license change is part of this addition.

The foundation consists of typed identities, version 1 commands/events, a pure
transition planner, an atomic persistence port, post-commit notifications and
public replay DTOs. Responses are complete records; there are no token deltas.
Provider ingestion, worker claims/leases, tool execution, client reducers and
interface delivery belong to subsequent implementation stages.

`AppendCommand` names a conversation, run, event and idempotency key. Identity
newtypes wrap adapter-defined strings (1–128 bytes), allowing existing applications
to keep stable IDs. `Payload` separates `PublicPayload` from `RestrictedPayload`.
Public replay uses `PublicEvent` and cannot carry a restricted payload. Public
kinds cover submission, run state, complete model response, exposed planning,
tool intent/start/result, usage, validation, approved answer, cancellation request,
cancellation and interruption. Restricted kinds preserve complete provider data
and opaque continuation values, or candidate answers, without reclassifying them
as planning.

Adapters call `validate` before deduplication and `plan` against locked state
inside their transaction. A new run starts with `Submitted`; approved answers
complete a run. Failed, cancelled, interrupted and completed states are terminal.
Deduplicate before planning so terminal retries return their original receipt.
A new event cannot reopen a terminal run. `Plan` includes the affected message
projection, tool receipt and model-call usage key; these belong to the same commit.

An adapter implements `AtomicPersistence::commit`: authenticate/authorize, acquire
the per-conversation lock, deduplicate, read locked run state, apply `plan`, write
the event/detail/projection/state and commit. Stable idempotency keys must detect
conflicting retries rather than acknowledging different content. Usage is unique
per run/model-call; tool success/failure receipts are unique per run/tool-call.
Tool name is display metadata, never a call identity. Adapters must reserve every
acknowledged alternate event ID/key so later conflicting reuse is rejected.

`append(store, notification, command)` awaits the committed receipt before calling
`Notification::committed`. Persistence failure emits no notification. A duplicate
emits no second notification. `AppendOutcome::CommittedButNotNotified` contains
the durable receipt and delivery error: notification failure never undoes history.
Direct `commit`/adapter append intentionally provides persistence only.

A payload is bounded to 64 KiB. Full exposed text may use `Text { preview, detail }`
with a detail ID and an associated command detail body up to 2 MiB. Reference and
body are validated and committed together. A replay batch returns up to 100 scanned
records and 1 MiB of serialized public events. `scanned_through` includes restricted
records; clients resume after it instead of expecting contiguous public sequences.
`high_watermark` identifies the locked committed snapshot, and `has_more` indicates
whether further scanning is needed. Unknown versions are errors, including versions
of restricted records passed during a public scan.

`tests/independent_consumer.rs` implements a separate notebook application's atomic
persistence and notification ports with no Kyomi dependency, proving that the API
can be consumed independently. The Kyomi adapter's actual PostgreSQL and SQLite
transaction conformance tests live in `kyomi-auth::conversation_events::tests`.
