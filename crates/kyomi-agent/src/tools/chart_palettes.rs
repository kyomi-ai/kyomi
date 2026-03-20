// SPDX-License-Identifier: AGPL-3.0-or-later

//! Chart color palettes for data visualization.
//!
//! Rust port of Python's `chart_palettes.py`.
//! Palettes are referenced by name in `User.chartml_config.style`.

use kyomi_core::{db_fetch_optional, DbPool};
use tracing::warn;

// ---------------------------------------------------------------------------
// Palette definitions — must match Python's chart_palettes.py
// ---------------------------------------------------------------------------

/// Balanced (Default) — classic BI with varied saturation/luminosity.
const BALANCED: &[&str] = &[
    "#1A75C9", "#B8405A", "#3D8A5A", "#D9952D", "#2D7A8A", "#C9734D",
    "#4D5A8A", "#99C94D", "#8A5A7A", "#D9B370", "#70B8D9", "#6B8A4D",
];

/// Vibrant — higher saturation for modern dashboards.
const VIBRANT: &[&str] = &[
    "#1E88C7", "#D92849", "#28C75A", "#E8B733", "#28C7A8", "#E87333",
    "#3355D9", "#A8D928", "#C728A8", "#D97328", "#28A8D9", "#73A828",
];

/// Accessible — maximum luminosity range for colorblind users.
const ACCESSIBLE: &[&str] = &[
    "#2D5F7A", "#A83D52", "#3D7A52", "#C9A642", "#3D8A8A", "#E89970",
    "#5C6D99", "#B8D96B", "#996B8A", "#B87752", "#85B8D9", "#85996B",
];

// ---------------------------------------------------------------------------
// Palette lookup
// ---------------------------------------------------------------------------

/// Resolve a palette name to its color list. Falls back to balanced.
pub fn get_palette(name: &str) -> Vec<String> {
    let colors = match name {
        "vibrant" => VIBRANT,
        "accessible" => ACCESSIBLE,
        _ => BALANCED,
    };
    colors.iter().map(|s| s.to_string()).collect()
}

/// Look up the user's preferred palette from `users.chartml_config`.
///
/// The `chartml_config` JSON column stores `{"style": "vibrant"}` etc.
/// Falls back to the balanced palette if the user has no preference,
/// or if the DB query fails (non-critical).
/// Row type for user palette query.
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
            return get_palette("balanced");
        }
        Err(e) => {
            warn!(error = %e, "Failed to query user palette preference");
            return get_palette("balanced");
        }
    };

    let palette_name = config
        .as_ref()
        .and_then(|c| c.get("style"))
        .and_then(|v| v.as_str())
        .unwrap_or("balanced");

    get_palette(palette_name)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn all_palettes_have_12_colors() {
        assert_eq!(BALANCED.len(), 12);
        assert_eq!(VIBRANT.len(), 12);
        assert_eq!(ACCESSIBLE.len(), 12);
    }

    #[test]
    fn get_palette_balanced() {
        let p = get_palette("balanced");
        assert_eq!(p.len(), 12);
        assert_eq!(p[0], "#1A75C9");
    }

    #[test]
    fn get_palette_vibrant() {
        let p = get_palette("vibrant");
        assert_eq!(p[0], "#1E88C7");
    }

    #[test]
    fn get_palette_accessible() {
        let p = get_palette("accessible");
        assert_eq!(p[0], "#2D5F7A");
    }

    #[test]
    fn get_palette_unknown_falls_back_to_balanced() {
        let p = get_palette("nonexistent");
        assert_eq!(p, get_palette("balanced"));
    }
}
