// SPDX-License-Identifier: AGPL-3.0-or-later

//! Markdown + ChartML renderer component.
//!
//! Splits content into alternating Markdown and ChartML segments,
//! rendering Markdown via `pulldown-cmark` and ChartML as styled
//! placeholder code blocks (actual chart rendering added in Phase 2).

use leptos::prelude::*;

/// A segment of dashboard content — either plain Markdown or a ChartML block.
#[derive(Clone, Debug, PartialEq)]
enum ContentSegment {
    /// Plain markdown text to be rendered as HTML.
    Markdown(String),
    /// ChartML YAML content extracted from a ```chartml fenced code block.
    ChartML(String),
}

/// Parse content into alternating Markdown and ChartML segments.
///
/// Scans for ```chartml fenced code blocks using a simple string-based
/// approach. Everything outside those blocks is treated as Markdown.
fn parse_segments(content: &str) -> Vec<ContentSegment> {
    let mut segments = Vec::new();
    let mut remaining = content;

    loop {
        // Find the next ```chartml opening fence
        let open_match = find_chartml_fence(remaining);

        match open_match {
            Some(open_start) => {
                // Everything before the fence is Markdown
                let before = &remaining[..open_start];
                if !before.trim().is_empty() {
                    segments.push(ContentSegment::Markdown(before.to_string()));
                }

                // Skip past the opening fence line (```chartml + newline)
                let after_open = &remaining[open_start..];
                let fence_end = after_open
                    .find('\n')
                    .map(|i| i + 1)
                    .unwrap_or(after_open.len());
                let inner = &after_open[fence_end..];

                // Find the closing ```
                match find_closing_fence(inner) {
                    Some(close_start) => {
                        let chartml_content = &inner[..close_start];
                        let trimmed = chartml_content.trim();
                        if !trimmed.is_empty() {
                            segments.push(ContentSegment::ChartML(trimmed.to_string()));
                        }

                        // Skip past the closing ``` line
                        let after_close = &inner[close_start..];
                        let close_fence_end = after_close
                            .find('\n')
                            .map(|i| i + 1)
                            .unwrap_or(after_close.len());
                        remaining = &after_close[close_fence_end..];
                    }
                    None => {
                        // No closing fence — treat the rest as ChartML
                        let trimmed = inner.trim();
                        if !trimmed.is_empty() {
                            segments.push(ContentSegment::ChartML(trimmed.to_string()));
                        }
                        break;
                    }
                }
            }
            None => {
                // No more ChartML blocks — rest is Markdown
                if !remaining.trim().is_empty() {
                    segments.push(ContentSegment::Markdown(remaining.to_string()));
                }
                break;
            }
        }
    }

    segments
}

/// Find the byte offset of a ```chartml fence in the given text.
/// The fence must start at the beginning of a line (or the start of text).
fn find_chartml_fence(text: &str) -> Option<usize> {
    let pattern = "```chartml";
    for (idx, _) in text.match_indices(pattern) {
        // Must be at start of text or preceded by a newline
        if idx == 0 || text.as_bytes().get(idx - 1) == Some(&b'\n') {
            return Some(idx);
        }
    }
    None
}

/// Find the byte offset of a closing ``` fence (a line starting with ```
/// that is NOT immediately followed by an alphanumeric char, i.e. not
/// another opening fence like ```python).
fn find_closing_fence(text: &str) -> Option<usize> {
    let pattern = "```";
    for (idx, _) in text.match_indices(pattern) {
        // Must be at start of text or preceded by a newline
        if idx == 0 || text.as_bytes().get(idx - 1) == Some(&b'\n') {
            // Must NOT be followed by an alphanumeric char (that would be an opening fence)
            let after = idx + 3;
            match text.as_bytes().get(after) {
                None | Some(b'\n') | Some(b'\r') | Some(b' ') => return Some(idx),
                _ => continue,
            }
        }
    }
    None
}

/// Convert a markdown string to HTML using pulldown-cmark.
fn markdown_to_html(markdown: &str) -> String {
    let parser = pulldown_cmark::Parser::new(markdown);
    let mut html_output = String::new();
    pulldown_cmark::html::push_html(&mut html_output, parser);
    html_output
}

/// Renders dashboard markdown content, handling embedded ChartML code blocks.
///
/// Markdown segments are converted to HTML via `pulldown-cmark` and rendered
/// with Tailwind prose classes. ChartML segments are rendered as styled code
/// blocks (placeholder — actual chart rendering will be added in Phase 2).
#[component]
pub fn MarkdownRenderer(
    /// The markdown content to render (may contain ```chartml blocks)
    #[prop(into)]
    content: Signal<String>,
) -> impl IntoView {
    let segments = Memo::new(move |_| parse_segments(&content.get()));

    view! {
        <div class="prose prose-sm dark:prose-invert max-w-none">
            <For
                each=move || {
                    segments.get().into_iter().enumerate().collect::<Vec<_>>()
                }
                key=|(i, _)| *i
                children=move |(_, segment)| {
                    match segment {
                        ContentSegment::Markdown(md) => {
                            let html = markdown_to_html(&md);
                            view! {
                                <div inner_html=html></div>
                            }.into_any()
                        }
                        ContentSegment::ChartML(yaml) => {
                            view! {
                                <div class="my-4 rounded-lg border border-border bg-muted/50 p-4">
                                    <div class="text-xs text-muted-foreground mb-2 font-medium">
                                        "ChartML"
                                    </div>
                                    <pre class="text-sm font-mono whitespace-pre-wrap text-foreground">
                                        {yaml}
                                    </pre>
                                </div>
                            }.into_any()
                        }
                    }
                }
            />
        </div>
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_plain_markdown_only() {
        let segments = parse_segments("# Hello\n\nSome text.");
        assert_eq!(segments.len(), 1);
        assert!(matches!(&segments[0], ContentSegment::Markdown(_)));
    }

    #[test]
    fn test_chartml_block_extraction() {
        let input = "# Title\n\n```chartml\ntype: bar\ndata: test\n```\n\nMore text.";
        let segments = parse_segments(input);
        assert_eq!(segments.len(), 3);
        assert!(matches!(&segments[0], ContentSegment::Markdown(_)));
        assert!(matches!(&segments[1], ContentSegment::ChartML(s) if s.contains("type: bar")));
        assert!(matches!(&segments[2], ContentSegment::Markdown(_)));
    }

    #[test]
    fn test_multiple_chartml_blocks() {
        let input = "Text\n```chartml\nchart1\n```\nMiddle\n```chartml\nchart2\n```\nEnd";
        let segments = parse_segments(input);
        assert_eq!(segments.len(), 5);
        assert!(matches!(&segments[0], ContentSegment::Markdown(_)));
        assert!(matches!(&segments[1], ContentSegment::ChartML(s) if s == "chart1"));
        assert!(matches!(&segments[2], ContentSegment::Markdown(_)));
        assert!(matches!(&segments[3], ContentSegment::ChartML(s) if s == "chart2"));
        assert!(matches!(&segments[4], ContentSegment::Markdown(_)));
    }

    #[test]
    fn test_chartml_at_start() {
        let input = "```chartml\nchart_data\n```\nAfter.";
        let segments = parse_segments(input);
        assert_eq!(segments.len(), 2);
        assert!(matches!(&segments[0], ContentSegment::ChartML(s) if s == "chart_data"));
        assert!(matches!(&segments[1], ContentSegment::Markdown(_)));
    }

    #[test]
    fn test_empty_content() {
        let segments = parse_segments("");
        assert!(segments.is_empty());
    }

    #[test]
    fn test_non_chartml_code_blocks_are_markdown() {
        let input = "```python\nprint('hello')\n```";
        let segments = parse_segments(input);
        assert_eq!(segments.len(), 1);
        assert!(matches!(&segments[0], ContentSegment::Markdown(_)));
    }

    #[test]
    fn test_markdown_to_html_basic() {
        let html = markdown_to_html("**bold** and *italic*");
        assert!(html.contains("<strong>bold</strong>"));
        assert!(html.contains("<em>italic</em>"));
    }
}
