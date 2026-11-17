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
-- chat_messages.tool_calls. It is replayed exactly as stored; a message with no
-- row (sessions older than this change, answers that used no server tool) has
-- nothing to replay.
CREATE TABLE chat_message_provider_content (
    message_id TEXT PRIMARY KEY REFERENCES chat_messages(message_id) ON DELETE CASCADE,
    blocks JSONB NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);
