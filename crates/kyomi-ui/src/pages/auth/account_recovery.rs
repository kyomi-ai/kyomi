// SPDX-License-Identifier: AGPL-3.0-or-later

//! Account Recovery page.
//!
//! Route: `/account/recover`
//!
//! Thin wrapper over the shared `<RecoveryRequestCard>` — see
//! `components/recovery_request_card.rs` for the full implementation.

use leptos::prelude::*;

use crate::pages::auth::components::{RecoveryKind, RecoveryRequestCard};

#[component]
pub fn AccountRecoveryPage() -> impl IntoView {
    view! { <RecoveryRequestCard kind=RecoveryKind::Account/> }
}
