// SPDX-License-Identifier: AGPL-3.0-or-later

//! Server functions for the personal setup wizard.
//!
//! Provides a lightweight check to determine whether the user's workspace
//! already has any datasources configured, used by the setup wizard to
//! decide which step to show (connect data vs. connect AI tool).

use leptos::prelude::*;

#[cfg(feature = "ssr")]
use super::{AuthenticatedContext, IntoServerFnError};

/// Check whether the current workspace has any datasources.
///
/// Returns `true` if at least one datasource exists (active or inactive),
/// `false` otherwise. Used by the setup wizard to skip the "Connect Data"
/// step when datasources are already configured.
#[server(prefix = "/leptos-api")]
pub async fn check_has_datasources() -> Result<bool, ServerFnError> {
    let ac = AuthenticatedContext::extract().await?;

    let datasources = kyomi_auth::datasource_service::list_datasources(ac.db(), &ac.ws_id, false)
        .await
        .into_sfn()?;

    Ok(!datasources.is_empty())
}
