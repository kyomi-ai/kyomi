// SPDX-License-Identifier: AGPL-3.0-or-later

//! Settings pages — tab-based layout matching the React SettingsContent.

pub mod ai;
pub mod ai_models;
pub mod analytics;
pub mod billing;
pub mod datasources;
pub mod profile;
pub mod push_notifications;
pub mod security;
pub mod settings_shell;
pub mod team;
#[cfg(feature = "slack")]
pub mod slack_connection;
pub mod usage;
pub mod workspace;
