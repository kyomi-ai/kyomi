// SPDX-License-Identifier: AGPL-3.0-or-later

//! Chat pages — list, message display, session loading.

pub mod chat_list;
pub mod chat_message;
pub mod chat_page;

pub use chat_list::ChatsListPage;
pub use chat_message::ChatMessage;
pub use chat_page::ChatPage;

// ─── Shared helpers ─────────────────────────────────────────────────────────

/// Format an RFC 3339 timestamp as a human-readable relative time string.
///
/// Matches React's `formatRelativeTime()` from `lib/formatters.js` and
/// `formatDate()` from `ChatsList.jsx`.
///
/// Used by both `ChatMessage` and `ChatsListPage`.
pub(crate) fn format_relative_time(rfc3339: &str) -> String {
    use chrono::Datelike as _;
    let Ok(parsed) = chrono::DateTime::parse_from_rfc3339(rfc3339) else {
        return rfc3339.to_string();
    };

    let now = chrono::Utc::now();
    let duration = now.signed_duration_since(parsed);

    let seconds = duration.num_seconds();
    if seconds < 60 {
        return "Just now".to_string();
    }

    let minutes = duration.num_minutes();
    if minutes < 60 {
        return format!("{minutes}m ago");
    }

    let hours = duration.num_hours();
    if hours < 24 {
        return format!("{hours}h ago");
    }

    let days = duration.num_days();
    if days < 7 {
        return format!("{days}d ago");
    }

    // For older dates, show "Mar 15" or "Mar 15, 2025".
    // Use numeric day (u32) to avoid %-d (no-pad) which is a GNU extension that
    // can produce Err(fmt::Error) via chrono in some build targets.
    let parsed_utc = parsed.with_timezone(&chrono::Utc);
    let month = parsed_utc.format("%b").to_string();
    let day = parsed_utc.day();
    if parsed_utc.year() == now.year() {
        format!("{month} {day}")
    } else {
        format!("{month} {day}, {}", parsed_utc.year())
    }
}
