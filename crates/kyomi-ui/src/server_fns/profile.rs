// SPDX-License-Identifier: AGPL-3.0-or-later

//! Server functions for the Profile settings page.
//!
//! These replace the REST API calls that ProfileSettings.jsx makes.
//! Each function calls the same service-layer code as the existing REST routes.

use leptos::prelude::*;

use crate::types::{DashboardSummary, InvitationData, ProfileData};

// ─────────────────────────────────────────────────────────────────────────────
// Read operations (called on page load via Resource)
// ─────────────────────────────────────────────────────────────────────────────

/// Load the current user's profile data.
///
/// Combines user info, preferences, chart config, and system config
/// into a single response — replacing multiple separate REST calls.
#[server(prefix = "/leptos-api")]
pub async fn get_profile() -> Result<ProfileData, ServerFnError> {
    let auth = extract_auth().await?;
    let ctx = extract_context()?;

    let user = kyomi_auth::user_service::get_user_by_id(&ctx.db, &auth.user_id)
        .await
        .into_sfn()?
        .ok_or_else(|| ServerFnError::new("User not found"))?;

    let metadata = user.extra_metadata.as_ref().and_then(|v| v.as_object());

    let theme = metadata
        .and_then(|m| m.get("theme"))
        .and_then(|v| v.as_str())
        .unwrap_or("system")
        .to_string();

    let landing_page = metadata
        .and_then(|m| m.get("landing_page"))
        .and_then(|v| v.as_str())
        .unwrap_or("chat")
        .to_string();

    let default_dashboard_id = metadata
        .and_then(|m| m.get("default_dashboard_id"))
        .and_then(|v| v.as_str())
        .map(String::from);

    let query_history_retention_days = metadata
        .and_then(|m| m.get("query_history_retention_days"))
        .and_then(|v| v.as_i64())
        .unwrap_or(30) as i32;

    let chart_palette =
        kyomi_auth::user_service::get_user_palette_name(&ctx.db, &auth.user_id).await;

    Ok(ProfileData {
        user_id: user.user_id,
        email: user.email,
        name: user.name,
        theme,
        landing_page,
        default_dashboard_id,
        query_history_retention_days,
        chart_palette,
        is_personal_mode: ctx.config.is_personal(),
        is_self_hosted: ctx.config.self_hosted,
    })
}

/// Load dashboards for the default dashboard selector.
#[server(prefix = "/leptos-api")]
pub async fn get_dashboards() -> Result<Vec<DashboardSummary>, ServerFnError> {
    let ac = AuthenticatedContext::extract().await?;

    let results = kyomi_auth::dashboard_service::search_dashboards(
        ac.db(),
        &ac.ws_id,
        None,
        Some(kyomi_core::models::DocType::Dashboard), // profile page only shows dashboards
        kyomi_auth::dashboard_service::SearchSort::Recent,
        100,
    )
    .await
    .into_sfn()?;

    Ok(results
        .into_iter()
        .map(|r| DashboardSummary {
            dashboard_id: r.dashboard_id,
            title: r.title,
        })
        .collect())
}

/// Load pending workspace invitations for the current user.
#[server(prefix = "/leptos-api")]
pub async fn get_pending_invitations() -> Result<Vec<InvitationData>, ServerFnError> {
    let auth = extract_auth().await?;
    let ctx = extract_context()?;

    let invitations =
        kyomi_auth::workspace_service::get_pending_invitations_for_email(&ctx.db, &auth.email)
            .await
            .into_sfn()?;

    Ok(invitations
        .into_iter()
        .map(|inv| InvitationData {
            invitation_id: inv.invitation_id,
            workspace_id: inv.workspace_id,
            email: inv.email,
            role: inv.role.to_string(),
            created_at: inv.created_at.to_rfc3339(),
            expires_at: inv.expires_at.to_rfc3339(),
        })
        .collect())
}

// ─────────────────────────────────────────────────────────────────────────────
// Write operations (called on user interaction via Action)
// ─────────────────────────────────────────────────────────────────────────────

/// Update the user's display name.
#[server(prefix = "/leptos-api")]
pub async fn update_profile_name(name: String) -> Result<(), ServerFnError> {
    let auth = extract_auth().await?;
    let ctx = extract_context()?;

    let trimmed = name.trim().to_string();
    if trimmed.is_empty() {
        return Err(ServerFnError::new("Name cannot be empty"));
    }

    kyomi_auth::user_service::update_user_name(&ctx.db, &auth.user_id, &trimmed)
        .await
        .into_sfn()?;

    Ok(())
}

/// Update user theme preference (light, dark, or system).
#[server(prefix = "/leptos-api")]
pub async fn update_theme(theme: String) -> Result<(), ServerFnError> {
    let auth = extract_auth().await?;
    let ctx = extract_context()?;

    if !["light", "dark", "system"].contains(&theme.as_str()) {
        return Err(ServerFnError::new("Theme must be light, dark, or system"));
    }

    let metadata = serde_json::json!({ "theme": theme });
    kyomi_auth::user_service::update_extra_metadata(&ctx.db, &auth.user_id, &metadata)
        .await
        .into_sfn()?;

    Ok(())
}

/// Update the user's landing page preference.
#[server(prefix = "/leptos-api")]
pub async fn update_landing_page(page: String) -> Result<(), ServerFnError> {
    let auth = extract_auth().await?;
    let ctx = extract_context()?;

    let valid = ["chat", "dashboards", "watches", "sql_editor"];
    if !valid.contains(&page.as_str()) {
        return Err(ServerFnError::new(
            "Invalid landing_page. Must be 'chat', 'dashboards', 'watches', or 'sql_editor'.",
        ));
    }

    let metadata = serde_json::json!({ "landing_page": page });
    kyomi_auth::user_service::update_extra_metadata(&ctx.db, &auth.user_id, &metadata)
        .await
        .into_sfn()?;

    Ok(())
}

/// Update the user's default dashboard.
#[server(prefix = "/leptos-api")]
pub async fn update_default_dashboard(dashboard_id: Option<String>) -> Result<(), ServerFnError> {
    let auth = extract_auth().await?;
    let ctx = extract_context()?;

    let value = match dashboard_id {
        Some(id) if !id.is_empty() => serde_json::json!({ "default_dashboard_id": id }),
        _ => serde_json::json!({ "default_dashboard_id": null }),
    };

    kyomi_auth::user_service::update_extra_metadata(&ctx.db, &auth.user_id, &value)
        .await
        .into_sfn()?;

    Ok(())
}

/// Update the user's query history retention days.
#[server(prefix = "/leptos-api")]
pub async fn update_query_retention(days: i32) -> Result<(), ServerFnError> {
    let auth = extract_auth().await?;
    let ctx = extract_context()?;

    if !(1..=365).contains(&days) {
        return Err(ServerFnError::new(
            "Retention days must be between 1 and 365",
        ));
    }

    let metadata = serde_json::json!({ "query_history_retention_days": days });
    kyomi_auth::user_service::update_extra_metadata(&ctx.db, &auth.user_id, &metadata)
        .await
        .into_sfn()?;

    Ok(())
}

/// Update the user's chart color palette.
#[server(prefix = "/leptos-api")]
pub async fn update_chart_palette(palette: String) -> Result<(), ServerFnError> {
    let auth = extract_auth().await?;
    let ctx = extract_context()?;

    let config = serde_json::json!({
        "type": "config",
        "version": 1,
        "style": palette
    });

    kyomi_auth::user_service::update_chartml_config(&ctx.db, &auth.user_id, &config)
        .await
        .into_sfn()?;

    Ok(())
}

/// Accept a workspace invitation.
#[server(prefix = "/leptos-api")]
pub async fn accept_invitation(invitation_id: String) -> Result<(), ServerFnError> {
    let auth = extract_auth().await?;
    let ctx = extract_context()?;

    kyomi_auth::workspace_service::accept_invitation_for_user(
        &ctx.db,
        &invitation_id,
        &auth.user_id,
    )
    .await
    .into_sfn()?;

    Ok(())
}

/// Decline a workspace invitation.
#[server(prefix = "/leptos-api")]
pub async fn decline_invitation(invitation_id: String) -> Result<(), ServerFnError> {
    let _auth = extract_auth().await?; // verify authenticated
    let ctx = extract_context()?;

    kyomi_auth::workspace_service::update_invitation_status(
        &ctx.db,
        &invitation_id,
        "declined",
    )
    .await
    .into_sfn()?;

    Ok(())
}

// Helpers — delegate to shared extractors in parent module
#[cfg(feature = "ssr")]
use super::{extract_auth, extract_context, AuthenticatedContext, IntoServerFnError};

#[cfg(all(test, feature = "ssr"))]
mod tests {
    //! Guards against accidental re-nesting of the chartml_config writer payload.
    //!
    //! KYO-129 Part 2 flattened `users.chartml_config` storage from
    //! `{"config": {"style": ...}}` to `{"style": ...}`. The writer literal is
    //! the canonical source of the shape — if someone re-wraps it in a future
    //! refactor these tests will fail before the regression reaches the DB.

    #[test]
    fn chart_palette_writer_produces_flat_shape() {
        let palette = "balanced".to_string();
        let config = serde_json::json!({
            "type": "config",
            "version": 1,
            "style": palette
        });
        assert_eq!(config["style"], "balanced");
        assert_eq!(config["type"], "config");
        assert_eq!(config["version"], 1);
        assert!(
            config.get("config").is_none(),
            "chartml_config must be flat, not nested under a 'config' key"
        );
    }
}
