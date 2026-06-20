// SPDX-License-Identifier: AGPL-3.0-or-later

//! Chart color palettes for server-side chart rendering (PDF export,
//! email snapshots, scheduled reports).
//!
//! This module is a **thin wrapper** around `kyomi_chart_theme::kyomi_palette`.
//! The actual palette definitions live in the `kyomi-chart-theme` crate so
//! that the browser (`kyomi-ui` wasm32) and the server (`kyomi-agent` native)
//! render identical colors on the same dashboard. If you edit a palette,
//! edit it in `crates/kyomi-chart-theme/src/lib.rs` — that's the one source
//! of truth.
//!
//! Responsibility of this module:
//!   1. Delegate user palette preference lookup to
//!      `kyomi_auth::user_service::get_user_palette_name`, which handles
//!      both flat and nested `chartml_config` storage shapes.
//!   2. Resolve a palette name to a concrete color list via
//!      `kyomi_chart_theme::kyomi_palette(name, is_dark)`.
//!
//! PDF export and the interactive chart tool use light mode (`is_dark =
//! false`) because those surfaces target print or the dashboard canvas where
//! the user's OS dark-mode preference isn't directly meaningful. The email
//! path, however, renders BOTH a light and a dark chart variant (via
//! [`get_user_palette_light_dark`]) so that the `prefers-color-scheme` media
//! query in the email template can swap to a dark-palette chart for mail
//! clients that support it (Apple Mail, iOS Mail).

use kyomi_core::DbPool;

/// Resolve a palette name to its color list for the given mode. The dark
/// variant lifts low-contrast slots for legibility on dark surfaces; see
/// `kyomi_chart_theme::kyomi_palette`.
pub fn get_palette_for_mode(name: &str, is_dark: bool) -> Vec<String> {
    kyomi_chart_theme::kyomi_palette(name, is_dark)
}

/// Resolve a palette name to its color list. Falls back to the Kyomi default
/// (amber-led editorial warm) when the name is unknown. Always light mode —
/// PDF export and the chart tool target print and dashboard surfaces.
pub fn get_palette(name: &str) -> Vec<String> {
    get_palette_for_mode(name, false)
}

/// Look up the user's preferred palette from `users.chartml_config` and
/// resolve it to a concrete color list. Falls back to the Kyomi default
/// palette when the user has no preference, the DB query fails, or the
/// stored palette name is unknown.
pub async fn get_user_palette(db: &DbPool, user_id: &str) -> Vec<String> {
    let name = kyomi_auth::user_service::get_user_palette_name(db, user_id).await;
    get_palette(&name)
}

/// Resolve the user's palette preference once and materialize both the
/// light and dark color lists. The email path renders a light and a dark
/// chart variant from a single name lookup. Returns `(light, dark)`.
pub async fn get_user_palette_light_dark(db: &DbPool, user_id: &str) -> (Vec<String>, Vec<String>) {
    let name = kyomi_auth::user_service::get_user_palette_name(db, user_id).await;
    (get_palette_for_mode(&name, false), get_palette_for_mode(&name, true))
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kyomi_palette_is_the_default() {
        let p = get_palette("kyomi");
        assert_eq!(p.len(), 12);
        assert_eq!(p[0], "#D97706", "slot 1 must be brand amber");
    }

    #[test]
    fn unknown_name_falls_back_to_kyomi() {
        assert_eq!(get_palette("nonexistent"), get_palette("kyomi"));
    }

    #[test]
    fn balanced_still_resolves() {
        // Existing workspaces with explicit `balanced` preference must keep
        // getting the balanced palette, not the new kyomi default.
        let p = get_palette("balanced");
        assert_eq!(p.len(), 12);
        assert_eq!(p[0], "#1A75C9");
    }

    #[test]
    fn vibrant_and_accessible_still_resolve() {
        assert_eq!(get_palette("vibrant")[0], "#1E88C7");
        assert_eq!(get_palette("accessible")[0], "#2D5F7A");
    }

    #[test]
    fn get_palette_for_mode_lifts_kyomi_slot_2_in_dark() {
        let light = get_palette_for_mode("kyomi", false);
        let dark = get_palette_for_mode("kyomi", true);
        // "Slot 2" in the palette comment is 1-based; index 1 in the Vec.
        assert_eq!(light[1], "#1E3A5F", "light slot 2 is editorial navy");
        assert_eq!(dark[1], "#5A87C2", "dark slot 2 lifts for contrast");
        assert_ne!(light[1], dark[1], "email dark variant must differ from light");
    }
}
