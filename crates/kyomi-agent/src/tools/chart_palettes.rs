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
//!   1. Look up the user's preferred palette name from
//!      `users.chartml_config.style` in the database.
//!   2. Resolve that name (or the Kyomi default) to a concrete color list
//!      via `kyomi_chart_theme::kyomi_palette(name, is_dark)`.
//!
//! Server-side rendering always uses light mode (`is_dark = false`) because
//! PDFs and email snapshots are displayed on paper or in mail clients where
//! a dark-mode user preference isn't meaningful.

use kyomi_core::{db_fetch_optional, DbPool};
use tracing::warn;

/// Resolve a palette name to its color list. Falls back to the Kyomi default
/// (amber-led editorial warm) when the name is unknown. Always light mode —
/// server-side rendering targets print and email surfaces.
pub fn get_palette(name: &str) -> Vec<String> {
    kyomi_chart_theme::kyomi_palette(name, false)
}

/// Look up the user's preferred palette from `users.chartml_config`.
///
/// The `chartml_config` JSON column stores `{"style": "kyomi"}`,
/// `{"style": "balanced"}`, etc. Falls back to the Kyomi default palette
/// (the new default for workspaces created after the editorial rollout)
/// when the user has no preference or when the DB query fails.
#[derive(sqlx::FromRow)]
struct UserPaletteRow {
    chartml_config: Option<serde_json::Value>,
}

pub async fn get_user_palette(db: &DbPool, user_id: &str) -> Vec<String> {
    let config: Option<serde_json::Value> = match db_fetch_optional!(
        db, UserPaletteRow,
        "SELECT chartml_config FROM users WHERE user_id = $1",
        user_id
    ) {
        Ok(Some(row)) => row.chartml_config,
        Ok(None) => {
            warn!(user_id = %user_id, "User not found when loading palette preference");
            return get_palette("kyomi");
        }
        Err(e) => {
            warn!(error = %e, "Failed to query user palette preference");
            return get_palette("kyomi");
        }
    };

    let palette_name = config
        .as_ref()
        .and_then(|c| c.get("style"))
        .and_then(|v| v.as_str())
        .unwrap_or("kyomi");

    get_palette(palette_name)
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
