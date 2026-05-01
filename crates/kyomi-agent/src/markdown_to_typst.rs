// SPDX-License-Identifier: AGPL-3.0-or-later

//! Markdown → Typst markup converter for PDF export.
//!
//! Converts dashboard markdown (with chart image markers already inserted)
//! to Typst markup. Handles the same elements as the HTML converter
//! in `pdf_export.rs`: headings, lists, tables, inline formatting,
//! horizontal rules, and passthrough for image/metric/table markers.

use crate::pdf_typst::typst_escape;

/// Convert a markdown string to Typst markup.
///
/// The input may contain special markers inserted by the chart replacement step:
/// - `<<IMAGE:chart_N.png>>` — chart image reference
/// - `<<METRIC:...>>` — pre-rendered metric Typst block (passed through)
/// - `<<TABLE:...>>` — pre-rendered table Typst block (passed through)
///
/// These markers are passed through as-is since they're already valid Typst.
pub fn markdown_to_typst(markdown_text: &str) -> String {
    let lines: Vec<&str> = markdown_text.split('\n').collect();
    let mut output: Vec<String> = Vec::new();
    let mut in_ul = false;
    let mut in_ol = false;
    let mut in_table = false;
    let mut table_columns: usize = 0;
    let mut table_header_done = false;
    let mut table_rows: Vec<String> = Vec::new();
    let mut table_header_cells: Vec<String> = Vec::new();
    // Multi-line blockquote collector: consecutive `> ...` lines are grouped
    // into one `#quote(block: true)[ ... ]` call so the `#show quote:` rule
    // in pdf_typst::wrap_document() styles them as one editorial pull-quote.
    let mut blockquote_lines: Vec<String> = Vec::new();

    let re_header = regex::Regex::new(r"^(#{1,6})\s+(.*)").expect("valid regex");
    let re_table_sep = regex::Regex::new(r"^\|[\s\-:]+\|$").expect("valid regex");
    let re_ul = regex::Regex::new(r"^[-*+]\s+(.*)").expect("valid regex");
    let re_ol = regex::Regex::new(r"^\d+\.\s+(.*)").expect("valid regex");
    let re_hr = regex::Regex::new(r"^[-*_]{3,}$").expect("valid regex");
    let re_blockquote = regex::Regex::new(r"^>\s?(.*)").expect("valid regex");

    // Track bracket depth for multiline Typst passthrough blocks.
    // When we enter a Typst block (e.g. #block(...)[...]), we pass through
    // all lines verbatim until brackets are balanced.
    let mut typst_block_depth: i32 = 0;

    for line in &lines {
        let stripped = line.trim();

        // If inside a multiline Typst block, pass through until balanced
        if typst_block_depth > 0 {
            typst_block_depth += count_bracket_depth(stripped);
            output.push(stripped.to_string());
            continue;
        }

        // Empty lines — close open lists/tables/blockquotes, emit blank line
        if stripped.is_empty() {
            close_list(&mut output, &mut in_ul, &mut in_ol);
            if in_table {
                emit_table(&mut output, &table_header_cells, &table_rows, table_columns);
                in_table = false;
                table_rows.clear();
                table_header_cells.clear();
                table_header_done = false;
            }
            if !blockquote_lines.is_empty() {
                emit_blockquote(&mut output, &blockquote_lines);
                blockquote_lines.clear();
            }
            output.push(String::new());
            continue;
        }

        // Blockquote lines (`> text`) — collect contiguously, emit on next non-blockquote.
        if let Some(caps) = re_blockquote.captures(stripped) {
            close_list(&mut output, &mut in_ul, &mut in_ol);
            blockquote_lines.push(caps[1].to_string());
            continue;
        }
        if !blockquote_lines.is_empty() {
            emit_blockquote(&mut output, &blockquote_lines);
            blockquote_lines.clear();
        }

        // Typst passthrough (pre-rendered Typst blocks from metric/table/chart/grid renderers).
        // `#figure(...)` is the editorial chart-emission shape used by
        // `pdf_export.rs` — it must pass through so the `#show figure` rule in
        // `pdf_typst::wrap_document()` can apply the editorial top/bottom rule
        // treatment with the "FIGURE N" eyebrow label.
        if stripped.starts_with("#image(")
            || stripped.starts_with("#block(")
            || stripped.starts_with("#table(")
            || stripped.starts_with("#figure(")
            || stripped.starts_with("#align(")
            || stripped.starts_with("#grid(")
            || stripped.starts_with("#v(")
            || stripped.starts_with("#text(")
        {
            close_list(&mut output, &mut in_ul, &mut in_ol);
            // Count brackets to track multiline blocks
            typst_block_depth += count_bracket_depth(stripped);
            output.push(stripped.to_string());
            continue;
        }

        // Headers
        if let Some(caps) = re_header.captures(stripped) {
            close_list(&mut output, &mut in_ul, &mut in_ol);
            let level = caps[1].len();
            let text = inline_formatting(&caps[2]);
            let equals = "=".repeat(level);
            output.push(format!("{equals} {text}"));
            continue;
        }

        // Table rows
        if stripped.contains('|') && stripped.starts_with('|') {
            let no_spaces = stripped.replace(' ', "");
            if re_table_sep.is_match(&no_spaces) {
                continue; // Skip separator row
            }

            let cells: Vec<&str> = stripped
                .trim_matches('|')
                .split('|')
                .map(|c| c.trim())
                .collect();

            if !in_table {
                in_table = true;
                table_header_done = false;
                table_columns = cells.len();
                table_header_cells = cells.iter().map(|c| inline_formatting(c)).collect();
                continue;
            }

            if !table_header_done {
                table_header_done = true;
            }

            // Collect body row cells
            let row_cells: Vec<String> = cells.iter().map(|c| inline_formatting(c)).collect();
            table_rows.push(
                row_cells
                    .iter()
                    .map(|c| format!("[{}]", c))
                    .collect::<Vec<_>>()
                    .join(", "),
            );
            continue;
        }

        // Close table if we're no longer in one
        if in_table && !stripped.starts_with('|') {
            emit_table(&mut output, &table_header_cells, &table_rows, table_columns);
            in_table = false;
            table_rows.clear();
            table_header_cells.clear();
            table_header_done = false;
        }

        // Unordered list
        if let Some(caps) = re_ul.captures(stripped) {
            in_ul = true;
            output.push(format!("- {}", inline_formatting(&caps[1])));
            continue;
        }

        // Ordered list
        if let Some(caps) = re_ol.captures(stripped) {
            in_ol = true;
            output.push(format!("+ {}", inline_formatting(&caps[1])));
            continue;
        }

        // Close lists if non-list line
        close_list(&mut output, &mut in_ul, &mut in_ol);

        // Horizontal rule
        if re_hr.is_match(stripped) {
            output.push(r##"#line(length: 100%, stroke: 0.5pt + rgb("#E8E5DE"))"##.to_string());
            continue;
        }

        // Regular paragraph
        output.push(inline_formatting(stripped));
    }

    // Close any remaining open elements
    close_list(&mut output, &mut in_ul, &mut in_ol);
    if in_table {
        emit_table(&mut output, &table_header_cells, &table_rows, table_columns);
    }
    if !blockquote_lines.is_empty() {
        emit_blockquote(&mut output, &blockquote_lines);
    }

    output.join("\n")
}

/// Emit a collected run of blockquote lines as a single `#quote(block: true)[...]`
/// call. The `#show quote:` rule in `pdf_typst::wrap_document()` styles this with
/// a 2pt amber left border and italic Instrument Serif, matching the viewer's
/// `.prose-kyomi blockquote`.
fn emit_blockquote(output: &mut Vec<String>, lines: &[String]) {
    if lines.is_empty() {
        return;
    }
    let body = lines
        .iter()
        .map(|l| inline_formatting(l))
        .collect::<Vec<_>>()
        .join(" \\\n");
    output.push(format!("#quote(block: true)[{body}]"));
}

/// Close any open list context.
fn close_list(output: &mut Vec<String>, in_ul: &mut bool, in_ol: &mut bool) {
    if *in_ul {
        output.push(String::new()); // Blank line to end list
        *in_ul = false;
    }
    if *in_ol {
        output.push(String::new());
        *in_ol = false;
    }
}

/// Emit a Typst table from collected header and body rows.
/// Styled to match .kyomi-markdown table CSS: muted header, horizontal dividers, white rows.
fn emit_table(
    output: &mut Vec<String>,
    header_cells: &[String],
    body_rows: &[String],
    columns: usize,
) {
    if header_cells.is_empty() {
        return;
    }

    // Header cells: uppercase, small, muted-foreground — warm design system colors
    let header_row = header_cells
        .iter()
        .map(|c| format!(r##"[#text(7pt, weight: "medium", fill: rgb("#6B6660"), tracking: 0.08em, font: "DM Sans")[#upper[{}]]]"##, c))
        .collect::<Vec<_>>()
        .join(", ");

    // Editorial ledger styling: 1.5pt dark #1C1917 top+bottom, thin 0.5pt
    // #E8E5DE between rows, uppercase tracked muted header. No zebra fill.
    // Matches `.prose-kyomi table` rules in main.css.
    let mut table = format!(
        r##"#table(
  columns: (1fr,) * {columns},
  stroke: none,
  inset: (x: 12pt, y: 10pt),
  table.hline(stroke: 1.5pt + rgb("#1C1917")),
  table.header({header_row}),
  table.hline(stroke: 1.5pt + rgb("#1C1917")),
"##
    );

    for (i, row) in body_rows.iter().enumerate() {
        table.push_str(&format!("  {row},\n"));
        // Thin divider between body rows, omitted after the final row
        // (the bottom edge of the table gets a 1.5pt rule below instead).
        if i + 1 < body_rows.len() {
            let y = i + 2; // +1 for header, +1 because we want the line below this row
            table.push_str(&format!(
                r##"  table.hline(y: {y}, stroke: 0.5pt + rgb("#E8E5DE")),
"##,
            ));
        }
    }
    table.push_str(
        r##"  table.hline(stroke: 1.5pt + rgb("#1C1917")),
)"##,
    );

    output.push(table);
}

/// Apply inline markdown formatting, converting to Typst markup.
fn inline_formatting(text: &str) -> String {
    let re_bold_star = regex::Regex::new(r"\*\*(.+?)\*\*").expect("valid regex");
    let re_bold_under = regex::Regex::new(r"__(.+?)__").expect("valid regex");
    let re_italic_star = regex::Regex::new(r"\*(.+?)\*").expect("valid regex");
    let re_italic_under =
        regex::Regex::new(r"(?:^|(\W))_(.+?)_(?:\W|$)").expect("valid regex");
    let re_code = regex::Regex::new(r"`(.+?)`").expect("valid regex");
    let re_link = regex::Regex::new(r"\[(.+?)\]\((.+?)\)").expect("valid regex");

    // Use Unicode private use area characters as placeholders.
    // These won't appear in normal text and won't be escaped by typst_escape.
    const PH_BOLD_S: &str = "\u{E001}";
    const PH_BOLD_E: &str = "\u{E002}";
    const PH_ITALIC_S: &str = "\u{E003}";
    const PH_ITALIC_E: &str = "\u{E004}";
    const PH_CODE_S: &str = "\u{E005}";
    const PH_CODE_E: &str = "\u{E006}";
    const PH_LINK_S: &str = "\u{E007}";
    const PH_LINK_M: &str = "\u{E008}";
    const PH_LINK_E: &str = "\u{E009}";

    // Replace markdown syntax with placeholders (preserving content)
    let bold_s = format!("{}$1{}", PH_BOLD_S, PH_BOLD_E);
    let text = re_bold_star.replace_all(text, bold_s.as_str());
    let text = re_bold_under.replace_all(&text, bold_s.as_str());
    let italic_s = format!("{}$1{}", PH_ITALIC_S, PH_ITALIC_E);
    let text = re_italic_star.replace_all(&text, italic_s.as_str());
    let italic_u = format!("$1{}$2{}", PH_ITALIC_S, PH_ITALIC_E);
    let text = re_italic_under.replace_all(&text, italic_u.as_str());
    let code_s = format!("{}$1{}", PH_CODE_S, PH_CODE_E);
    let text = re_code.replace_all(&text, code_s.as_str());
    let link_s = format!("{}$2{}$1{}", PH_LINK_S, PH_LINK_M, PH_LINK_E);
    let text = re_link.replace_all(&text, link_s.as_str());

    // Now escape the remaining text (placeholders pass through untouched)
    let text = typst_escape(&text);

    // Restore placeholders with Typst syntax
    let text = text.replace(PH_BOLD_S, "*").replace(PH_BOLD_E, "*");
    let text = text.replace(PH_ITALIC_S, "_").replace(PH_ITALIC_E, "_");
    let text = text.replace(PH_CODE_S, "`").replace(PH_CODE_E, "`");
    text
        .replace(PH_LINK_S, "#link(\"")
        .replace(PH_LINK_M, "\")[")
        .replace(PH_LINK_E, "]")
}

/// Render a metric card as Typst markup.
///
/// Produces a styled block with label, value, and optional trend indicator.
pub fn render_metric_typst(
    title: &str,
    value: &str,
    trend: Option<(&str, bool)>, // (percentage_text, is_positive)
) -> String {
    let escaped_title = typst_escape(title);
    let escaped_value = typst_escape(value);

    let trend_line = match trend {
        Some((pct, positive)) => {
            let (arrow, color) = if positive {
                ("▲", "#15803D") // --success
            } else {
                ("▼", "#DC2626") // --error
            };
            format!(
                r##"
    #v(2pt)
    #text(10pt, fill: rgb("{color}"), font: "DM Sans")[{arrow} {pct}]"##,
            )
        }
        None => String::new(),
    };

    format!(
        r##"#block(fill: rgb("#F5F3EF"), stroke: rgb("#E8E5DE"), radius: 6pt, inset: (x: 12pt, y: 10pt), width: 100%, breakable: false)[
  #align(center)[
    #text(9pt, fill: rgb("#6B6660"), font: "DM Sans", weight: "medium")[{escaped_title}]
    #v(3pt)
    #text(24pt, weight: "bold", font: "Geist Mono")[{escaped_value}]
{trend_line}
  ]
]"##
    )
}

/// Render a data table as Typst markup.
///
/// Produces a styled table with headers, striped rows, and optional truncation notice.
pub fn render_data_table_typst(
    title: &str,
    headers: &[String],
    rows: &[Vec<String>],
    total_rows: usize,
    max_rows: usize,
) -> String {
    let escaped_title = typst_escape(title);
    let columns = headers.len();

    // Header cells: uppercase, small, warm muted — design system colors
    let header_cells = headers
        .iter()
        .map(|h| format!(
            r##"[#text(7pt, weight: "medium", fill: rgb("#6B6660"), tracking: 0.08em, font: "DM Sans")[#upper[{}]]]"##,
            typst_escape(h)
        ))
        .collect::<Vec<_>>()
        .join(", ");

    let mut table = format!(
        r##"#text(12pt, weight: "semibold", fill: rgb("#1C1917"), font: "DM Sans")[{escaped_title}]
#v(6pt)
#table(
  columns: (1fr,) * {columns},
  stroke: none,
  inset: (x: 12pt, y: 9pt),
  fill: (_, y) => if y == 0 {{ rgb("#F5F3EF") }} else {{ white }},
  table.hline(stroke: 0.5pt + rgb("#E8E5DE")),
  table.header({header_cells}),
  table.hline(stroke: 0.5pt + rgb("#E8E5DE")),
"##
    );

    for (i, row) in rows.iter().enumerate() {
        let cells = row
            .iter()
            .map(|c| {
                let font = if is_numeric_cell(c) { "Geist Mono" } else { "DM Sans" };
                format!("[#text(9pt, fill: rgb(\"#1C1917\"), font: \"{font}\")[{}]]", typst_escape(c))
            })
            .collect::<Vec<_>>()
            .join(", ");
        table.push_str(&format!("  {cells},\n"));
        let y = i + 1;
        table.push_str(&format!(
            r##"  table.hline(y: {}, stroke: 0.5pt + rgb("#E8E5DE")),
"##,
            y + 1
        ));
    }
    table.push(')');

    if total_rows > max_rows {
        table.push_str(&format!(
            "\n#v(4pt)\n#text(8pt, fill: rgb(\"#9C9790\"), font: \"DM Sans\")[Showing {} of {} rows]",
            max_rows, total_rows
        ));
    }

    table
}

/// Count the net bracket depth change for a line of Typst source.
///
/// Used to track multi-line Typst blocks during passthrough. Counts `[`, `(`
/// as +1 and `]`, `)` as -1, but skips any character preceded by a backslash
/// (`\`) so that Typst escape sequences like `\[`, `\]`, `\(`, `\)` (produced
/// by `typst_escape` for user content) don't distort the balance calculation.
fn count_bracket_depth(s: &str) -> i32 {
    let mut depth: i32 = 0;
    let mut prev_was_backslash = false;
    for ch in s.chars() {
        if prev_was_backslash {
            // This char is escaped — skip it for bracket counting
            prev_was_backslash = false;
            continue;
        }
        match ch {
            '\\' => prev_was_backslash = true,
            '[' | '(' => depth += 1,
            ']' | ')' => depth -= 1,
            _ => {}
        }
    }
    depth
}

/// Detect whether a cell value looks numeric (for Geist Mono font selection).
///
/// Matches currency values ($620K, €1.2M), percentages (+15%, -3.1%),
/// plain numbers (12,847), and signed values. Text labels get DM Sans.
fn is_numeric_cell(value: &str) -> bool {
    let v = value.trim();
    if v.is_empty() {
        return false;
    }
    // Strip leading sign and currency symbols
    let v = v.trim_start_matches(['+', '-', '(']);
    let v = v.trim_start_matches(['$', '€', '£', '¥']);
    // Must start with a digit after stripping
    let Some(first) = v.chars().next() else {
        return false;
    };
    first.is_ascii_digit()
        && v.chars()
            .all(|c| c.is_ascii_digit() || matches!(c, '.' | ',' | '%' | ')' | 'K' | 'M' | 'B' | 'T' | ' '))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn headers() {
        let result = markdown_to_typst("# H1\n## H2\n### H3");
        assert!(result.contains("= H1"));
        assert!(result.contains("== H2"));
        assert!(result.contains("=== H3"));
    }

    #[test]
    fn unordered_list() {
        let result = markdown_to_typst("- Item 1\n- Item 2");
        assert!(result.contains("- Item 1"));
        assert!(result.contains("- Item 2"));
    }

    #[test]
    fn ordered_list() {
        let result = markdown_to_typst("1. First\n2. Second");
        assert!(result.contains("+ First"));
        assert!(result.contains("+ Second"));
    }

    #[test]
    fn horizontal_rule() {
        let result = markdown_to_typst("---");
        assert!(result.contains("#line("));
    }

    #[test]
    fn inline_bold() {
        let result = inline_formatting("this is **bold** text");
        assert!(result.contains("*bold*"));
    }

    #[test]
    fn inline_italic() {
        let result = inline_formatting("this is *italic* text");
        assert!(result.contains("_italic_"));
    }

    #[test]
    fn inline_code() {
        let result = inline_formatting("use `code` here");
        assert!(result.contains("`code`"));
    }

    #[test]
    fn inline_link() {
        let result = inline_formatting("[click here](http://example.com)");
        assert!(result.contains("#link(\"http://example.com\")[click here]"));
    }

    #[test]
    fn table() {
        let md = "| Name | Value |\n|------|-------|\n| A | 1 |\n| B | 2 |";
        let result = markdown_to_typst(md);
        assert!(result.contains("#table("));
        assert!(result.contains("(1fr,) * 2"), "columns spec missing: {result}");
        // Header cells are uppercase with warm design system styling
        assert!(result.contains("#upper[Name]"), "header missing: {result}");
        assert!(result.contains("[A]"));
        assert!(result.contains("[B]"));
    }

    #[test]
    fn typst_passthrough() {
        let result = markdown_to_typst("#image(\"chart_0.png\", width: 100%)");
        assert!(result.contains("#image(\"chart_0.png\", width: 100%)"));
    }

    #[test]
    fn metric_card() {
        let result = render_metric_typst("Revenue", "$42,000", Some(("20.0%", true)));
        assert!(result.contains("Revenue"));
        assert!(result.contains("\\$42,000"));
        assert!(result.contains("▲ 20.0%"));
        assert!(result.contains("#15803D"), "success color missing: {result}");
    }

    #[test]
    fn metric_card_negative_trend() {
        let result = render_metric_typst("Costs", "$10,000", Some(("5.0%", false)));
        assert!(result.contains("▼ 5.0%"));
        assert!(result.contains("#DC2626"), "error color missing: {result}");
    }

    #[test]
    fn data_table_with_truncation() {
        let headers = vec!["Name".to_string(), "Value".to_string()];
        let rows = vec![vec!["A".to_string(), "1".to_string()]];
        let result = render_data_table_typst("My Table", &headers, &rows, 100, 50);
        assert!(result.contains("#table("));
        assert!(result.contains("Showing 50 of 100 rows"));
    }

    #[test]
    fn special_chars_escaped_in_paragraph() {
        let result = markdown_to_typst("Revenue is $100 & growing");
        assert!(result.contains("\\$100"));
    }

    #[test]
    fn numeric_cell_detection() {
        // Numeric values → Geist Mono
        assert!(is_numeric_cell("$620K"));
        assert!(is_numeric_cell("+15%"));
        assert!(is_numeric_cell("-3.1%"));
        assert!(is_numeric_cell("12,847"));
        assert!(is_numeric_cell("$1.45M"));
        assert!(is_numeric_cell("100"));
        // Text labels → DM Sans
        assert!(!is_numeric_cell("North America"));
        assert!(!is_numeric_cell("Revenue"));
        assert!(!is_numeric_cell(""));
        assert!(!is_numeric_cell("Q3 2026"));
    }

    #[test]
    fn bracket_depth_basic() {
        assert_eq!(count_bracket_depth("#block(fill: rgb(\"#F5F3EF\"))["), 1);
        assert_eq!(count_bracket_depth("]"), -1);
        assert_eq!(count_bracket_depth("  #align(center)[text]"), 0);
    }

    #[test]
    fn bracket_depth_skips_escaped_brackets() {
        // \[ and \] from typst_escape should not count toward depth
        assert_eq!(count_bracket_depth(r"text with \[escaped\] brackets"), 0);
        assert_eq!(count_bracket_depth(r"#text(...)[Chart: \[2026\]]"), 0);
        // Unescaped brackets still count
        assert_eq!(count_bracket_depth("[open"), 1);
        assert_eq!(count_bracket_depth("close]"), -1);
    }

    #[test]
    fn passthrough_with_special_chars_in_chart_title() {
        // Chart unavailable placeholder with backticks and brackets in the title
        // must not leave the Typst passthrough in an incorrect state so that
        // any surrounding markdown is parsed correctly afterward.
        let placeholder = concat!(
            "#block(fill: rgb(\"#F5F3EF\"), stroke: rgb(\"#E8E5DE\"), radius: 6pt, inset: 24pt, width: 100%)[",
            "\n  #align(center)[#text(fill: rgb(\"#6B6660\"), style: \"italic\", font: \"DM Sans\")",
            "[Chart unavailable: Revenue from \\`orders\\` table with \\[brackets\\]]]",
            "\n]"
        );
        let markdown = format!("## Heading\n\n{placeholder}\n\nAfter paragraph.");
        let result = markdown_to_typst(&markdown);
        // The heading before the block must be converted (not treated as passthrough)
        assert!(result.contains("== Heading"), "heading missing from result: {result}");
        // The paragraph after the block must also be converted correctly
        assert!(result.contains("After paragraph"), "paragraph after block missing: {result}");
        // The placeholder block must be emitted verbatim
        assert!(result.contains("Chart unavailable"), "placeholder missing: {result}");
    }
}
