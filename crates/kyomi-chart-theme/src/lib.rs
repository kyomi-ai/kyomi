// SPDX-License-Identifier: AGPL-3.0-or-later

//! Kyomi chart palettes and editorial chart theme.
//!
//! This module is the single Rust source of truth for the chart visual
//! identity across every Kyomi surface — the dashboard viewer, the chart
//! builder, the PDF export, the watches email snapshots, and the MCP
//! chart app. Both `kyomi-ui` (browser rendering path) and `kyomi-agent`
//! (SSR / PDF / email path) consume the same functions defined here.
//!
//! The browser rendering path additionally mirrors these values as CSS
//! custom properties in `crates/kyomi-ui/style/main.css` under the
//! `:root { --chartml-* }` block, because chartml-leptos's shipped CSS
//! rules win over SVG presentation attributes in the browser cascade.
//! That block is a hand-maintained mirror — if you edit `kyomi_theme()`,
//! you must also edit `main.css`. The module docstring on `kyomi_theme`
//! below spells out the contract.

use chartml_core::theme::{BarCornerRadius, GridStyle, TextTransform, Theme, ZeroLineSpec};

/// Return the 12-color qualitative palette for the given palette name.
///
/// `is_dark` only affects palettes that override individual slots for
/// dark-mode readability. Today only `kyomi` does this (slot 2 navy lifts
/// to a brighter mid-lightness blue on `#12100F`). The other palettes are
/// mode-independent and ignore the flag.
///
/// Unknown palette names fall back to `kyomi`, which is the new default
/// for every workspace created on or after the Variant A editorial rollout.
pub fn kyomi_palette(name: &str, is_dark: bool) -> Vec<String> {
    match name {
        // Kyomi signature — amber-anchored editorial warm. Slot 2 is
        // per-mode: #1E3A5F reads strong against the warm light `#FAFAF8`
        // but disappears against `#12100F` in dark mode, so we lift it to
        // `#5A87C2` (same ~213° hue family, ~60% lightness) on dark.
        "kyomi" => {
            let slot2 = if is_dark { "#5A87C2" } else { "#1E3A5F" };
            vec![
                "#D97706", slot2, "#3D8A5A", "#7C2D12", "#2D7A8A", "#A16207",
                "#7E22CE", "#6B8A4D", "#0891B2", "#9F1239", "#CA8A04", "#4D5A8A",
            ]
        }
        "vibrant" => vec![
            "#1E88C7", "#D92849", "#28C75A", "#E8B733", "#28C7A8", "#E87333",
            "#3355D9", "#A8D928", "#C728A8", "#D97328", "#28A8D9", "#73A828",
        ],
        "accessible" => vec![
            "#2D5F7A", "#A83D52", "#3D7A52", "#C9A642", "#3D8A8A", "#E89970",
            "#5C6D99", "#B8D96B", "#996B8A", "#B87752", "#85B8D9", "#85996B",
        ],
        "balanced" => vec![
            "#1A75C9", "#B8405A", "#3D8A5A", "#D9952D", "#2D7A8A", "#C9734D",
            "#4D5A8A", "#99C94D", "#8A5A7A", "#D9B370", "#70B8D9", "#6B8A4D",
        ],
        // Unknown name → fall back to kyomi (the new default).
        _ => return kyomi_palette("kyomi", is_dark),
    }
    .into_iter()
    .map(String::from)
    .collect()
}

/// Kyomi chart chrome theme — "Editorial Figure" (Variant A).
///
/// Instrument Serif titles, DM Sans uppercase tracked axis labels,
/// Geist Mono tabular numerics, horizontal-only warm gridlines, 2px
/// series lines, top-rounded bars, haloed dot markers, emphasized
/// baseline on signed data, page-matched background.
///
/// See DESIGN.md §Chart Chrome for the rationale and the Variant A
/// samples at `/tmp/kyomi-chartml-variants.html` for the visual reference.
///
/// # Source of truth — read this before editing
///
/// This function is the single source of truth for the **SSR path** —
/// PDF export, email snapshots, MCP chart app — where there is no CSS
/// cascade and chartml reads values directly from SVG presentation
/// attributes.
///
/// For the **browser path**, `crates/kyomi-ui/style/main.css` mirrors
/// these values as CSS custom properties in the `:root { --chartml-* }`
/// block. chartml-leptos's shipped CSS rules (`.tick-value { font-family:
/// var(...) }` etc.) win over SVG presentation attributes in the browser
/// cascade, so without those CSS variables the browser renders with the
/// wrong typography.
///
/// **If you edit any field here, also edit the matching `--chartml-*`
/// CSS variable in `main.css`.** They're hand-maintained mirrors; drift
/// produces inconsistent rendering between the dashboard viewer and the
/// PDF export.
pub fn kyomi_theme(is_dark: bool) -> Theme {
    // Page-matched chrome: chartml never emits a background rect, so the
    // chart inherits the surface it's placed on. `theme.bg` is the color
    // used for element *separators* (dot outlines, pie slice gaps, stacked
    // bar segment borders) — it MUST match the actual page background so
    // those separators visually disappear into the surface.
    let (page_bg, text_primary, text_secondary, axis, grid, zero, halo) = if is_dark {
        (
            "#12100F", // --color-background (dark) — warm near-black
            "#F5F3EF", // text
            "#A8A29E", // text-secondary — warm stone-400
            "#A8A29E", // axis line — muted warm on dark
            "#2E2925", // grid — warm neutral, visible but restrained
            "#F5F3EF", // zero line — emphasized baseline on dark
            "#12100F", // dot halo — matches page bg
        )
    } else {
        (
            "#FAFAF8", // --color-background (light)
            "#1C1917", // text
            "#6B6660", // text-secondary
            "#1C1917", // axis line — editorial baseline weight
            "#EDE9E0", // grid — warm neutral, horizontal only
            "#1C1917", // zero line — emphasized baseline on light
            "#FAFAF8", // dot halo — matches page bg
        )
    };

    // `Theme` is `#[non_exhaustive]` upstream, so consumers can't build it
    // with struct literals. Start from the chartml default and override
    // only the fields that carry editorial intent. Any future field
    // additions in chartml-core get sensible defaults automatically —
    // this is a compile-time-enforced contract, not a convention.
    let mut t = Theme::default();

    // ----- chrome colors -----
    t.text = text_primary.into();
    t.text_secondary = text_secondary.into();
    t.text_strong = text_primary.into();
    t.axis_line = axis.into();
    t.tick = axis.into();
    t.grid = grid.into();
    t.bg = page_bg.into();

    // ----- typography: title -----
    t.title_font_family = "'Instrument Serif', Georgia, serif".into();
    t.title_font_size = 22.0;
    t.title_font_weight = 400;
    t.title_font_style = "normal".into();

    // ----- typography: labels -----
    t.label_font_family = "'DM Sans', system-ui, sans-serif".into();
    t.label_font_size = 10.0;
    t.label_font_weight = 500;
    t.label_letter_spacing = 1.2;
    t.label_text_transform = TextTransform::Uppercase;

    // ----- typography: numeric tick values -----
    t.numeric_font_family = "'Geist Mono', ui-monospace, monospace".into();
    t.numeric_font_size = 11.0;

    // ----- typography: legend -----
    t.legend_font_family = "'DM Sans', system-ui, sans-serif".into();
    t.legend_font_size = 11.0;
    t.legend_font_weight = 500;

    // ----- shape / stroke -----
    t.axis_line_weight = 1.0;
    t.grid_line_weight = 1.0;
    t.series_line_weight = 2.0;
    t.annotation_line_weight = 1.0;
    t.bar_corner_radius = BarCornerRadius::Top(2.0);
    t.dot_radius = 4.0;
    t.dot_halo_color = Some(halo.into());
    t.dot_halo_width = 1.5;

    // ----- grid + baseline -----
    t.grid_style = GridStyle::HorizontalOnly;
    t.zero_line = Some(ZeroLineSpec {
        color: zero.into(),
        width: 1.5,
    });

    // ----- table chrome -----
    //
    // Derived from the same editorial palette used for the rest of the
    // theme so rendered `<table>`s in chart exports match the chart chrome
    // they sit alongside. The row background uses the page-matched
    // `page_bg` for a transparent-feeling surface, the alternating stripe
    // and the cell borders both use `grid` (warm neutral in both modes —
    // `#EDE9E0` on light, `#2E2925` on dark), and cell text uses the primary
    // so headers and body read at the same weight as axis labels. Cell
    // padding and font size are CSS string values — kept modest so tables
    // don't dominate a chart figure.
    t.table_header_bg = grid.into();
    t.table_header_text = text_primary.into();
    t.table_row_bg = page_bg.into();
    t.table_row_bg_alt = grid.into();
    t.table_border = grid.into();
    t.table_text = text_primary.into();
    t.table_cell_padding = "8px 12px".into();
    t.table_font_size = "13px".into();

    t
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kyomi_palette_light_mode_uses_editorial_navy_at_slot_2() {
        let colors = kyomi_palette("kyomi", false);
        assert_eq!(colors.len(), 12);
        assert_eq!(colors[0], "#D97706", "slot 1 must be brand amber");
        assert_eq!(
            colors[1], "#1E3A5F",
            "slot 2 must be editorial navy in light mode"
        );
    }

    #[test]
    fn kyomi_palette_dark_mode_lifts_slot_2_for_contrast() {
        let colors = kyomi_palette("kyomi", true);
        assert_eq!(colors.len(), 12);
        assert_eq!(colors[0], "#D97706", "slot 1 must be brand amber in both modes");
        assert_eq!(
            colors[1], "#5A87C2",
            "slot 2 must lift to mid-lightness blue in dark mode so it doesn't \
             disappear into the #12100F page background"
        );
    }

    #[test]
    fn kyomi_palette_unknown_name_falls_back_to_kyomi() {
        let fallback = kyomi_palette("nonexistent", false);
        let kyomi = kyomi_palette("kyomi", false);
        assert_eq!(fallback, kyomi);
    }

    #[test]
    fn balanced_palette_is_mode_independent() {
        assert_eq!(
            kyomi_palette("balanced", false),
            kyomi_palette("balanced", true)
        );
    }

    #[test]
    fn kyomi_theme_uses_variant_a_typography() {
        let t = kyomi_theme(false);
        assert_eq!(t.title_font_family, "'Instrument Serif', Georgia, serif");
        assert_eq!(t.title_font_size, 22.0);
        assert_eq!(t.title_font_weight, 400);
        assert_eq!(t.label_font_family, "'DM Sans', system-ui, sans-serif");
        assert_eq!(t.label_text_transform, TextTransform::Uppercase);
        assert_eq!(t.label_letter_spacing, 1.2);
        assert_eq!(t.numeric_font_family, "'Geist Mono', ui-monospace, monospace");
    }

    #[test]
    fn kyomi_theme_uses_variant_a_shape() {
        let t = kyomi_theme(false);
        assert_eq!(t.series_line_weight, 2.0);
        assert_eq!(t.dot_radius, 4.0);
        assert_eq!(t.dot_halo_width, 1.5);
        assert!(matches!(t.bar_corner_radius, BarCornerRadius::Top(2.0)));
        assert_eq!(t.grid_style, GridStyle::HorizontalOnly);
        assert!(t.zero_line.is_some());
    }

    #[test]
    fn kyomi_theme_light_mode_page_matched_bg() {
        let t = kyomi_theme(false);
        // theme.bg is the element separator color — MUST match the page
        // background, not be empty or white, so dot outlines and pie slice
        // gaps disappear into the warm surface.
        assert_eq!(t.bg, "#FAFAF8");
        assert_eq!(t.dot_halo_color.as_deref(), Some("#FAFAF8"));
        assert_eq!(t.axis_line, "#1C1917");
        assert_eq!(t.grid, "#EDE9E0");
    }

    #[test]
    fn kyomi_theme_dark_mode_page_matched_bg() {
        let t = kyomi_theme(true);
        assert_eq!(t.bg, "#12100F");
        assert_eq!(t.dot_halo_color.as_deref(), Some("#12100F"));
        assert_eq!(t.axis_line, "#A8A29E");
        assert_eq!(t.grid, "#2E2925");
        assert_eq!(t.text, "#F5F3EF");
    }

    #[test]
    fn kyomi_theme_emphasized_zero_line_matches_axis_color() {
        let light = kyomi_theme(false);
        let dark = kyomi_theme(true);
        let lz = light.zero_line.as_ref().expect("light zero_line set");
        let dz = dark.zero_line.as_ref().expect("dark zero_line set");
        assert_eq!(lz.color, "#1C1917");
        assert_eq!(lz.width, 1.5);
        assert_eq!(dz.color, "#F5F3EF");
        assert_eq!(dz.width, 1.5);
    }
}
