// SPDX-License-Identifier: AGPL-3.0-or-later

//! Server functions for the personal setup wizard.
//!
//! Provides a lightweight check to determine whether the user's workspace
//! already has any datasources configured, used by the setup wizard to
//! decide which step to show (connect data vs. connect AI tool).

use leptos::prelude::*;

#[cfg(feature = "ssr")]
use super::{extract_auth, extract_context, workspace_id};

/// Check whether the current workspace has any datasources.
///
/// Returns `true` if at least one datasource exists (active or inactive),
/// `false` otherwise. Used by the setup wizard to skip the "Connect Data"
/// step when datasources are already configured.
#[server(prefix = "/leptos-api")]
pub async fn check_has_datasources() -> Result<bool, ServerFnError> {
    let auth = extract_auth().await?;
    let ctx = extract_context()?;
    let ws_id = workspace_id(&auth)?;

    let datasources = kyomi_auth::datasource_service::list_datasources(&ctx.db, ws_id, false)
        .await
        .map_err(|e| ServerFnError::new(e.to_string()))?;

    Ok(!datasources.is_empty())
}
