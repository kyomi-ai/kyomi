// SPDX-License-Identifier: AGPL-3.0-or-later

//! Typst-based PDF generation.
//!
//! Compiles a Typst document string (with image references) to PDF bytes.
//! Uses `typst-as-lib` for compilation and `typst-pdf` for export.

use std::collections::HashMap;
use typst::layout::PagedDocument;
use typst_as_lib::{typst_kit_options::TypstKitFontOptions, TypstEngine};

// ---------------------------------------------------------------------------
// Compile-time embedded assets — fonts + logo
// ---------------------------------------------------------------------------

/// The Kyomi full logo SVG (starburst icon + geometric lettering), dark variant.
const KYOMI_LOGO_SVG: &[u8] = include_bytes!("../../kyomi-ui/public/kyomi_full_logo.svg");

// Design system fonts embedded at compile time so PDFs render identically
// regardless of the host environment (Docker, k8s, NAS, dev machine).
static FONT_DM_SANS_REGULAR: &[u8] = include_bytes!("../fonts/DMSans-Regular.ttf");
static FONT_DM_SANS_MEDIUM: &[u8] = include_bytes!("../fonts/DMSans-Medium.ttf");
static FONT_DM_SANS_SEMIBOLD: &[u8] = include_bytes!("../fonts/DMSans-SemiBold.ttf");
static FONT_DM_SANS_BOLD: &[u8] = include_bytes!("../fonts/DMSans-Bold.ttf");
static FONT_DM_SANS_ITALIC: &[u8] = include_bytes!("../fonts/DMSans-Italic.ttf");
static FONT_INSTRUMENT_SERIF_REGULAR: &[u8] = include_bytes!("../fonts/InstrumentSerif-Regular.ttf");
static FONT_INSTRUMENT_SERIF_ITALIC: &[u8] = include_bytes!("../fonts/InstrumentSerif-Italic.ttf");
static FONT_GEIST_MONO_REGULAR: &[u8] = include_bytes!("../fonts/GeistMono-Regular.ttf");
static FONT_GEIST_MONO_MEDIUM: &[u8] = include_bytes!("../fonts/GeistMono-Medium.ttf");

/// All bundled font bytes for the Typst engine (static slice references).
fn bundled_fonts() -> Vec<&'static [u8]> {
    vec![
        FONT_DM_SANS_REGULAR, FONT_DM_SANS_MEDIUM, FONT_DM_SANS_SEMIBOLD,
        FONT_DM_SANS_BOLD, FONT_DM_SANS_ITALIC,
        FONT_INSTRUMENT_SERIF_REGULAR, FONT_INSTRUMENT_SERIF_ITALIC,
        FONT_GEIST_MONO_REGULAR, FONT_GEIST_MONO_MEDIUM,
    ]
}

/// All bundled font bytes as owned `Vec<u8>` — the form
/// `chartml_render::init_font_database` requires. The kyomi-agent
/// module uses this to seed chartml-render's font database with the
/// same fonts Typst uses, so chart PNGs embedded in PDFs render
/// `Instrument Serif`, `DM Sans`, and `Geist Mono` correctly. Without
/// this, resvg would silently drop text elements whose `font-family`
/// names aren't in the system font database.
pub fn bundled_fonts_owned() -> Vec<Vec<u8>> {
    bundled_fonts().into_iter().map(|b| b.to_vec()).collect()
}

/// Generate a PDF from a Typst document string and a set of named images.
///
/// Images are referenced in the Typst source as `#image("chart_0.png")` etc.
/// The `images` map provides the PNG bytes for each filename.
///
/// Returns raw PDF bytes.
pub fn generate_pdf(
    typst_source: &str,
    images: &HashMap<String, Vec<u8>>,
) -> Result<Vec<u8>, String> {
    // Build image file entries for the static file resolver.
    // Always include the Kyomi logo SVG for the page header.
    let mut image_entries: Vec<(&str, &[u8])> = images
        .iter()
        .map(|(name, bytes)| (name.as_str(), bytes.as_slice()))
        .collect();
    image_entries.push(("kyomi_logo.svg", KYOMI_LOGO_SVG));

    // Include embedded typst-assets fonts as fallback for symbols (▲, ▼, etc.)
    // but not system fonts (ensures identical rendering everywhere).
    let font_options = TypstKitFontOptions::new()
        .include_system_fonts(false)
        .include_embedded_fonts(true);

    let engine = TypstEngine::builder()
        .main_file(typst_source)
        .search_fonts_with(font_options)
        .fonts(bundled_fonts())
        .with_static_file_resolver(image_entries)
        .build();

    // Compile the document
    let warned = engine.compile::<PagedDocument>();
    let doc = warned.output.map_err(|e| format!("Typst compilation failed: {e}"))?;

    // Log any warnings (non-fatal)
    for warning in &warned.warnings {
        tracing::warn!(message = %warning.message, "Typst warning");
    }

    // Export to PDF
    let options = typst_pdf::PdfOptions::default();
    let pdf_bytes = typst_pdf::pdf(&doc, &options)
        .map_err(|errors| {
            let msgs: Vec<String> = errors.iter().map(|e| format!("{:?}", e)).collect();
            format!("PDF export failed: {}", msgs.join("; "))
        })?;

    Ok(pdf_bytes)
}

/// Build a complete Typst document with page setup, styling, and body content.
///
/// Uses the Kyomi design system: Instrument Serif for headings, DM Sans for
/// body text, Geist Mono for data, warm grays, and amber (#D97706) accent.
/// Includes a branded header with logo and amber rule, and a branded footer.
///
/// Editorial prose treatment mirrors `.prose-kyomi` in
/// `crates/kyomi-ui/style/main.css` and the `.wysiwyg-container` CSS variable
/// block driving the WYSIWYG editor — viewer and PDF render the same content
/// with the same typography, so the three surfaces (viewer, editor, PDF)
/// stay consistent. If you change an h2 rule here, also change it in main.css.
///
/// No cover page variant. The branded page header with the dashboard title
/// appears on every page including the first.
pub fn wrap_document(title: &str, body: &str) -> String {
    wrap_document_inner(title, body, None)
}

/// Build a complete Typst document with an optional cover page preceding
/// the content. Use this for long editorial exports where a title page
/// gives the document a "finished publication" feel. Pass `None` as the
/// date to suppress the date line in the cover eyebrow.
pub fn wrap_document_with_cover(title: &str, body: &str, cover_date: Option<&str>) -> String {
    wrap_document_inner(title, body, Some(cover_date.unwrap_or("")))
}

fn wrap_document_inner(title: &str, body: &str, cover_date: Option<&str>) -> String {
    let escaped_title = typst_escape(title);

    // Cover page block — emitted inline before the content when present.
    // Renders the dashboard title in display-size Instrument Serif with a
    // small uppercase tracked eyebrow above, then a pagebreak. The branded
    // header still appears on the cover page because we don't touch per-
    // page chrome (simpler and keeps the document consistent).
    let cover_block = match cover_date {
        Some(date) => {
            let escaped_date = typst_escape(date);
            let eyebrow = if escaped_date.is_empty() {
                "Kyomi \\u{00B7} Dashboard".to_string()
            } else {
                format!("Kyomi \\u{{00B7}} Dashboard \\u{{00B7}} {escaped_date}")
            };
            format!(
                r##"#v(4cm)
#align(left)[
  #text(8pt, weight: "medium", tracking: 0.12em, fill: rgb("#6B6660"), font: "DM Sans")[
    #upper[{eyebrow}]
  ]
  #v(1.2cm)
  #text(48pt, weight: "regular", fill: rgb("#1C1917"), font: "Instrument Serif")[
    {escaped_title}
  ]
]
#pagebreak()

"##
            )
        }
        None => String::new(),
    };

    format!(
        r##"// -- Kyomi Design System PDF Template --
// Fonts: Instrument Serif (display), DM Sans (body), Geist Mono (data/code)
// Colors: warm grays, amber accent (#D97706)
// Editorial parity with `.prose-kyomi` in crates/kyomi-ui/style/main.css.

#set page(
  paper: "a4",
  margin: (top: 3cm, bottom: 2.5cm, left: 2.5cm, right: 2.5cm),
  header: context {{
    set text(font: "DM Sans")
    grid(
      columns: (auto, 1fr),
      align: (left + horizon, right + horizon),
      column-gutter: 8pt,
      [#image("kyomi_logo.svg", height: 28pt)],
      [#text(9pt, fill: rgb("#9C9790"))[{escaped_title}]],
    )
    v(4pt)
    line(length: 100%, stroke: 1pt + rgb("#D97706"))
  }},
  header-ascent: 15%,
  footer: context {{
    line(length: 100%, stroke: 0.5pt + rgb("#E8E5DE"))
    v(4pt)
    set text(8pt, fill: rgb("#9C9790"), font: "DM Sans")
    grid(
      columns: (1fr, 1fr),
      align: (left, right),
      [Generated by kyomi.ai],
      [Page #counter(page).display("1 of 1", both: true)],
    )
  }},
  footer-descent: 20%,
)

// Body defaults — editorial density with DM Sans 10.5pt. Leading
// 0.62em / spacing 0.9em — a touch more breathing room inside
// paragraphs than the pre-editorial 0.55em leading, while keeping
// the same inter-block spacing so titles still sit tight to their
// following paragraphs. Still denser than web prose (~0.7em leading
// at 1.7 line-height).
#set text(10.5pt, font: "DM Sans", fill: rgb("#1C1917"))
#set par(leading: 0.62em, spacing: 0.9em)

// Strong/bold uses weight 600 per .prose-kyomi (Tailwind default is 700).
#show strong: set text(weight: "semibold")

// Links in amber accent.
#show link: set text(fill: rgb("#D97706"))

// ── Headings ──────────────────────────────────────────────────────
// h1-h3 use Instrument Serif regular, h4-h6 use DM Sans.
// Italic emphasis inside h1/h2/h3 is amber (editorial "one-word pop"
// pattern). h2 has a ruled bottom separator. h5/h6 are uppercase
// tracked metadata labels.
#set heading(numbering: none)

#show heading.where(level: 1): it => block(below: 16pt)[
  #set text(32pt, weight: "regular", fill: rgb("#1C1917"), font: "Instrument Serif")
  #set par(leading: 0.35em)
  #show emph: set text(fill: rgb("#D97706"), style: "italic")
  #it.body
]

#show heading.where(level: 2): it => block(above: 28pt, below: 10pt)[
  #set text(22pt, weight: "regular", fill: rgb("#1C1917"), font: "Instrument Serif")
  #set par(leading: 0.35em)
  #show emph: set text(fill: rgb("#D97706"), style: "italic")
  #it.body
  #v(6pt)
  #line(length: 100%, stroke: 0.5pt + rgb("#E8E5DE"))
]

#show heading.where(level: 3): it => block(above: 20pt, below: 8pt)[
  #set text(16pt, weight: "regular", fill: rgb("#1C1917"), font: "Instrument Serif")
  #set par(leading: 0.35em)
  #show emph: set text(fill: rgb("#D97706"), style: "italic")
  #it.body
]

#show heading.where(level: 4): it => block(above: 16pt, below: 6pt)[
  #set text(13pt, weight: "semibold", fill: rgb("#1C1917"), font: "DM Sans")
  #it.body
]

#show heading.where(level: 5): it => block(above: 14pt, below: 4pt)[
  #set text(8pt, weight: "medium", fill: rgb("#6B6660"), font: "DM Sans", tracking: 0.08em)
  #upper[#it.body]
]

#show heading.where(level: 6): it => block(above: 14pt, below: 4pt)[
  #set text(8pt, weight: "medium", fill: rgb("#6B6660"), font: "DM Sans", tracking: 0.08em)
  #upper[#it.body]
]

// ── Lists ────────────────────────────────────────────────────────
// Amber bullet markers; ordered-list numerals in Geist Mono so
// numbers in nested lists align like a ledger.
#set list(marker: ([#text(fill: rgb("#D97706"))[•]], [#text(fill: rgb("#D97706"))[◦]]))
#show enum.item: it => {{
  set text(font: "DM Sans")
  it
}}

// ── Blockquote ──────────────────────────────────────────────────
// Typst renders markdown `> quoted` as the `quote` function, which
// we style with a 2pt amber left border + italic Instrument Serif
// 14pt, matching the viewer's blockquote treatment.
#show quote: it => block(
  stroke: (left: 2pt + rgb("#D97706")),
  inset: (left: 14pt, top: 4pt, bottom: 4pt),
  spacing: 1em,
)[
  #set text(14pt, style: "italic", fill: rgb("#1C1917"), font: "Instrument Serif")
  #it.body
]

// ── Code — inline and block ─────────────────────────────────────
// Inline code: Geist Mono on warm #F5F3EF background, pill padding.
// Block code: Geist Mono 9.5pt on warm #F5F3EF with a 2pt left rule
// in #D4D0C8, matching the viewer's editorial code sample aesthetic.
#show raw.where(block: false): it => box(
  fill: rgb("#F5F3EF"),
  inset: (x: 4pt, y: 1pt),
  outset: (y: 3pt),
  radius: 2pt,
)[
  #set text(font: "Geist Mono", size: 0.9em, fill: rgb("#1C1917"))
  #it
]

#show raw.where(block: true): it => block(
  fill: rgb("#F5F3EF"),
  stroke: (left: 2pt + rgb("#D4D0C8")),
  inset: (x: 14pt, y: 12pt),
  spacing: 1em,
  width: 100%,
)[
  #set text(font: "Geist Mono", size: 9.5pt, fill: rgb("#1C1917"))
  #it
]

// ── Figures ──────────────────────────────────────────────────────
// Chart images wrapped in `#figure(...)` render with the signature
// editorial top+bottom rule treatment and a "FIGURE N" eyebrow above.
// The caption, if present, is italic Instrument Serif centered below.
// This is the distinctive move from the variants reference site.
#set figure(numbering: "1")
#show figure: it => block(above: 18pt, below: 18pt)[
  #line(length: 100%, stroke: 1pt + rgb("#1C1917"))
  #v(4pt)
  #text(7pt, weight: "medium", fill: rgb("#6B6660"), font: "DM Sans", tracking: 0.12em)[
    #upper[Figure #context counter(figure).display()]
  ]
  #v(6pt)
  #it.body
  #v(6pt)
  #line(length: 100%, stroke: 1pt + rgb("#1C1917"))
  #if it.caption != none [
    #v(6pt)
    #align(center)[
      #text(9pt, style: "italic", fill: rgb("#6B6660"), font: "Instrument Serif")[
        // `.body` extracts the raw caption text without Typst's
        // auto-generated "Figure N." supplement prefix — we already
        // emit the "FIGURE N" eyebrow manually above the figure body.
        #it.caption.body
      ]
    ]
  ]
]

{cover_block}{body}
"##
    )
}

/// Escape special Typst characters in text content.
///
/// Typst uses `*`, `_`, `#`, etc. as markup. We need to escape them
/// when inserting raw text that should be displayed literally.
///
/// Characters escaped:
/// - `` ` `` — starts a raw span; unescaped backtick without a closing pair causes
///   "unclosed raw block" compilation errors.
/// - `[` / `]` — content block delimiters; an unmatched `[` causes "unclosed
///   delimiter" and even matched pairs create unintended nested content blocks
///   when interpolated into function arguments like `#text(...)[user text]`.
/// - `#` — starts a Typst function call or markup directive.
/// - `$` — starts math mode.
/// - `*`, `_` — strong/emphasis markup.
/// - `@`, `~` — reference and non-breaking-space markup.
/// - `<`, `>` — label syntax.
/// - `\` — escape character itself.
pub fn typst_escape(text: &str) -> String {
    let mut result = String::with_capacity(text.len() + 8);
    for c in text.chars() {
        match c {
            '#' | '*' | '_' | '`' | '@' | '<' | '>' | '$' | '~' | '\\'
            | '[' | ']' => {
                result.push('\\');
                result.push(c);
            }
            _ => result.push(c),
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command;

    /// Write PDF bytes to a temp file, extract text with pdftotext, return the text.
    fn pdf_to_text(pdf: &[u8], label: &str) -> String {
        let path = format!("/tmp/kyomi_test_{label}.pdf");
        std::fs::write(&path, pdf).expect("failed to write pdf");
        let out = Command::new("pdftotext")
            .args([&path, "-"])
            .output()
            .expect("pdftotext not found — install poppler-utils");
        assert!(
            out.status.success(),
            "pdftotext failed: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8_lossy(&out.stdout).to_string()
    }

    #[test]
    fn typst_escape_special_chars() {
        assert_eq!(typst_escape("Revenue & *Costs*"), "Revenue & \\*Costs\\*");
        assert_eq!(typst_escape("#heading"), "\\#heading");
        assert_eq!(typst_escape("$100"), "\\$100");
        assert_eq!(typst_escape("normal text"), "normal text");
        // Backtick: must be escaped to prevent "unclosed raw block" Typst errors
        assert_eq!(typst_escape("`code`"), "\\`code\\`");
        assert_eq!(typst_escape("use `orders` table"), "use \\`orders\\` table");
        // Square brackets: must be escaped to prevent unmatched delimiter errors
        assert_eq!(typst_escape("[Q1]"), "\\[Q1\\]");
        assert_eq!(typst_escape("Revenue [2026]"), "Revenue \\[2026\\]");
        // Unmatched bracket that would break Typst compilation without escaping
        assert_eq!(typst_escape("filter [active"), "filter \\[active");
    }

    /// Verify that a Typst document with backtick-containing text compiles
    /// without "unclosed raw block" errors. Regression test for the production
    /// failure where chart titles containing backticks crashed the entire PDF.
    #[test]
    fn document_with_backtick_in_content_compiles() {
        let source = wrap_document(
            "Dashboard with `backtick` title",
            &format!(
                "#block(fill: rgb(\"#F5F3EF\"), stroke: rgb(\"#E8E5DE\"), radius: 6pt, inset: 24pt, width: 100%)[
  #align(center)[#text(fill: rgb(\"#6B6660\"), style: \"italic\", font: \"DM Sans\")[Chart unavailable: {}]]
]",
                typst_escape("Revenue from `orders` table")
            ),
        );
        let pdf = generate_pdf(&source, &HashMap::new())
            .expect("PDF with backtick in chart title must compile without error");
        assert!(!pdf.is_empty());
    }

    /// Verify that a Typst document with square brackets in user content compiles
    /// without "unclosed delimiter" errors.
    #[test]
    fn document_with_brackets_in_content_compiles() {
        let source = wrap_document(
            "Dashboard [Q1 2026]",
            &format!(
                "#block(fill: rgb(\"#F5F3EF\"), stroke: rgb(\"#E8E5DE\"), radius: 6pt, inset: 24pt, width: 100%)[
  #align(center)[#text(fill: rgb(\"#6B6660\"), style: \"italic\", font: \"DM Sans\")[Chart unavailable: {}]]
]",
                typst_escape("Revenue [Q1] (2026)")
            ),
        );
        let pdf = generate_pdf(&source, &HashMap::new())
            .expect("PDF with brackets in chart title must compile without error");
        assert!(!pdf.is_empty());
    }

    #[test]
    fn simple_document_renders_text() {
        let source = r#"
#set page(paper: "a4")
#set text(11pt)
= Hello World
This is a test document with some content.
"#;
        let pdf = generate_pdf(source, &HashMap::new())
            .expect("PDF generation failed");
        let text = pdf_to_text(&pdf, "simple");
        assert!(text.contains("Hello World"), "title missing from PDF text: {text}");
        assert!(text.contains("test document"), "body missing from PDF text: {text}");
    }

    #[test]
    fn wrap_document_renders_title_and_body() {
        let body = "Some paragraph text here.\n\nA second paragraph.";
        let doc = wrap_document("Test Dashboard", body);
        let pdf = generate_pdf(&doc, &HashMap::new())
            .expect("PDF generation failed");
        let text = pdf_to_text(&pdf, "wrapped");
        assert!(text.contains("Test Dashboard"), "title missing: {text}");
        assert!(text.contains("Some paragraph text"), "body missing: {text}");
        assert!(text.contains("kyomi.ai"), "footer missing: {text}");
    }

    #[test]
    fn dashboard_pdf_renders_all_content() {
        use crate::markdown_to_typst::{markdown_to_typst, render_metric_typst, render_data_table_typst};

        let metric1 = render_metric_typst("Total Revenue", "$1.45M", Some(("20.3%", true)));
        let metric2 = render_metric_typst("Active Users", "12,847", Some(("3.1%", false)));
        let metric3 = render_metric_typst("Avg Deal Size", "$31K", Some(("8.7%", true)));

        // Metrics in a side-by-side grid (simulates what replace_chartml_with_typst produces)
        let metric_grid = format!(
            "#grid(\n  columns: (1fr, 1fr, 1fr),\n  column-gutter: 12pt,\n  [{metric1}],\n  [{metric2}],\n  [{metric3}]\n)"
        );

        let table = render_data_table_typst(
            "Top Regions",
            &["Region".into(), "Revenue".into(), "Growth".into()],
            &[
                vec!["North America".into(), "$620K".into(), "+15%".into()],
                vec!["Europe".into(), "$410K".into(), "+22%".into()],
                vec!["Asia Pacific".into(), "$280K".into(), "+31%".into()],
            ],
            3,
            50,
        );

        // No leading H1 — the branded page header already shows the title.
        // In real exports, strip_leading_title() removes the duplicate H1.
        let markdown = format!(
            "{metric_grid}\n\n\
            ## Highlights\n\n\
            - Enterprise deals closed: **12**\n\
            - New customers acquired: **47**\n\
            - Monthly churn rate: `1.2%`\n\n\
            {table}\n\n\
            ## Action Items\n\n\
            1. Expand APAC sales team by Q3\n\
            2. Launch mid-market pricing tier\n\n\
            ---\n\n\
            *Report generated by Kyomi.*\n"
        );

        let typst_body = markdown_to_typst(&markdown);
        let typst_doc = wrap_document("Monthly Revenue Report", &typst_body);
        std::fs::write("/tmp/kyomi_test_dashboard.typ", &typst_doc).unwrap();

        let pdf = generate_pdf(&typst_doc, &HashMap::new())
            .expect("PDF generation failed");
        let text = pdf_to_text(&pdf, "dashboard");

        // Title
        assert!(text.contains("Monthly Revenue Report"), "title missing: {text}");
        // Metric cards
        assert!(text.contains("Total Revenue"), "metric label missing: {text}");
        assert!(text.contains("1.45M"), "metric value missing: {text}");
        assert!(text.contains("20.3%"), "metric trend missing: {text}");
        assert!(text.contains("Active Users"), "metric 2 label missing: {text}");
        assert!(text.contains("12,847"), "metric 2 value missing: {text}");
        // Table
        assert!(text.contains("Top Regions"), "table title missing: {text}");
        assert!(text.contains("North America"), "table row missing: {text}");
        assert!(text.contains("Asia Pacific"), "table row missing: {text}");
        // List
        assert!(text.contains("Enterprise deals closed"), "list item missing: {text}");
        // Numbered list
        assert!(text.contains("Expand APAC"), "numbered list missing: {text}");
        // Footer
        assert!(text.contains("kyomi.ai"), "footer missing: {text}");
    }

    #[test]
    fn wrap_document_with_cover_compiles_and_renders() {
        // Exercises the Phase 6 editorial cover page plus the new `#show
        // figure` rule. Uses the same production emission shape as
        // `pdf_export.rs`: charts wrapped in `#figure(image(...))` so the
        // editorial top+bottom rule treatment fires.
        use crate::chartml_factory::render_chart_to_png_sync;
        use crate::markdown_to_typst::markdown_to_typst;

        let chart_yaml = r##"
type: chart
version: 1
title: "Quarterly Revenue"
data:
  provider: inline
  rows:
    - quarter: "Q1"
      revenue: 142
    - quarter: "Q2"
      revenue: 168
    - quarter: "Q3"
      revenue: 195
    - quarter: "Q4"
      revenue: 231
visualize:
  type: bar
  columns: quarter
  rows: revenue
"##;

        let chart_png = render_chart_to_png_sync(chart_yaml, 700, 380, 144, None)
            .expect("chart render");
        let mut images = HashMap::new();
        images.insert("chart_0.png".to_string(), chart_png);

        // Markdown that mixes all the Phase 4 editorial elements plus a
        // figure-wrapped chart.
        let markdown = "\
## *Quarterly* Highlights\n\n\
Revenue grew consistently through the year, with Q4 showing the largest \
jump driven by **enterprise** expansion in the APAC region.\n\n\
#figure(image(\"chart_0.png\", width: 100%), caption: [Quarterly revenue by region])\n\n\
### Key drivers\n\n\
- Enterprise deals closed: **12**\n\
- New customers: **47**\n\
- Churn rate: `1.2%`\n\n\
> The APAC expansion was the single biggest driver of Q4 growth.\n\n\
---\n\n\
#### Supporting Data\n\n\
##### METRICS TRACKED\n\n\
Metrics pulled from `finance.transactions` and cross-checked against the \
`reporting.monthly_rollup` materialized view.\n";

        let typst_body = markdown_to_typst(markdown);
        let typst_doc = wrap_document_with_cover(
            "Q4 Revenue Report",
            &typst_body,
            Some("April 2026"),
        );
        std::fs::write("/tmp/kyomi_test_editorial.typ", &typst_doc).unwrap();

        let pdf = generate_pdf(&typst_doc, &images)
            .expect("editorial PDF with cover + figure should compile");
        let text = pdf_to_text(&pdf, "editorial");

        // Cover page content. pdftotext inserts spaces between letters when
        // CSS tracking is applied (the eyebrow uses 0.12em), so "KYOMI"
        // extracts as "K YO M I". Strip spaces and compare uppercase to
        // match the actual eyebrow regardless of the visual spacing.
        assert!(text.contains("Q4 Revenue Report"), "cover title missing: {text}");
        let text_nospace = text.to_uppercase().replace(' ', "");
        assert!(
            text_nospace.contains("KYOMI") && text_nospace.contains("DASHBOARD"),
            "cover eyebrow missing: {text}"
        );
        // Headings
        assert!(text.contains("Quarterly Highlights"), "h2 missing: {text}");
        assert!(text.contains("Key drivers"), "h3 missing: {text}");
        // Body
        assert!(text.contains("Enterprise deals closed"), "list item missing: {text}");
        // Blockquote
        assert!(text.contains("APAC expansion"), "blockquote missing: {text}");
        // Figure caption — raw caption text without Typst's auto
        // "Figure N." supplement prefix. We emit "FIGURE N" ourselves
        // as an eyebrow above the chart, so a second prefix inside the
        // caption would be a double label.
        let text_lower = text.to_lowercase();
        assert!(
            text_lower.contains("quarterly revenue"),
            "figure caption missing: {text}"
        );
        assert!(
            !text_lower.contains("figure 1. quarterly"),
            "figure caption has redundant auto-supplement prefix: {text}"
        );
        // The custom show rule should still emit the manual "FIGURE N" eyebrow
        // above the chart body.
        assert!(
            text_lower.contains("figure 1") || text_lower.contains("figure  1"),
            "figure eyebrow missing: {text}"
        );
        // Footer
        assert!(text.contains("kyomi.ai"), "footer missing: {text}");
    }

    #[test]
    fn dashboard_pdf_with_chart() {
        use crate::chartml_factory::render_chart_to_png_sync;
        use crate::markdown_to_typst::{markdown_to_typst, render_metric_typst};

        // ChartML bar chart with inline data (no datasource needed)
        let chart_yaml = r##"
type: chart
version: 1
title: "Monthly Revenue"
data:
  provider: inline
  rows:
    - month: "Jan"
      revenue: 42000
    - month: "Feb"
      revenue: 51000
    - month: "Mar"
      revenue: 48000
    - month: "Apr"
      revenue: 63000
    - month: "May"
      revenue: 71000
    - month: "Jun"
      revenue: 68000
visualize:
  type: bar
  columns: month
  rows: revenue
  axes:
    rows:
      label: "Revenue ($)"
      format: "$,.0f"
"##;

        // Render chart to PNG
        let chart_png = render_chart_to_png_sync(chart_yaml, 700, 380, 144, None)
            .expect("Chart rendering failed");
        assert!(!chart_png.is_empty(), "chart PNG is empty");

        // Store PNG for Typst to embed
        let mut images = HashMap::new();
        images.insert("chart_0.png".to_string(), chart_png);

        let metric = render_metric_typst("Total Revenue", "$343K", Some(("18.2%", true)));

        let markdown = format!(
            "{metric}\n\n\
            ## Monthly Revenue\n\n\
            #block(stroke: 0.5pt + rgb(\"#E8E5DE\"), radius: 4pt, clip: true, width: 100%)[#image(\"chart_0.png\", width: 100%)]\n\n\
            ## Summary\n\n\
            Revenue grew consistently through H1, with June showing a slight dip from May's peak.\n"
        );

        let typst_body = markdown_to_typst(&markdown);
        let typst_doc = wrap_document("Revenue Dashboard", &typst_body);
        std::fs::write("/tmp/kyomi_test_chart.typ", &typst_doc).unwrap();

        let pdf = generate_pdf(&typst_doc, &images).expect("PDF generation failed");
        let text = pdf_to_text(&pdf, "chart");

        assert!(text.contains("Revenue Dashboard"), "title missing: {text}");
        assert!(text.contains("Total Revenue"), "metric missing: {text}");
        assert!(text.contains("343K"), "metric value missing: {text}");
        assert!(text.contains("Monthly Revenue"), "section heading missing: {text}");
        assert!(text.contains("Revenue grew"), "body text missing: {text}");
        assert!(text.contains("kyomi.ai"), "footer missing: {text}");
    }
}
