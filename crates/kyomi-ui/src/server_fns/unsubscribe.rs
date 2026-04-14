// SPDX-License-Identifier: AGPL-3.0-or-later

//! Server function for email unsubscribe — public, no auth required.
//!
//! Mirrors `POST /api/v1/unsubscribe` in `apps/server/src/routes/subscribe.rs`.
//! Updates `marketing_consent = false` on the `email_subscribers` table.
//! Always returns success for privacy (does not reveal whether the email exists).

use leptos::prelude::*;

#[cfg(feature = "ssr")]
use super::extract_context;

/// Unsubscribe an email from marketing communications.
///
/// This is a public endpoint — no authentication required.
/// Always returns `Ok(())` for privacy, regardless of whether the email exists.
#[server(prefix = "/leptos-api")]
pub async fn unsubscribe_email(email: String) -> Result<(), ServerFnError> {
    let ctx = extract_context()?;

    let email = email.trim().to_lowercase();
    if email.is_empty() {
        // Return Ok for privacy — don't reveal whether email exists
        return Ok(());
    }

    let is_pg = ctx.db.is_postgres();
    let now_expr = kyomi_core::sql_compat::now(is_pg);
    let bf = kyomi_core::sql_compat::bool_false(is_pg);

    let sql = format!(
        "UPDATE email_subscribers \
         SET marketing_consent = {bf}, updated_at = {now_expr} \
         WHERE email = $1"
    );
    kyomi_core::db_execute!(&ctx.db, &sql, &email)
        .map_err(|e| ServerFnError::new(format!("Failed to unsubscribe: {e}")))?;

    tracing::info!(email = %email, "Unsubscribe processed via Leptos server fn");

    Ok(())
}
