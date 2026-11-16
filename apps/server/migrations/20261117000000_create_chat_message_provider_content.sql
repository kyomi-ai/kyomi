-- SPDX-License-Identifier: AGPL-3.0-or-later
-- KYO-969: the provider's verbatim assistant content blocks for one chat message.
--
-- Anthropic's server-side web search/fetch returns `server_tool_use`,
-- `web_search_tool_result` and `web_fetch_tool_result` blocks whose
-- `encrypted_content` / `encrypted_index` values must be sent back unchanged in
-- later turns. `chat_messages.content` only keeps the answer text, so the blocks
-- are stored here, keyed by the message they belong to. Keying by message_id (and
-- cascading) keeps them aligned with edits, deletes and rewinds, which an
-- ordering match against the event journal could not guarantee.
--
-- `blocks` is the encrypted JSON array, in the same envelope as
-- chat_messages.tool_calls. `backend` names the provider format the blocks are
-- in; replay is only attempted for a matching backend, and a message with no row
-- (or an unreadable one) falls back to the text in chat_messages.
CREATE TABLE chat_message_provider_content (
    message_id TEXT PRIMARY KEY REFERENCES chat_messages(message_id) ON DELETE CASCADE,
    backend TEXT NOT NULL,
    blocks JSONB NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);
