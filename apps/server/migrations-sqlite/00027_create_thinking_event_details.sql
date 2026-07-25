-- SPDX-License-Identifier: AGPL-3.0-or-later
-- Stores full (untruncated) reasoning text for agent thinking events.
-- Mirrors Postgres migration 20260608000000_create_thinking_event_details.
-- The main thinking event metadata in chat_messages.extra_metadata carries a
-- 200-char truncated title for fast streaming/display. This table holds the
-- complete cleaned text, fetched on demand when the user clicks "more".
-- Encrypted at rest (AES-256-GCM) like chat_messages.content.

CREATE TABLE IF NOT EXISTS thinking_event_details (
    id TEXT NOT NULL PRIMARY KEY,
    message_id TEXT NOT NULL REFERENCES chat_messages(message_id) ON DELETE CASCADE,
    event_id TEXT NOT NULL,
    full_text TEXT NOT NULL,
    created_at TEXT NOT NULL DEFAULT (datetime('now'))
);

CREATE INDEX IF NOT EXISTS idx_thinking_event_details_message
    ON thinking_event_details (message_id);

CREATE UNIQUE INDEX IF NOT EXISTS idx_thinking_event_details_message_event
    ON thinking_event_details (message_id, event_id);
