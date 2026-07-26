// SPDX-License-Identifier: AGPL-3.0-or-later

//! Shared Unicode-safe string helpers.
//!
//! `&str[..N]` is a **byte** slice in Rust. If `N` falls in the middle of a
//! multi-byte UTF-8 character, indexing panics at runtime with
//! `byte index N is not a char boundary`. This module centralizes the safe
//! alternative so no call site has to reimplement it.
//!
//! Originally introduced in `kyomi-agent` (KYO-211) for three call sites in
//! that crate. Moved here (KYO-241) because a further six unsafe byte-slice
//! truncation sites were found across `kyomi-auth` and `kyomi-ui` — the
//! latter is a WASM client crate that cannot depend on `kyomi-core` or
//! `kyomi-agent`, so `kyomi-types` is the only crate reachable from both the
//! client and every server crate. Do not add another copy of this function;
//! import it from here.

/// Truncate `message` to at most `max_chars` **characters** (Unicode-safe)
/// and append `"..."` if truncation occurred.
///
/// Unlike `&message[..N]`, this never panics on multi-byte UTF-8 content —
/// the cut point is always a character boundary, found via `char_indices`
/// rather than a raw byte offset.
pub fn truncate_preview(message: &str, max_chars: usize) -> String {
    match message.char_indices().nth(max_chars) {
        // There is a character at index `max_chars` — `boundary` is its byte
        // offset, i.e. the byte offset of the first char to drop. Slicing
        // there is always safe: `char_indices` only ever yields boundaries.
        Some((boundary, _)) => format!("{}...", &message[..boundary]),
        // Fewer than `max_chars` characters total — nothing to truncate.
        None => message.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // -- Callers' existing behavior (moved from alert.rs, KYO-211) -----------

    #[test]
    fn short_message_is_unchanged() {
        let message = "Short message";
        let preview = truncate_preview(message, 200);
        assert_eq!(preview, "Short message");
    }

    #[test]
    fn long_ascii_message_truncates_at_char_count() {
        let message = "A".repeat(300);
        let preview = truncate_preview(&message, 200);
        assert_eq!(preview.len(), 203); // 200 'A's + "..."
        assert!(preview.ends_with("..."));
    }

    #[test]
    fn long_unicode_message_truncates_by_chars_not_bytes() {
        // Each of these is a multi-byte character.
        let message = "日".repeat(250);
        let preview = truncate_preview(&message, 200);
        assert!(preview.ends_with("..."));
        let content = preview.trim_end_matches("...");
        assert_eq!(content.chars().count(), 200);
    }

    #[test]
    fn exact_length_is_unchanged() {
        let message = "A".repeat(200);
        let preview = truncate_preview(&message, 200);
        assert_eq!(preview, message); // No truncation needed
    }

    #[test]
    fn empty_string_is_unchanged() {
        let preview = truncate_preview("", 200);
        assert_eq!(preview, "");
    }

    #[test]
    fn one_char_max_truncates_to_single_char() {
        let preview = truncate_preview("Hello", 1);
        assert_eq!(preview, "H...");
    }

    // -- KYO-211: exact call-site inputs (watch prompt @ 200, agent response @ 500) --

    #[test]
    fn prompt_200_char_limit_mid_char_byte_boundary_does_not_panic() {
        // 199 ASCII bytes + one 3-byte CJK char: the byte offset 200 (the
        // old `&s[..200]` cut point) lands inside that character's encoding.
        // This is the exact input that panicked pre-fix (watch.rs:693).
        let prompt = format!("{}{}{}", "a".repeat(199), "日", "b".repeat(10));
        assert!(prompt.len() > 200, "prompt must exceed the byte-length fast path");

        let preview = truncate_preview(&prompt, 200);

        assert!(preview.ends_with("..."));
        let content = preview.trim_end_matches("...");
        assert_eq!(content.chars().count(), 200, "must keep exactly 200 characters");
    }

    #[test]
    fn agent_response_500_char_limit_mid_char_byte_boundary_does_not_panic() {
        // 499 ASCII bytes + one 3-byte CJK char: byte offset 500 (the old
        // `&r[..500]` cut point) lands inside that character's encoding.
        // This is the exact input that panicked pre-fix (watch.rs:919 and
        // watch_execution.rs:515).
        let response = format!("{}{}{}", "a".repeat(499), "日", "b".repeat(10));
        assert!(response.len() > 500, "response must exceed the byte-length fast path");

        let preview = truncate_preview(&response, 500);

        assert!(preview.ends_with("..."));
        let content = preview.trim_end_matches("...");
        assert_eq!(content.chars().count(), 500, "must keep exactly 500 characters");
    }

    // -- Emoji (4-byte UTF-8 sequences) --------------------------------------

    #[test]
    fn emoji_truncation_keeps_whole_emoji_and_char_count() {
        // 🎉 is a 4-byte UTF-8 sequence.
        let message = "🎉".repeat(210);
        let preview = truncate_preview(&message, 200);
        assert!(preview.ends_with("..."));
        let content = preview.trim_end_matches("...");
        assert_eq!(content.chars().count(), 200);
        assert_eq!(content, "🎉".repeat(200));
    }

    #[test]
    fn emoji_mixed_with_ascii_mid_sequence_does_not_panic() {
        // 199 ASCII bytes + one 4-byte emoji: byte offset 200 lands inside
        // the emoji's encoding (bytes 199..203).
        let message = format!("{}{}", "x".repeat(199), "🎉🎉🎉");
        assert!(message.len() > 200);

        let preview = truncate_preview(&message, 200);

        assert!(preview.ends_with("..."));
        let content = preview.trim_end_matches("...");
        assert_eq!(content.chars().count(), 200);
        assert_eq!(content, format!("{}{}", "x".repeat(199), "🎉"));
    }

    // -- Accented / CJK content ------------------------------------------------

    #[test]
    fn accented_text_truncates_on_char_boundary() {
        // "é" here is a single composed 2-byte codepoint (U+00E9).
        let message = "café ".repeat(50); // 250 chars, 2 non-ASCII bytes per "é"
        assert!(message.chars().count() > 200);

        let preview = truncate_preview(&message, 200);

        assert!(preview.ends_with("..."));
        let content = preview.trim_end_matches("...");
        assert_eq!(content.chars().count(), 200);
        assert_eq!(content, message.chars().take(200).collect::<String>());
    }

    #[test]
    fn cjk_text_truncates_on_char_boundary() {
        let message = "見積もりを毎日確認する".repeat(20); // 3-byte-per-char CJK
        assert!(message.chars().count() > 200);

        let preview = truncate_preview(&message, 200);

        assert!(preview.ends_with("..."));
        let content = preview.trim_end_matches("...");
        assert_eq!(content.chars().count(), 200);
        assert_eq!(content, message.chars().take(200).collect::<String>());
    }

    // -- Boundary cases -------------------------------------------------------

    #[test]
    fn one_char_over_limit_truncates() {
        let message = "A".repeat(201);
        let preview = truncate_preview(&message, 200);
        assert_eq!(preview, format!("{}...", "A".repeat(200)));
    }

    #[test]
    fn one_char_under_limit_is_unchanged() {
        let message = "A".repeat(199);
        let preview = truncate_preview(&message, 200);
        assert_eq!(preview, message);
    }

    #[test]
    fn exactly_at_limit_is_unchanged_unicode() {
        let message = "日".repeat(200);
        let preview = truncate_preview(&message, 200);
        assert_eq!(preview, message);
    }
}
