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

    let re_header = regex::Regex::new(r"^(#{1,6})\s+(.*)").expect("valid regex");
    let re_table_sep = regex::Regex::new(r"^\|[\s\-:]+\|$").expect("valid regex");
    let re_ul = regex::Regex::new(r"^[-*+]\s+(.*)").expect("valid regex");
    let re_ol = regex::Regex::new(r"^\d+\.\s+(.*)").expect("valid regex");
    let re_hr = regex::Regex::new(r"^[-*_]{3,}$").expect("valid regex");

    for line in &lines {
        let stripped = line.trim();

        // Empty lines — close open lists/tables, emit blank line
        if stripped.is_empty() {
            close_list(&mut output, &mut in_ul, &mut in_ol);
            if in_table {
                emit_table(&mut output, &table_header_cells, &table_rows, table_columns);
                in_table = false;
                table_rows.clear();
                table_header_cells.clear();
                table_header_done = false;
            }
            output.push(String::new());
            continue;
        }

        // Typst passthrough (markers we already inserted as valid Typst)
        if stripped.starts_with("#image(")
            || stripped.starts_with("#block(")
            || stripped.starts_with("#table(")
            || stripped.starts_with("#align(")
            || stripped.starts_with("#v(")
        {
            close_list(&mut output, &mut in_ul, &mut in_ol);
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
            output.push(r##"#line(length: 100%, stroke: 0.5pt + rgb("#e5e7eb"))"##.to_string());
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

    output.join("\n")
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
fn emit_table(
    output: &mut Vec<String>,
    header_cells: &[String],
    body_rows: &[String],
    columns: usize,
) {
    if header_cells.is_empty() {
        return;
    }

    let header_row = header_cells
        .iter()
        .map(|c| format!("[*{}*]", c))
        .collect::<Vec<_>>()
        .join(", ");

    let mut table = format!(
        r##"#table(
  columns: {columns},
  stroke: rgb("#e5e7eb"),
  fill: (_, y) => if calc.odd(y) {{ rgb("#f9fafb") }} else {{ none }},
  table.header({header_row}),
"##
    );

    for row in body_rows {
        table.push_str(&format!("  {row},\n"));
    }
    table.push(')');

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
    let text = text
        .replace(PH_LINK_S, "#link(\"")
        .replace(PH_LINK_M, "\")[")
        .replace(PH_LINK_E, "]");

    text
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
                ("▲", "#059669")
            } else {
                ("▼", "#dc2626")
            };
            format!(
                r##"    #text(10pt, fill: rgb("{color}"))[{arrow} {pct}]"##,
            )
        }
        None => String::new(),
    };

    format!(
        r##"#block(fill: rgb("#f9fafb"), stroke: rgb("#e5e7eb"), radius: 8pt, inset: 16pt, width: 100%, breakable: false)[
  #align(center)[
    #text(11pt, fill: rgb("#6b7280"))[{escaped_title}]
    #v(4pt)
    #text(28pt, weight: "bold")[{escaped_value}]
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

    let header_cells = headers
        .iter()
        .map(|h| format!("[*{}*]", typst_escape(h)))
        .collect::<Vec<_>>()
        .join(", ");

    let mut table = format!(
        r##"#text(12pt, weight: "semibold")[{escaped_title}]
#v(4pt)
#table(
  columns: {columns},
  stroke: rgb("#e5e7eb"),
  fill: (_, y) => if calc.odd(y) {{ rgb("#f9fafb") }} else {{ none }},
  table.header({header_cells}),
"##
    );

    for row in rows {
        let cells = row
            .iter()
            .map(|c| format!("[{}]", typst_escape(c)))
            .collect::<Vec<_>>()
            .join(", ");
        table.push_str(&format!("  {cells},\n"));
    }
    table.push(')');

    if total_rows > max_rows {
        table.push_str(&format!(
            "\n#text(9pt, fill: rgb(\"#6b7280\"))[Showing {} of {} rows]",
            max_rows, total_rows
        ));
    }

    table
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
        assert!(result.contains("columns: 2"));
        assert!(result.contains("[*Name*]"));
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
        assert!(result.contains("#059669"));
    }

    #[test]
    fn metric_card_negative_trend() {
        let result = render_metric_typst("Costs", "$10,000", Some(("5.0%", false)));
        assert!(result.contains("▼ 5.0%"));
        assert!(result.contains("#dc2626"));
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
}
