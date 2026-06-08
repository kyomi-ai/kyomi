-- SPDX-License-Identifier: AGPL-3.0-or-later
-- Stores full (untruncated) reasoning text for agent thinking events.
-- The main thinking event metadata in chat_messages.extra_metadata carries a
-- 200-char truncated title for fast streaming/display. This table holds the
-- complete cleaned text, fetched on demand when the user clicks "more".
-- Encrypted at rest (AES-256-GCM) like chat_messages.content.

CREATE TABLE thinking_event_details (
    id TEXT PRIMARY KEY,
    message_id TEXT NOT NULL REFERENCES chat_messages(message_id) ON DELETE CASCADE,
    event_id TEXT NOT NULL,
    full_text TEXT NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX idx_thinking_event_details_message
    ON thinking_event_details (message_id);

CREATE UNIQUE INDEX idx_thinking_event_details_message_event
    ON thinking_event_details (message_id, event_id);
