-- SPDX-License-Identifier: AGPL-3.0-or-later
--
-- KYO-493 (Phase 1): add `status` and `owner_instance` to `chat_messages`.
--
-- Today the assistant's row for a turn is written exactly once, at the end
-- of the agent loop (`kyomi_agent::adapter::ChatAgentAdapter::persist_after_chat`).
-- There is no durable record of "a turn is in progress" while the agent is
-- still running, and no durable record of *how* a turn ended beyond what
-- happens to be in `extra_metadata`. This migration adds the two columns
-- Phase 2 needs to make both of those observable:
--
--   * `status` — the lifecycle of a `chat_messages` row. One of
--     'in_progress' | 'complete' | 'error' | 'cancelled' | 'interrupted'
--     (see `kyomi_auth::chat_service::MessageStatus` for the Rust side of
--     this mapping — an explicit enum with its own `as_str`/`FromStr`, not
--     free strings scattered across call sites). Every row written before
--     this migration was, definitionally, a finished row — hence the
--     'complete' default applied to existing data. Phase 2 uses
--     'in_progress' for the empty assistant placeholder
--     `chat_service::prepare_chat_dispatch` writes before the agent is
--     spawned, and 'error' / 'cancelled' for the two ways a turn can end
--     without a real answer. 'interrupted' is reserved for the later
--     stuck-row sweep (KYO-493 Phase 4: startup sweep of an instance's own
--     in_progress rows, a graceful-shutdown sweep, and a hard-timeout age
--     bound) that reclassifies an 'in_progress' row whose owning process is
--     no longer running — nothing in Phase 1/2 writes it yet.
--   * `owner_instance` — which server process currently owns an
--     'in_progress' row, so that the Phase 4 sweep above can tell "still
--     being worked on by a live process" apart from "the process that was
--     writing this died mid-turn." Nullable: only ever populated for a row
--     that was `in_progress` at some point; a `complete` row written before
--     this migration (or by an `AdapterInserts` caller that never
--     pre-inserts a placeholder — copilot, Slack, watch execution) has no
--     owning process to record. Holds `"{HOSTNAME}:{PORT}"` outside
--     personal mode (the port disambiguates multiple server processes on
--     one machine sharing both HOSTNAME and Postgres — e.g. dev.kyomi.ai
--     plus per-worktree verifier servers) or the fixed literal `"desktop"`
--     in personal mode — see `kyomi_core::resolve_process_instance`,
--     resolved once at server startup, not the value of `HOSTNAME` alone.
--
-- Both columns are purely additive; nothing here changes how any existing
-- row is read.
ALTER TABLE public.chat_messages
    ADD COLUMN IF NOT EXISTS status character varying(20) NOT NULL DEFAULT 'complete'
        CHECK (status IN ('in_progress', 'complete', 'error', 'cancelled', 'interrupted'));

ALTER TABLE public.chat_messages
    ADD COLUMN IF NOT EXISTS owner_instance character varying(255);

COMMENT ON COLUMN public.chat_messages.status IS
    'Lifecycle of this row: in_progress | complete | error | cancelled | interrupted. See kyomi_auth::chat_service::MessageStatus. Existing rows default to complete.';

COMMENT ON COLUMN public.chat_messages.owner_instance IS
    'Identity of the server process that owns this row while status = in_progress: "{HOSTNAME}:{PORT}", or "desktop" in personal mode (see kyomi_core::resolve_process_instance, resolved once at server startup). NULL for rows that were never in_progress.';
