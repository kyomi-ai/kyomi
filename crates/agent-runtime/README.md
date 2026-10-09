# agent-runtime

An application-independent durable conversation foundation. Kyomi is one adapter;
this crate has no Kyomi, SQLx, HTTP, Redis, provider or product-tool dependency.
No publication or license change is part of this addition.

The foundation consists of typed identities, version 1 commands/events, a pure
transition planner, an atomic persistence port, post-commit notifications and
public replay DTOs. Responses are complete records; there are no token deltas.
Complete-provider ingestion and awaited tool execution are exposed through reusable
ports. Client reducers and application interface delivery remain adapter responsibilities. The lifecycle module supplies submission
idempotency, queued ownership, leases/heartbeats/fencing, cancellation and terminal
projection policy for this stage.

`AppendCommand` names a conversation, run, event and idempotency key. Identity
newtypes wrap adapter-defined strings (1–128 bytes), allowing existing applications
to keep stable IDs. `Payload` separates `PublicPayload` from `RestrictedPayload`.
Public replay uses `PublicEvent` and cannot carry a restricted payload. Public
kinds cover submission, run state, complete model response, exposed planning,
tool intent/start/result, usage, validation, approved answer, cancellation request,
cancellation, truthful failure and interruption. Restricted kinds preserve complete provider data
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

A public payload is bounded to 64 KiB; complete restricted provider/candidate payloads
are bounded to 2 MiB. Full exposed text may use `Text { preview, detail }`
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

Lifecycle adapters call `plan_submission` against the locked request identity and
`plan_claim`, `plan_heartbeat`, `plan_cancel`, `plan_finalize` or `plan_expire` against
locked run/conversation state. `plan_interrupt_queued` lets an adapter terminalize
queued work whose initiating identity lost authorization. Live owners and already
terminal runs are left unchanged. `AtomicLifecyclePersistence` commits the resulting
queue state, timestamps, journal events and compatibility projections together in
the same persistence infrastructure as `AtomicPersistence`. Request identity
includes authorized conversation, actor/source, context and exposed body, while
generated record IDs and server acceptance time may differ on retries. A retry
returns the original run. Adapters supply clocks in milliseconds and positive
lease durations; expiration is inclusive. Fences are monotonic integers and all
owner writes validate current stored expiration under lock. Heartbeats may retain
the original fence token. Expiry interrupts a running run; it does not reexecute it.
Cancellation persisted before completion wins, and every terminal plan produces
one assistant projection even before the first provider response. Terminal plans
must be committed once; adapters deduplicate committed command retries before
planning. Authorization, queue discovery and loading committed context at claim
time remain adapter responsibilities. `tests/lifecycle_consumer.rs` demonstrates
this contract through an independent notebook queue adapter.

`PublicPayload::MessageRecorded` carries a complete assistant or tool message and
its original message/tool-call metadata for owned compatibility history writes.
It does not infer tool success, approve an answer or complete a run. The adapter
commits any corresponding compatibility message in the same fenced event
transaction. `PublicPayload::Failed` retains truthful failure text in public replay,
including failures before the first provider response.

The execution module provides `CompleteProvider`, `ToolExecution` and `EventSink`.
`ExecutionContext::complete` persists complete restricted provider/continuation data,
the candidate, per-model-call usage and adapter-cleaned exposed planning before
returning to the caller. A planning preview and its full body use one atomic command;
short planning has no detail reference. Envelope IDs use bounded deterministic hashes.
The provider port supplies complete responses only; it exposes no token API.

`ExecutionContext::tool` first recovers a committed result receipt when one exists.
Otherwise it awaits intent and start commits before exactly one tool invocation, then
awaits the typed transport/domain outcome. A transport-completed result can have a
rejected domain outcome. Repeated names have distinct call IDs. A duplicate start
without a result is uncertain and refuses execution. Retrying result persistence must
retry the same command, never invoke the action again. `SinkReceipt` distinguishes
committed history from notification failure without undoing the commit.

This contract cannot promise exactly-once external effects. An adapter that owns a
local mutation transaction can connect a result receipt to that transaction; a tool
with an internally owned transaction or a remote action may commit a side effect
before result persistence fails. Recovery must record that outcome as unknown and
interrupt the run, preserving the effect and refusing blind retries. Provider and tool
credentials, product validation and authorization remain application responsibilities.
