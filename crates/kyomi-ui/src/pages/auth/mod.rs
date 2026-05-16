// SPDX-License-Identifier: AGPL-3.0-or-later

//! Authentication pages — login, signup, password recovery.

pub mod account_recovery;
pub mod account_recovery_complete;
pub mod auth_layout;
pub mod components;
pub mod datasource_oauth_callback;
pub mod google_callback;
pub mod google_link_callback;
pub mod login;
pub mod oauth_complete;
pub mod passkey_recovery;
pub mod passkey_recovery_complete;
pub mod passkey_signup_complete;
pub mod signup_complete;
