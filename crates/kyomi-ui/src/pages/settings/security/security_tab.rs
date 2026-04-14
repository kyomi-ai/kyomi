// SPDX-License-Identifier: AGPL-3.0-or-later

//! Security settings tab — assembles all security sub-components.
//!
//! Matches the Security tab in `apps/frontend/src/pages/SettingsContent.jsx` lines 274-290.
//! Structure: heading + 4 sub-components in a vertical stack.

use leptos::prelude::*;

use super::password_manager::PasswordManager;
use super::two_factor_auth::TwoFactorAuth;
use super::session_management::SessionManagement;
use super::passkey_manager::PasskeyManager;

/// Security settings tab content.
///
/// React: inline in SettingsContent.jsx (not a separate component).
/// Shows: PasswordManager, TwoFactorAuth, PasskeyManager, SessionManagement.
#[component]
pub fn SecurityTab() -> impl IntoView {
    view! {
        <div class="p-6">
            <h2 class="text-xl font-display text-foreground mb-6">"Security"</h2>
            <div class="space-y-6">
                <PasswordManager/>
                <TwoFactorAuth/>
                <PasskeyManager/>
                <SessionManagement/>
            </div>
        </div>
    }
}
