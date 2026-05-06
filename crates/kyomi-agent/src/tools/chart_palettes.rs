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
//! Server-side rendering always uses light mode (`is_dark = false`) because
//! PDFs and email snapshots are displayed on paper or in mail clients where
//! a dark-mode user preference isn't meaningful.

use kyomi_core::DbPool;

/// Resolve a palette name to its color list. Falls back to the Kyomi default
/// (amber-led editorial warm) when the name is unknown. Always light mode —
/// server-side rendering targets print and email surfaces.
pub fn get_palette(name: &str) -> Vec<String> {
    kyomi_chart_theme::kyomi_palette(name, false)
}

/// Look up the user's preferred palette from `users.chartml_config` and
/// resolve it to a concrete color list. Falls back to the Kyomi default
/// palette when the user has no preference, the DB query fails, or the
/// stored palette name is unknown.
pub async fn get_user_palette(db: &DbPool, user_id: &str) -> Vec<String> {
    let name = kyomi_auth::user_service::get_user_palette_name(db, user_id).await;
    get_palette(&name)
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
}
