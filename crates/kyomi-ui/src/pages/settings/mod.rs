// SPDX-License-Identifier: AGPL-3.0-or-later

//! Settings pages — tab-based layout matching the React SettingsContent.

pub mod ai_provider;
pub mod profile;
pub mod push_notifications;
pub mod security;
pub mod settings_shell;
#[cfg(feature = "slack")]
pub mod slack_connection;
