// SPDX-License-Identifier: AGPL-3.0-or-later

//! Shared time/timezone helpers for the Leptos frontend.
//!
//! Used by both `chat_page.rs` (main chat) and `chat_engine.rs` (copilot) to
//! pass the user's local time and IANA timezone to server functions.

/// Parse a timestamp string that may be RFC 3339 or Postgres format.
///
/// Handles `2026-06-05T09:40:53Z` (RFC 3339) and
/// `2026-06-05 09:40:53.348324+00` (Postgres).
pub fn parse_timestamp(s: &str) -> Option<chrono::DateTime<chrono::FixedOffset>> {
    chrono::DateTime::parse_from_rfc3339(s)
        .or_else(|_| chrono::DateTime::parse_from_str(s, "%Y-%m-%d %H:%M:%S%.f%#z"))
        .ok()
}

/// Compute the current time with timezone offset for agent awareness.
///
/// Returns a string in `YYYY-MM-DDTHH:MM:SS±HH:MM` format.
/// Matches React's `getTimeContext()` in ChatInterface.jsx (lines 384-407).
///
/// On SSR, returns an empty string (time context is only relevant client-side).
pub fn get_time_context() -> String {
    #[cfg(target_arch = "wasm32")]
    {
        let now = js_sys::Date::new_0();

        // getTimezoneOffset() returns minutes *west* of UTC (negative = east).
        // React: `const offsetMinutes = -now.getTimezoneOffset();`
        let raw_offset = now.get_timezone_offset() as i32; // minutes west of UTC
        let offset_minutes = -raw_offset; // minutes east of UTC

        let offset_sign = if offset_minutes >= 0 { '+' } else { '-' };
        let abs_offset = offset_minutes.unsigned_abs();
        let offset_hours = abs_offset / 60;
        let offset_mins = abs_offset % 60;

        let year = now.get_full_year();
        let month = now.get_month() + 1; // JS months are 0-indexed
        let date = now.get_date();
        let hours = now.get_hours();
        let minutes = now.get_minutes();
        let seconds = now.get_seconds();

        format!(
            "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}{}{:02}:{:02}",
            year, month, date, hours, minutes, seconds, offset_sign, offset_hours, offset_mins
        )
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        String::new()
    }
}

/// Return the user's IANA timezone name (e.g. "America/Los_Angeles").
///
/// Uses `Intl.DateTimeFormat().resolvedOptions().timeZone` on WASM.
/// Returns `"UTC"` on non-WASM (SSR).
pub fn get_user_timezone() -> String {
    #[cfg(target_arch = "wasm32")]
    {
        use wasm_bindgen::JsValue;

        // Intl.DateTimeFormat().resolvedOptions().timeZone
        let intl = js_sys::Intl::DateTimeFormat::new(&js_sys::Array::new(), &js_sys::Object::new());
        let options = intl.resolved_options();

        // resolvedOptions() returns a plain JS object; access .timeZone via Reflect.
        let tz = js_sys::Reflect::get(&options, &JsValue::from_str("timeZone"))
            .ok()
            .and_then(|v| v.as_string());

        tz.unwrap_or_else(|| "UTC".to_string())
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        "UTC".to_string()
    }
}
