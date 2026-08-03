// SPDX-License-Identifier: AGPL-3.0-or-later

//! KYO-229: automated parity guard between `kyomi_theme()` (the SSR-path
//! source of truth — PDF export, email snapshots, MCP chart app) and the
//! `--chartml-*` CSS custom properties in `crates/kyomi-ui/style/main.css`
//! (the browser-path source of truth). Both must exist — see the module
//! docstring on `kyomi_theme` in `src/lib.rs` for why — but nothing
//! previously stopped them from drifting apart. This test does.
//!
//! ## Design chosen: option 2 (a parity test), not option 1 (codegen)
//!
//! The ticket asked to check for an existing codegen/CSS-generation step
//! before adding a new one. There isn't one: the only `build.rs` files in
//! the workspace (`kyomi-embed`, `apps/desktop`, `apps/server`) embed
//! assets/version info, not CSS. Generating `main.css`'s `:root` block from
//! `kyomi_theme()` would also fight the file's own structure — the block
//! carries hand-written explanatory comments (why `--chartml-bg` must match
//! the page background, the table-chrome rationale) that a generator would
//! either have to reproduce as a template (no simpler than this test) or
//! drop. A parity test is cheaper here and was the ticket's stated
//! preference absent a reason to generate.
//!
//! ## Why the mapping is a table, not a struct destructure
//!
//! `chartml_core::theme::Theme` is `#[non_exhaustive]`, so it cannot be
//! pattern-matched exhaustively — the usual "add a field, compiler forces
//! you to handle it" mechanism is unavailable by construction. Two
//! independent inventories stand in for it instead:
//!
//! - [`theme_field_names`] reflects the set of `Theme`'s top-level field
//!   names off its derived `Debug` output (see that function's doc comment
//!   for exactly what this catches and what it doesn't).
//! - The `--chartml-*` variable names actually present in `main.css` are
//!   parsed directly out of the file (see [`extract_top_level_blocks`]).
//!
//! [`MAPPED_FIELDS`] pairs the two together with a value-comparison rule.
//! Anything left over on either side must appear in [`RUST_ONLY_FIELDS`] or
//! [`CSS_ONLY_VARS`] with a justification, or the test fails — see
//! `every_theme_field_is_mapped_or_allow_listed` and
//! `every_chartml_css_variable_is_mapped_or_allow_listed`.

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

use chartml_core::theme::{BarCornerRadius, TextTransform, Theme};
use kyomi_chart_theme::kyomi_theme;

// ============================================================================
// main.css location + minimal CSS parsing
// ============================================================================

fn main_css_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../kyomi-ui/style/main.css")
}

/// Reads `main.css`. Fails loudly (panics with a diagnosable message) if the
/// file is missing — a guard that silently skips when its input disappears
/// is worse than no guard, because CI stays green while the two theme
/// definitions drift unnoticed. Never turn this into `Option`/`.ok()`.
fn main_css_source() -> String {
    let path = main_css_path();
    std::fs::read_to_string(&path).unwrap_or_else(|e| {
        panic!(
            "KYO-229 parity guard: could not read {path:?} ({e}). This test \
             enforces that kyomi_theme() (crates/kyomi-chart-theme/src/lib.rs) \
             and main.css's `--chartml-*` custom properties never drift apart. \
             If main.css moved, update `main_css_path()` in this file — do not \
             skip or ignore this failure."
        )
    })
}

/// Strips `/* ... */` comments. CSS comments don't nest, so a single pass is
/// sufficient. Operates on `char`s (not bytes) so it's safe on any UTF-8
/// content, even though this file's comments are ASCII today.
fn strip_css_comments(css: &str) -> String {
    let mut out = String::with_capacity(css.len());
    let mut chars = css.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '/' && chars.peek() == Some(&'*') {
            chars.next(); // consume '*'
            let mut prev = '\0';
            for cc in chars.by_ref() {
                if prev == '*' && cc == '/' {
                    break;
                }
                prev = cc;
            }
        } else {
            out.push(c);
        }
    }
    out
}

/// Splits `css` into `(selector, body)` pairs for every *top-level* `{ ... }`
/// block (depth 0 -> 1 -> 0), via brace-depth counting. Nested blocks (e.g.
/// a selector inside `@media { ... }`) are included verbatim in their
/// parent's body rather than returned separately — this test only ever
/// looks up bodies for known top-level selectors (`:root`, `.dark`,
/// `@theme`), so that's sufficient.
fn extract_top_level_blocks(css: &str) -> Vec<(String, String)> {
    let mut blocks = Vec::new();
    let mut depth: i32 = 0;
    let mut selector_start = 0usize;
    let mut body_start = 0usize;
    let mut current_selector = String::new();

    for (idx, ch) in css.char_indices() {
        match ch {
            '{' => {
                if depth == 0 {
                    current_selector = css[selector_start..idx].trim().to_string();
                    body_start = idx + ch.len_utf8();
                }
                depth += 1;
            }
            '}' => {
                depth -= 1;
                assert!(
                    depth >= 0,
                    "main.css: unbalanced `}}` while scanning for top-level blocks"
                );
                if depth == 0 {
                    blocks.push((current_selector.clone(), css[body_start..idx].to_string()));
                    selector_start = idx + ch.len_utf8();
                }
            }
            _ => {}
        }
    }
    assert_eq!(
        depth, 0,
        "main.css: unbalanced braces (unterminated block) while scanning for top-level blocks"
    );
    blocks
}

/// Parses `name: value;` declarations out of a flat block body (no nested
/// selectors). Keys retain their leading `--`.
fn parse_declarations(body: &str) -> BTreeMap<String, String> {
    let mut map = BTreeMap::new();
    for decl in body.split(';') {
        let decl = decl.trim();
        if decl.is_empty() {
            continue;
        }
        let Some((name, value)) = decl.split_once(':') else {
            continue;
        };
        let name = name.trim().to_string();
        let value = value.trim().to_string();
        let prev = map.insert(name.clone(), value);
        assert!(
            prev.is_none(),
            "main.css: duplicate declaration of `{name}` within the same block"
        );
    }
    map
}

fn chartml_vars(body: &str) -> BTreeMap<String, String> {
    parse_declarations(body)
        .into_iter()
        .filter_map(|(name, value)| name.strip_prefix("--chartml-").map(|suffix| (suffix.to_string(), value)))
        .collect()
}

/// The two `--chartml-*` blocks plus the two `--color-background` values,
/// parsed once per test from the real `main.css` on disk.
struct ParsedCss {
    light_chartml: BTreeMap<String, String>,
    dark_chartml: BTreeMap<String, String>,
    light_background: String,
    dark_background: String,
}

fn parse_main_css() -> ParsedCss {
    let css = strip_css_comments(&main_css_source());
    let blocks = extract_top_level_blocks(&css);

    // The light chart-theme block is a bare `:root { ... }` selector. The
    // file has more than one bare `:root` block (there's an unrelated one
    // for kode-editor variable mapping), so disambiguate by content rather
    // than assuming position or ordinal.
    let light_block = blocks
        .iter()
        .find(|(selector, body)| selector == ":root" && body.contains("--chartml-"))
        .unwrap_or_else(|| {
            panic!(
                "KYO-229 parity guard: no bare `:root {{ ... }}` block in main.css \
                 contains `--chartml-` declarations. The light chart-theme block may \
                 have moved, been renamed, or been merged into another selector — \
                 update `parse_main_css()` in this test to match."
            )
        });
    let light_chartml = chartml_vars(&light_block.1);
    assert_eq!(
        light_chartml.len(),
        35,
        "KYO-229 parity guard: expected 35 `--chartml-*` properties in main.css's \
         light `:root` block, found {}. If this is an intentional addition/removal, \
         update MAPPED_FIELDS / CSS_ONLY_VARS in this test (and this count) to match \
         — do not just bump the number.",
        light_chartml.len()
    );

    let dark_block = blocks
        .iter()
        .find(|(selector, _)| selector == ".dark")
        .unwrap_or_else(|| panic!("KYO-229 parity guard: no bare `.dark {{ ... }}` block found in main.css"));
    let dark_chartml = chartml_vars(&dark_block.1);
    assert_eq!(
        dark_chartml.len(),
        18,
        "KYO-229 parity guard: expected 18 `--chartml-*` properties in main.css's \
         `.dark` block, found {}. Update MAPPED_FIELDS in this test to match if this \
         is intentional.",
        dark_chartml.len()
    );

    // `--color-background` (light) lives in the `@theme { ... }` block, not
    // `:root` — Tailwind v4's `@theme` directive is how this file declares
    // design-system tokens. The dark value lives in the same `.dark` block
    // already parsed above.
    let theme_block = blocks
        .iter()
        .find(|(selector, _)| selector == "@theme")
        .unwrap_or_else(|| panic!("KYO-229 parity guard: no `@theme {{ ... }}` block found in main.css"));
    let theme_decls = parse_declarations(&theme_block.1);
    let light_background = theme_decls.get("--color-background").cloned().unwrap_or_else(|| {
        panic!("KYO-229 parity guard: `--color-background` not found in main.css's `@theme` block")
    });

    let dark_decls = parse_declarations(&dark_block.1);
    let dark_background = dark_decls
        .get("--color-background")
        .cloned()
        .unwrap_or_else(|| panic!("KYO-229 parity guard: `--color-background` not found in main.css's `.dark` block"));

    ParsedCss {
        light_chartml,
        dark_chartml,
        light_background,
        dark_background,
    }
}

// ============================================================================
// Theme field-name inventory (the non_exhaustive workaround)
// ============================================================================

/// Reflects the set of `Theme`'s top-level field names off its derived
/// `Debug` output.
///
/// `Theme` is `#[non_exhaustive]` in chartml-core, so it cannot be
/// destructured with a struct pattern — the compiler-enforced "handle every
/// field" mechanism that would normally catch a new field is unavailable by
/// design. This is the substitute: `#[derive(Debug)]` unconditionally lists
/// every field as a `name: value` line, and pretty-print (`{:#?}`) indents
/// top-level struct fields by exactly 4 spaces, with nested struct fields
/// (e.g. `ZeroLineSpec`'s `color`/`width` inside `zero_line: Some(...)`)
/// indented further. Filtering to lines with *exactly* 4 leading spaces (the
/// stripped remainder must not itself start with whitespace) isolates
/// `Theme`'s own fields from anything nested inside them. Verified against
/// chartml-core 5.1.12's actual `{:#?}` output before writing this.
///
/// **What this catches:** a field added to, removed from, or renamed on
/// `Theme` upstream changes the set this returns, which fails
/// `every_theme_field_is_mapped_or_allow_listed` below until the new/old
/// name is reconciled against `MAPPED_FIELDS/RUST_ONLY_FIELDS`.
///
/// **What this does NOT catch:** a field keeping its name but changing type
/// (e.g. `title_font_size: f32` becoming a newtype). That can't compile
/// silently either though — `kyomi_theme()`'s assignment (`t.title_font_size
/// = 22.0;`) and this test's `theme_field_repr()` match arm would both fail
/// to compile, just via a type error instead of a failing assertion. It also
/// depends on chartml-core continuing to `#[derive(Debug)]` on `Theme` with
/// unmodified derive formatting; if that ever changes, the sanity `assert!`
/// below (fields.len() >= 40) fails loudly rather than this function
/// silently returning an empty/tiny set that would vacuously pass the
/// exhaustiveness checks.
fn theme_field_names() -> BTreeSet<String> {
    let debug = format!("{:#?}", Theme::default());
    let names: BTreeSet<String> = debug
        .lines()
        .filter_map(|line| {
            let rest = line.strip_prefix("    ")?;
            if rest.starts_with(' ') {
                return None; // more deeply indented — a nested struct's field
            }
            let (name, _) = rest.split_once(':')?;
            if !name.is_empty() && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
                Some(name.to_string())
            } else {
                None
            }
        })
        .collect();
    assert!(
        names.len() >= 40,
        "theme_field_names(): extracted only {} field names from Theme's `{{:#?}}` \
         output, expected at least 40. This almost certainly means chartml-core \
         changed how `Theme` derives/formats Debug output and the 4-space-indent \
         parsing in this function no longer works — fix the parser, do not lower \
         this threshold to make it pass.",
        names.len()
    );
    names
}

// ============================================================================
// The mapping table + allow-lists
// ============================================================================

#[derive(Clone, Copy)]
enum ValueKind {
    /// CSS color literal (hex) vs. a Rust `String` color field. Compared
    /// case-insensitively — both sides currently use uppercase hex, but
    /// that must not be load-bearing.
    Color,
    /// CSS font stack (e.g. `'Instrument Serif', Georgia, serif`) vs. a Rust
    /// `String` with the identical stack. Whitespace-normalized before
    /// comparison rather than assumed byte-identical.
    FontStack,
    /// CSS `<number>px` vs. a Rust `f32` (e.g. `22px` <-> `22.0`).
    PxF32,
    /// CSS bare integer (e.g. `500`) vs. a Rust `u16`.
    Weight,
    /// Values that are already plain strings on both sides (e.g.
    /// `transparent`, `uppercase`, `0`) — compared literally after trimming.
    Literal,
    /// CSS `text-transform` keyword vs. Rust's `TextTransform` enum.
    TextTransform,
    /// CSS `<number>px` vs. Rust's `BarCornerRadius` enum — only the radius
    /// *magnitude* is themeable via CSS; which corners round (`Top` vs
    /// `Uniform`) is a Rust-side structural choice with no CSS
    /// representation, so only the numeric value is compared here.
    BarCornerRadius,
}

/// `(css var name without the `--chartml-` prefix, kyomi_theme() field name, comparison kind)`.
///
/// This is the single source of truth for which `--chartml-*` variable maps
/// to which `Theme` field. Every entry here must correspond to a real,
/// currently-passing value-equality assertion in the tests below — the
/// exhaustiveness tests only check that every *name* is accounted for, not
/// that the comparison kind is correct, so get this right by hand.
const MAPPED_FIELDS: &[(&str, &str, ValueKind)] = &[
    // ----- chrome colors (mode-dependent — overridden in .dark) -----
    ("text", "text", ValueKind::Color),
    ("text-secondary", "text_secondary", ValueKind::Color),
    ("text-strong", "text_strong", ValueKind::Color),
    ("grid", "grid", ValueKind::Color),
    ("axis-line", "axis_line", ValueKind::Color),
    ("bg", "bg", ValueKind::Color),
    // ----- table chrome (mode-dependent — overridden in .dark) -----
    ("table-header-bg", "table_header_bg", ValueKind::Literal),
    ("table-header-text", "table_header_text", ValueKind::Color),
    ("table-header-font-weight", "table_header_font_weight", ValueKind::Literal),
    ("table-header-letter-spacing", "table_header_letter_spacing", ValueKind::Literal),
    ("table-header-text-transform", "table_header_text_transform", ValueKind::Literal),
    ("table-header-border", "table_header_border", ValueKind::Color),
    ("table-row-bg", "table_row_bg", ValueKind::Literal),
    ("table-row-bg-alt", "table_row_bg_alt", ValueKind::Literal),
    ("table-border", "table_border", ValueKind::Color),
    ("table-border-radius", "table_border_radius", ValueKind::Literal),
    ("table-text", "table_text", ValueKind::Color),
    // ----- typography: title (mode-independent — light :root only) -----
    ("title-font", "title_font_family", ValueKind::FontStack),
    ("title-font-size", "title_font_size", ValueKind::PxF32),
    ("title-font-weight", "title_font_weight", ValueKind::Weight),
    ("title-font-style", "title_font_style", ValueKind::Literal),
    // ----- typography: labels (mode-independent) -----
    ("label-font", "label_font_family", ValueKind::FontStack),
    ("label-font-size", "label_font_size", ValueKind::PxF32),
    ("label-font-weight", "label_font_weight", ValueKind::Weight),
    ("label-letter-spacing", "label_letter_spacing", ValueKind::PxF32),
    ("label-text-transform", "label_text_transform", ValueKind::TextTransform),
    // ----- typography: numeric tick values (mode-independent) -----
    ("numeric-font", "numeric_font_family", ValueKind::FontStack),
    ("numeric-font-size", "numeric_font_size", ValueKind::PxF32),
    // ----- typography: legend (mode-independent) -----
    ("legend-font", "legend_font_family", ValueKind::FontStack),
    ("legend-font-size", "legend_font_size", ValueKind::PxF32),
    ("legend-font-weight", "legend_font_weight", ValueKind::Weight),
    // ----- shape / stroke (mode-independent) -----
    ("series-line-weight", "series_line_weight", ValueKind::PxF32),
    ("bar-corner-radius", "bar_corner_radius", ValueKind::BarCornerRadius),
    ("dot-radius", "dot_radius", ValueKind::PxF32),
];

/// `--chartml-*` CSS variables in `main.css` with no `kyomi_theme()` field
/// counterpart. Each entry needs a verified reason.
const CSS_ONLY_VARS: &[(&str, &str)] = &[
    (
        "annotation",
        "chartml-core 5.1.12's `Theme` has `annotation_line_weight` (a stroke \
         width) but no annotation *color* field — verified in \
         ~/.cargo/registry/.../chartml-core-5.1.12/src/theme.rs. \
         `generate_annotations()` in chartml-chart-cartesian 5.0.9 falls back \
         to `theme.text_secondary` for annotation line/label color, not a \
         dedicated field. Additionally verified by grepping every resolved \
         chartml-* crate source (chart-cartesian, chart-scatter, chart-pie, \
         chart-table, chart-metric, chartml-leptos's chartml.css) for \
         `--chartml-annotation`: zero matches. The CSS variable is fully \
         unconsumed — filed as KYO-298 to decide whether chartml gains an \
         annotation-color Theme field or main.css's variable should be \
         removed; out of scope for this parity guard, which must not change \
         rendering.",
    ),
];

/// `kyomi_theme()` / `Theme` fields with no `--chartml-*` CSS counterpart.
/// Each entry needs a verified reason. All ten were checked against
/// chartml-core 5.1.12, chartml-leptos 5.1.9's shipped `chartml.css`, and
/// chartml-chart-{cartesian,scatter,pie,table,metric}'s Rust sources (the
/// versions actually resolved by this workspace's `Cargo.lock`) for any
/// `var(--chartml-<name>, ...)` reference — none exists for any of these.
const RUST_ONLY_FIELDS: &[(&str, &str)] = &[
    (
        "tick",
        "No `var(--chartml-tick, ...)` anywhere in the resolved chartml-* \
         sources. Tick marks are colored from `theme.tick`/`theme.axis_line` \
         as a plain SVG attribute at render time; there is no CSS override \
         path for this field today.",
    ),
    (
        "axis_line_weight",
        "Stroke width is written as a plain SVG attribute from \
         `theme.axis_line_weight`; no CSS var indirection exists for stroke \
         widths anywhere in the theme system.",
    ),
    (
        "grid_line_weight",
        "Same as axis_line_weight — stroke width, no CSS var.",
    ),
    (
        "annotation_line_weight",
        "Same — stroke width for annotation lines, no CSS var. (The \
         annotation *color* has the opposite asymmetry — see \
         CSS_ONLY_VARS's `annotation` entry.)",
    ),
    (
        "dot_halo_color",
        "Dot halos are drawn as a second SVG circle with attributes set \
         directly from `Theme` at render time; no CSS var.",
    ),
    (
        "dot_halo_width",
        "Same as dot_halo_color — no CSS var.",
    ),
    (
        "grid_style",
        "Which gridlines to draw (`GridStyle`) is a structural rendering \
         choice made at SVG-generation time, not a themeable CSS value — no \
         var exists for it.",
    ),
    (
        "zero_line",
        "The emphasized-baseline color and width are plain SVG attributes \
         set directly from `Theme`; no CSS var.",
    ),
    (
        "table_cell_padding",
        "chartml-chart-table 5.0.5's inline `style` generation emits \
         `var(--chartml-table-*, ...)` for header/row/border/text colors and \
         header font-weight/letter-spacing/text-transform/border, but never \
         for cell padding — verified by grepping \
         chartml-chart-table-5.0.5/src/lib.rs for `--chartml-table-cell-padding`.",
    ),
    (
        "table_font_size",
        "Same file, same check — no `--chartml-table-font-size` reference \
         exists in chartml-chart-table 5.0.5.",
    ),
];

// ============================================================================
// Value normalization + per-field string comparison
// ============================================================================

fn normalize_color(value: &str) -> String {
    value.trim().to_ascii_uppercase()
}

fn normalize_font_stack(value: &str) -> String {
    value.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn parse_px(value: &str) -> f32 {
    let trimmed = value.trim();
    let number = trimmed
        .strip_suffix("px")
        .unwrap_or_else(|| panic!("expected a `px` unit in CSS value {trimmed:?}"));
    number
        .trim()
        .parse::<f32>()
        .unwrap_or_else(|e| panic!("failed to parse px value {trimmed:?}: {e}"))
}

fn parse_weight(value: &str) -> u16 {
    value
        .trim()
        .parse::<u16>()
        .unwrap_or_else(|e| panic!("failed to parse font-weight CSS value {value:?}: {e}"))
}

fn parse_text_transform(value: &str) -> TextTransform {
    match value.trim() {
        "uppercase" => TextTransform::Uppercase,
        "lowercase" => TextTransform::Lowercase,
        "none" => TextTransform::None,
        other => panic!("unknown text-transform CSS value {other:?}"),
    }
}

fn text_transform_debug(t: &TextTransform) -> String {
    format!("{t:?}")
}

fn corner_radius_magnitude(r: &BarCornerRadius) -> f32 {
    match r {
        BarCornerRadius::Uniform(v) | BarCornerRadius::Top(v) => *v,
    }
}

/// Converts a raw CSS value for `css_name` into a canonical string for
/// comparison, per its `ValueKind`.
///
/// `PxF32`/`BarCornerRadius` format via `{:?}` (`f32`'s `Debug` impl), not a
/// fixed-precision `{:.4}`. `{:.4}` rounds — a drift smaller than 0.00005
/// (e.g. `1.2` vs. `1.20004`) would produce identical strings and the
/// `assert_eq!` in the tests below would pass silently, which defeats the
/// point of a guard. `f32`'s `Debug` format is the shortest decimal string
/// that round-trips back to the exact same `f32` bit pattern, so two
/// different `f32` values are guaranteed to format differently — the
/// comparison becomes exact while staying string-based (this file compares
/// every `ValueKind` as a `String`; keeping that uniform, rather than special
/// -casing float kinds to compare typed `f32`s, keeps the mapped-field loop
/// in `light_mode_values_match_kyomi_theme_false` etc. simple). It also
/// reads *better* than `{:.4}` for whole numbers (`22.0` instead of
/// `22.0000`).
fn css_value_repr(kind: ValueKind, raw: &str) -> String {
    match kind {
        ValueKind::Color => normalize_color(raw),
        ValueKind::FontStack => normalize_font_stack(raw),
        ValueKind::PxF32 | ValueKind::BarCornerRadius => format!("{:?}", parse_px(raw)),
        ValueKind::Weight => parse_weight(raw).to_string(),
        ValueKind::Literal => raw.trim().to_string(),
        ValueKind::TextTransform => text_transform_debug(&parse_text_transform(raw)),
    }
}

/// Converts the named `Theme` field's value into the same canonical string
/// space as [`css_value_repr`]. One arm per entry in [`MAPPED_FIELDS`] —
/// deliberately explicit (not reflection-based) because the unit
/// conversions differ per field and reflection would just move the
/// bug-surface from "forgot to map a field" to "forgot to convert it
/// correctly, silently."
///
/// The `f32` fields (font sizes, letter spacing, stroke/corner-radius
/// magnitudes) format via `{:?}`, matching `css_value_repr`'s `PxF32`/
/// `BarCornerRadius` arm — see that function's doc comment for why (exact,
/// round-trip-lossless comparison instead of a `{:.4}` that rounds sub
/// -0.00005 drift away).
fn theme_field_repr(theme: &Theme, rust_field: &str) -> String {
    match rust_field {
        "text" => normalize_color(&theme.text),
        "text_secondary" => normalize_color(&theme.text_secondary),
        "text_strong" => normalize_color(&theme.text_strong),
        "grid" => normalize_color(&theme.grid),
        "axis_line" => normalize_color(&theme.axis_line),
        "bg" => normalize_color(&theme.bg),
        "table_header_bg" => theme.table_header_bg.trim().to_string(),
        "table_header_text" => normalize_color(&theme.table_header_text),
        "table_header_font_weight" => theme.table_header_font_weight.trim().to_string(),
        "table_header_letter_spacing" => theme.table_header_letter_spacing.trim().to_string(),
        "table_header_text_transform" => theme.table_header_text_transform.trim().to_string(),
        "table_header_border" => normalize_color(&theme.table_header_border),
        "table_row_bg" => theme.table_row_bg.trim().to_string(),
        "table_row_bg_alt" => theme.table_row_bg_alt.trim().to_string(),
        "table_border" => normalize_color(&theme.table_border),
        "table_border_radius" => theme.table_border_radius.trim().to_string(),
        "table_text" => normalize_color(&theme.table_text),
        "title_font_family" => normalize_font_stack(&theme.title_font_family),
        "title_font_size" => format!("{:?}", theme.title_font_size),
        "title_font_weight" => theme.title_font_weight.to_string(),
        "title_font_style" => theme.title_font_style.trim().to_string(),
        "label_font_family" => normalize_font_stack(&theme.label_font_family),
        "label_font_size" => format!("{:?}", theme.label_font_size),
        "label_font_weight" => theme.label_font_weight.to_string(),
        "label_letter_spacing" => format!("{:?}", theme.label_letter_spacing),
        "label_text_transform" => text_transform_debug(&theme.label_text_transform),
        "numeric_font_family" => normalize_font_stack(&theme.numeric_font_family),
        "numeric_font_size" => format!("{:?}", theme.numeric_font_size),
        "legend_font_family" => normalize_font_stack(&theme.legend_font_family),
        "legend_font_size" => format!("{:?}", theme.legend_font_size),
        "legend_font_weight" => theme.legend_font_weight.to_string(),
        "series_line_weight" => format!("{:?}", theme.series_line_weight),
        "bar_corner_radius" => format!("{:?}", corner_radius_magnitude(&theme.bar_corner_radius)),
        "dot_radius" => format!("{:?}", theme.dot_radius),
        other => panic!(
            "theme_field_repr(): no comparison arm for Theme field `{other}` — it's in \
             MAPPED_FIELDS but this function wasn't updated to match. Add an arm."
        ),
    }
}

// ============================================================================
// Tests
// ============================================================================

#[test]
fn light_mode_values_match_kyomi_theme_false() {
    let css = parse_main_css();
    let theme = kyomi_theme(false);
    for &(css_name, rust_field, kind) in MAPPED_FIELDS {
        let raw = css
            .light_chartml
            .get(css_name)
            .unwrap_or_else(|| panic!("--chartml-{css_name} missing from main.css's light `:root` block"));
        let expected = css_value_repr(kind, raw);
        let actual = theme_field_repr(&theme, rust_field);
        assert_eq!(
            actual, expected,
            "kyomi_theme(false).{rust_field} ({actual:?}) does not match \
             --chartml-{css_name}: {raw:?} in main.css's light `:root` block"
        );
    }
}

#[test]
fn dark_mode_overridden_values_match_kyomi_theme_true() {
    let css = parse_main_css();
    let theme = kyomi_theme(true);
    for &(css_name, rust_field, kind) in MAPPED_FIELDS {
        let Some(raw) = css.dark_chartml.get(css_name) else {
            continue; // not overridden in .dark — covered by the mode-independence test below
        };
        let expected = css_value_repr(kind, raw);
        let actual = theme_field_repr(&theme, rust_field);
        assert_eq!(
            actual, expected,
            "kyomi_theme(true).{rust_field} ({actual:?}) does not match \
             --chartml-{css_name}: {raw:?} in main.css's `.dark` block"
        );
    }
}

#[test]
fn fields_not_overridden_in_dark_css_are_mode_independent_in_rust() {
    let css = parse_main_css();
    let light = kyomi_theme(false);
    let dark = kyomi_theme(true);
    for &(css_name, rust_field, _) in MAPPED_FIELDS {
        if css.dark_chartml.contains_key(css_name) {
            continue; // dark-overridden — covered by the previous test
        }
        let light_repr = theme_field_repr(&light, rust_field);
        let dark_repr = theme_field_repr(&dark, rust_field);
        assert_eq!(
            light_repr, dark_repr,
            "--chartml-{css_name} has no `.dark` override in main.css, so \
             kyomi_theme() must agree between light and dark for `{rust_field}` — \
             found light={light_repr:?} dark={dark_repr:?}. Either main.css needs \
             a `.dark` override for --chartml-{css_name}, or kyomi_theme() has a \
             real light/dark divergence CSS can't see. Report this — do not \
             'fix' it by editing either side without a rendering decision."
        );
    }
}

#[test]
fn every_chartml_css_variable_is_mapped_or_allow_listed() {
    let css = parse_main_css();
    let mapped: BTreeSet<&str> = MAPPED_FIELDS.iter().map(|(css_name, ..)| *css_name).collect();
    let allow_listed: BTreeSet<&str> = CSS_ONLY_VARS.iter().map(|(name, _)| *name).collect();

    let stale_in_both: Vec<&str> = mapped.intersection(&allow_listed).copied().collect();
    assert!(
        stale_in_both.is_empty(),
        "--chartml-{stale_in_both:?} is both mapped in MAPPED_FIELDS and allow-listed \
         in CSS_ONLY_VARS — remove the stale entry."
    );

    for name in css.light_chartml.keys() {
        assert!(
            mapped.contains(name.as_str()) || allow_listed.contains(name.as_str()),
            "--chartml-{name} is defined in main.css's light `:root` block but is \
             not in MAPPED_FIELDS (no matching kyomi_theme() field) or \
             CSS_ONLY_VARS (no justified reason it lacks one). Either wire it to a \
             Theme field in kyomi_theme() or add a justified CSS_ONLY_VARS entry."
        );
    }

    for name in mapped.iter().chain(allow_listed.iter()) {
        assert!(
            css.light_chartml.contains_key(*name),
            "MAPPED_FIELDS/CSS_ONLY_VARS references --chartml-{name}, but it no \
             longer exists in main.css's light `:root` block — update the mapping \
             table in this test."
        );
    }

    // The `.dark` block only overrides the subset of properties that are
    // mode-dependent (17-18 of the 35 today) — everything else is
    // legitimately absent there and inherited from `:root`. So this is a
    // one-directional membership check, not full exhaustiveness like the
    // light block above: every property that *is* present in `.dark` must
    // be a name we recognize (mapped or allow-listed), but a recognized
    // name being *absent* from `.dark` is not a failure. Catches an
    // unexpected/typo'd dark-block property by name; the count assertion
    // in `parse_main_css()` remains the backstop for *removals*, which a
    // membership check alone can't see.
    for name in css.dark_chartml.keys() {
        assert!(
            mapped.contains(name.as_str()) || allow_listed.contains(name.as_str()),
            "--chartml-{name} is defined in main.css's `.dark` block but is not in \
             MAPPED_FIELDS (no matching kyomi_theme() field) or CSS_ONLY_VARS (no \
             justified reason it lacks one). Either wire it to a Theme field in \
             kyomi_theme() or add a justified CSS_ONLY_VARS entry."
        );
    }
}

#[test]
fn every_theme_field_is_mapped_or_allow_listed() {
    let names = theme_field_names();
    let mapped: BTreeSet<&str> = MAPPED_FIELDS.iter().map(|(_, field, _)| *field).collect();
    let allow_listed: BTreeSet<&str> = RUST_ONLY_FIELDS.iter().map(|(field, _)| *field).collect();

    let stale_in_both: Vec<&str> = mapped.intersection(&allow_listed).copied().collect();
    assert!(
        stale_in_both.is_empty(),
        "Theme field(s) {stale_in_both:?} are both mapped in MAPPED_FIELDS and \
         allow-listed in RUST_ONLY_FIELDS — remove the stale entry."
    );

    for name in &names {
        assert!(
            mapped.contains(name.as_str()) || allow_listed.contains(name.as_str()),
            "Theme field `{name}` (chartml_core::theme::Theme) has no entry in \
             MAPPED_FIELDS or RUST_ONLY_FIELDS. This is likely a NEW field added \
             upstream in chartml-core — decide whether kyomi_theme() should set it \
             and whether main.css needs a matching --chartml-* variable, then add \
             the mapping (or a justified RUST_ONLY_FIELDS entry if it genuinely \
             can't be themed via CSS)."
        );
    }

    for name in mapped.iter().chain(allow_listed.iter()) {
        assert!(
            names.contains(*name),
            "MAPPED_FIELDS/RUST_ONLY_FIELDS references Theme field `{name}`, but it \
             no longer exists on chartml_core::theme::Theme — update the mapping \
             table in this test. (If many fields vanish here at once, check \
             whether Theme's Debug derive/pretty-print format changed instead — \
             see theme_field_names()'s doc comment.)"
        );
    }
}

#[test]
fn background_colors_match_design_system_color_background() {
    let css = parse_main_css();
    let light = kyomi_theme(false);
    let dark = kyomi_theme(true);

    assert_eq!(
        normalize_color(&light.bg),
        normalize_color(&css.light_background),
        "kyomi_theme(false).bg must equal main.css's --color-background (light) — \
         both encode the same DESIGN.md page-background constant and must never \
         drift independently"
    );
    assert_eq!(
        normalize_color(&dark.bg),
        normalize_color(&css.dark_background),
        "kyomi_theme(true).bg must equal main.css's --color-background (dark)"
    );

    // Pin the literal DESIGN.md-derived values too, so a change to either
    // constant is a deliberate, reviewed edit rather than incidental drift
    // (KYO-229's stated minimum bar).
    assert_eq!(normalize_color(&css.light_background), "#FAFAF8");
    assert_eq!(normalize_color(&css.dark_background), "#12100F");
}
