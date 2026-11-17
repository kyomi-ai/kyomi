-- SPDX-License-Identifier: AGPL-3.0-or-later
-- KYO-969: the provider's verbatim assistant content blocks for one chat message.
-- Mirrors Postgres migration 20261117000000_create_chat_message_provider_content.
CREATE TABLE IF NOT EXISTS chat_message_provider_content (
    message_id TEXT NOT NULL PRIMARY KEY REFERENCES chat_messages(message_id) ON DELETE CASCADE,
    blocks TEXT NOT NULL,
    created_at TEXT NOT NULL DEFAULT (datetime('now'))
);
