// SPDX-License-Identifier: AGPL-3.0-or-later

//! Knowledge file utilities — text chunking and table reference extraction.
//!
//! These utility functions are used by `dashboard_service::rechunk_document`
//! to split document content into chunks and extract table references.
//! The CRUD operations that previously lived here (operating on the now-dropped
//! `knowledge_files` table) have been removed.

use regex::Regex;
use std::sync::LazyLock;

/// Regex for extracting backtick-wrapped table references (e.g., `schema.table`).
static TABLE_REF_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"`(\w+\.\w+(?:\.\w+)?)`").expect("valid regex"));

// ---------------------------------------------------------------------------
// Chunking
// ---------------------------------------------------------------------------

/// Split text into approximately byte-sized chunks with overlap.
/// A chunk always includes at least one whole character, even for a zero-byte size.
pub fn split_into_chunks(text: &str, chunk_size: usize, overlap: usize) -> Vec<String> {
    if text.is_empty() {
        return vec![];
    }
    if text.len() <= chunk_size {
        return vec![text.to_string()];
    }

    let mut chunks = Vec::new();
    let mut start = 0;

    while start < text.len() {
        let mut end = start.saturating_add(chunk_size.max(1)).min(text.len());

        // Back up to a valid UTF-8 char boundary
        while !text.is_char_boundary(end) && end > start {
            end -= 1;
        }

        // A byte budget smaller than the next character must still make progress.
        if end == start {
            end = start + text[start..].chars().next()
                .expect("start is before text end").len_utf8();
        }

        // Try to break at a paragraph or sentence boundary
        let chunk_end = if end < text.len() {
            find_break_point(text, start, end)
        } else {
            end
        };

        chunks.push(text[start..chunk_end].to_string());

        if chunk_end >= text.len() {
            break;
        }

        // Next chunk starts at (end - overlap), but never before current start + 1
        let mut next_start = if chunk_end > overlap {
            chunk_end - overlap
        } else {
            chunk_end
        };

        // Round backward to retain the whole character in the overlap.
        while !text.is_char_boundary(next_start) {
            next_start -= 1;
        }

        if next_start <= start {
            // Safety: always advance
            start = chunk_end;
        } else {
            start = next_start;
        }
    }

    chunks
}

/// Find a good break point near `target_end` within the text.
/// Prefers paragraph breaks (\n\n), then line breaks (\n), then sentence ends.
fn find_break_point(text: &str, start: usize, target_end: usize) -> usize {
    let mut search_start = target_end.saturating_sub(200).max(start);
    while !text.is_char_boundary(search_start) {
        search_start -= 1;
    }
    let segment = &text[search_start..target_end];

    // Prefer paragraph break
    if let Some(pos) = segment.rfind("\n\n") {
        return search_start + pos + 2;
    }

    // Then line break
    if let Some(pos) = segment.rfind('\n') {
        return search_start + pos + 1;
    }

    // Then sentence end
    if let Some(pos) = segment.rfind(". ") {
        return search_start + pos + 2;
    }

    // Fall back to target_end
    target_end
}

// ---------------------------------------------------------------------------
// Table reference extraction
// ---------------------------------------------------------------------------

/// Extract table references from content.
///
/// Looks for backtick-wrapped identifiers matching `word.word` pattern
/// (at least one dot, no spaces). E.g., `` `billing.subscriptions` `` matches.
pub fn extract_table_references(content: &str) -> Vec<String> {
    let re = &*TABLE_REF_RE;
    let mut refs: Vec<String> = re
        .captures_iter(content)
        .map(|cap| cap[1].to_string())
        .collect();

    refs.sort();
    refs.dedup();
    refs
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // -- extract_table_references tests --

    #[test]
    fn extract_simple_table_ref() {
        let content = "The data is in `billing.subscriptions` table.";
        let refs = extract_table_references(content);
        assert_eq!(refs, vec!["billing.subscriptions"]);
    }

    #[test]
    fn extract_three_part_table_ref() {
        let content = "Query `project.dataset.orders` for results.";
        let refs = extract_table_references(content);
        assert_eq!(refs, vec!["project.dataset.orders"]);
    }

    #[test]
    fn extract_multiple_refs_deduped() {
        let content = "Join `billing.subscriptions` with `billing.invoices`. \
                        Also check `billing.subscriptions` again.";
        let refs = extract_table_references(content);
        assert_eq!(refs, vec!["billing.invoices", "billing.subscriptions"]);
    }

    #[test]
    fn no_refs_for_plain_backtick_words() {
        let content = "The `amount` column is in cents. Use `status = 'active'`.";
        let refs = extract_table_references(content);
        assert!(refs.is_empty());
    }

    #[test]
    fn no_refs_for_code_blocks() {
        // Backtick-wrapped identifiers inside code blocks should still be caught
        // since we're doing simple regex matching (this is by design).
        let content = "```sql\nSELECT * FROM `public.orders`\n```";
        let refs = extract_table_references(content);
        assert_eq!(refs, vec!["public.orders"]);
    }

    // -- split_into_chunks tests --

    #[test]
    fn short_text_single_chunk() {
        let text = "Hello world";
        let chunks = split_into_chunks(text, 2000, 400);
        assert_eq!(chunks.len(), 1);
        assert_eq!(chunks[0], "Hello world");
    }

    #[test]
    fn long_text_multiple_chunks() {
        // Create text that's definitely longer than chunk size
        let text = "A ".repeat(1500); // 3000 chars
        let chunks = split_into_chunks(&text, 2000, 400);
        assert!(chunks.len() >= 2, "Expected >= 2 chunks, got {}", chunks.len());
    }

    #[test]
    fn chunks_cover_all_content() {
        let text = "Word. ".repeat(500); // 3000 chars
        let chunks = split_into_chunks(&text, 2000, 400);
        // Verify no content is lost: the first chunk's start and last chunk's end
        // should cover the original text
        assert!(chunks[0].starts_with("Word. "));
        assert!(chunks.last().unwrap().ends_with("Word. "));
    }

    #[test]
    fn empty_text_no_chunks() {
        let chunks = split_into_chunks("", 2000, 400);
        let expected: Vec<String> = vec![];
        assert_eq!(chunks, expected);
    }

    // These fixtures have no preferred breaks, so byte offsets can independently
    // verify every chunk, the overlap, and complete coverage without deduplication.
    fn assert_chunk_coverage(text: &str, chunk_size: usize, overlap: usize) {
        let chunks = split_into_chunks(text, chunk_size, overlap);
        assert!(!chunks.is_empty());
        assert!(chunks.len() <= text.chars().count(), "must advance by a character");
        let mut start = 0;
        let mut covered_end = 0;
        for (index, chunk) in chunks.iter().enumerate() {
            assert!(!chunk.is_empty(), "chunk {index} must make progress");
            assert!(chunk.len() <= chunk_size.max(4));
            let end = start + chunk.len();
            assert_eq!(text.get(start..end), Some(chunk.as_str()));
            assert!(start <= covered_end, "no gaps between chunks");
            assert!(end >= covered_end, "coverage must not move backward");
            covered_end = end;
            if end == text.len() {
                assert_eq!(index + 1, chunks.len());
                break;
            }
            // Choose the nearest preceding character boundary for the requested
            // overlap, independently via char_indices rather than byte rounding.
            let desired = if end > overlap { end - overlap } else { end };
            let next_start = text.char_indices().map(|(offset, _)| offset)
                .take_while(|offset| *offset <= desired).last().unwrap();
            let next_start = if next_start > start { next_start } else { end };
            if next_start < end {
                let actual_overlap = end - next_start;
                assert!(actual_overlap >= overlap);
                assert!(actual_overlap <= overlap.saturating_add(3));
                assert!(chunks[index + 1].starts_with(&text[next_start..end]));
            }
            assert!(next_start > start, "every chunk must advance its start");
            start = next_start;
        }
        assert_eq!(covered_end, text.len(), "all source bytes must be covered");
    }

    #[test]
    fn unicode_search_window_boundaries_preserve_content_and_overlap() {
        for character in ["—", "🙂", "界"] {
            let text = format!("{}{character}{}", "a".repeat(1798), "b".repeat(300));
            assert_chunk_coverage(&text, 2000, 400);
        }
        assert_chunk_coverage(&"—🙂界".repeat(500), 2001, 400);
    }

    #[test]
    fn unicode_overlap_boundaries_preserve_content_and_overlap() {
        for character in ["—", "🙂", "界"] {
            let text = format!("{}{character}{}", "a".repeat(1599), "b".repeat(601));
            assert_chunk_coverage(&text, 2000, 400);
        }
        // Both the search window and the overlap fall inside repeated characters.
        assert_chunk_coverage(&"界".repeat(1000), 2000, 400);
    }

    #[test]
    fn unicode_chunk_end_boundaries_preserve_whole_characters() {
        for character in ["—", "🙂", "界"] {
            let text = format!("{}{character}{}", "a".repeat(1999), "b".repeat(301));
            assert_chunk_coverage(&text, 2000, 400);
        }
    }

    #[test]
    fn tiny_byte_budgets_and_large_overlap_terminate_with_complete_content() {
        for size in 0..=5 {
            for overlap in [0, 1, 4, 400, usize::MAX] {
                assert_chunk_coverage("🙂界—abc🙂界", size, overlap);
            }
        }
        assert_eq!(split_into_chunks("abc", usize::MAX, 400), vec!["abc"]);
        assert_eq!(split_into_chunks("", 0, 0), Vec::<String>::new());
    }

    #[test]
    fn ascii_chunks_keep_byte_size_and_overlap() {
        let text = "abcdefghijklmnopqrstuvwxyz";
        assert_eq!(split_into_chunks(text, 10, 3), vec![
            "abcdefghij", "hijklmnopq", "opqrstuvwx", "vwxyz",
        ]);
        assert_chunk_coverage(text, 10, 3);
    }

    #[test]
    fn chunks_prefer_paragraph_then_line_then_sentence_breaks() {
        for (suffix, expected_end) in [
            ("paragraph\n\nline\nsentence. tail", "paragraph\n\n"),
            ("line\nsentence. tail", "line\n"),
            ("sentence. tail", "sentence. "),
        ] {
            let prefix = "a".repeat(1800);
            let text = format!("{prefix}{suffix}{}", "b".repeat(400));
            let chunks = split_into_chunks(&text, 2000, 400);
            assert_eq!(chunks[0], format!("{prefix}{expected_end}"));
        }
    }

}
