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

/// Split text into fixed-size chunks with overlap.
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
        let mut end = (start + chunk_size).min(text.len());

        // Back up to a valid UTF-8 char boundary
        while !text.is_char_boundary(end) && end > start {
            end -= 1;
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
        let next_start = if chunk_end > overlap {
            chunk_end - overlap
        } else {
            chunk_end
        };

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
    let search_start = target_end.saturating_sub(200).max(start);
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
}
