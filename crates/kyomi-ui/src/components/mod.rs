// SPDX-License-Identifier: AGPL-3.0-or-later

//! Shared UI components for the Leptos frontend.
//!
//! These components follow the shadcn/ui design pattern and use
//! Kyomi's Tailwind design tokens. They are reusable across all pages.

pub mod action_status;
pub mod card;
pub mod confirm_dialog;
pub mod icons;
pub mod select;
pub mod theme;
pub mod toast;

// Re-export commonly used components for convenience
pub use action_status::ActionStatus;
pub use card::{Card, CardContent, CardDescription, CardHeader, CardTitle};
pub use confirm_dialog::ConfirmDialog;
pub use select::StyledSelect;
pub use theme::ThemeProvider;
