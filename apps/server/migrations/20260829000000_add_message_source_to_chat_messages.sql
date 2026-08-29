-- SPDX-License-Identifier: AGPL-3.0-or-later
--
-- KYO-506: add `message_source` alongside the existing
-- `current_time_user_tz` column so the annotation `build_metadata_prefix`
-- (kyomi-agent) puts in front of a user message for the LLM — e.g.
-- `[source: web, user_local_time: 2026-08-23T...]` — can be reconstructed
-- when rebuilding agent context, instead of only ever being available for
-- the live turn.
--
-- Background: KYO-492 moved the user-message write for the main chat path
-- (`kyomi_auth::chat_service::prepare_chat_dispatch`) from
-- `ChatAgentAdapter::persist_after_chat` (after the agent loop finished) to
-- before the agent is even spawned, so a mid-run page reload shows the
-- user's own message. That fix also changed what gets stored: the raw
-- message, not the metadata-prefixed content `CustomAgent::chat` builds for
-- the LLM. Display was unaffected (`get_session_messages` already strips
-- the prefix before the UI sees it), but `get_agent_messages` — the
-- LLM-context path `ChatAgentAdapter::load_context` uses to rebuild history
-- — does not strip it, so it also never had anything to reconstruct from.
-- Every prior turn's source/local-time annotation stopped reaching the
-- model from the turn after it was sent.
--
-- This column is purely additive and nullable, and is populated only for
-- messages stored going forward:
--   * Rows written before this migration never had a `message_source` to
--     record — there is nothing to backfill, and this migration does not
--     attempt to guess one. Reconstruction for those rows degrades to a
--     time-only annotation (or no annotation at all, matching the historic
--     behavior on a row with neither column populated).
--   * `current_time_user_tz` (this same table) already stores the other
--     half of the annotation and needed no migration — this file adds only
--     what was actually missing.
--
-- Matches `current_time_user_tz`'s `character varying(50)` — both hold a
-- short, fixed-vocabulary identifier ("web", "slack", "mcp", "Kyomi Watch"),
-- never free text.
ALTER TABLE public.chat_messages
    ADD COLUMN IF NOT EXISTS message_source character varying(50);

COMMENT ON COLUMN public.chat_messages.message_source IS
    'Where this message originated ("web", "slack", "mcp", "Kyomi Watch", ...). NULL for messages stored before KYO-506 and for roles other than the user turn. Used with current_time_user_tz to reconstruct the metadata prefix agent.chat() built for the LLM when rebuilding agent context (kyomi_agent::adapter::ChatAgentAdapter::load_context).';
