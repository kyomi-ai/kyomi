// SPDX-License-Identifier: AGPL-3.0-or-later

//! Shared UI components for the Leptos frontend.
//!
//! These components match the React shadcn/ui components in
//! `apps/frontend/src/components/ui/` exactly. Classes are copied
//! from the React source, not approximated.
//!
//! Design system: `docs/DESIGN_SYSTEM.md`

pub mod action_status;
pub mod alert;
pub mod badge;
pub mod button;
pub mod card;
pub mod confirm_dialog;
pub mod input;
pub mod label;
pub mod layout;
pub mod modal;
pub mod select;
pub mod skeleton;
pub mod status_badge;
pub mod switch;
pub mod theme;
pub mod toast;

// Re-export commonly used components
pub use action_status::ActionStatus;
pub use alert::{Alert, AlertDescription, AlertTitle, AlertVariant};
pub use badge::{Badge, BadgeVariant};
pub use button::{Button, ButtonSize, ButtonVariant};
pub use card::{Card, CardContent, CardDescription, CardFooter, CardHeader, CardTitle};
pub use confirm_dialog::ConfirmDialog;
pub use input::INPUT_CLASS;
pub use label::Label;
pub use layout::Layout;
pub use modal::{Modal, ModalSize};
pub use select::StyledSelect;
pub use skeleton::Skeleton;
pub use status_badge::{StatusBadge, StatusBadgeVariant};
pub use switch::Switch;
pub use theme::ThemeProvider;
