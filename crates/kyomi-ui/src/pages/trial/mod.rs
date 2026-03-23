// SPDX-License-Identifier: AGPL-3.0-or-later

//! Trial chat page — standalone page for anonymous users to explore sample data.
//!
//! This page is public (no authentication required) and provides a sandboxed
//! chat experience with a sample SaaS dataset. It matches
//! `apps/frontend/src/components/TrialChat.jsx`.

mod trial_chat;

pub use trial_chat::TrialChatPage;
