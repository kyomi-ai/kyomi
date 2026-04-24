// SPDX-License-Identifier: AGPL-3.0-or-later

//! Shared UI components — no server-side dependencies.

pub mod action_status;
pub mod alert;
pub mod badge;
pub mod button;
pub mod card;
pub mod checkbox;
pub mod confirm_dialog;
pub mod dashboard;
pub mod empty_state;
pub mod input;
pub mod label;
pub mod modal;
pub mod navigation_progress;
pub mod popover;
pub mod search_input;
pub mod select;
pub mod skeleton;
pub mod spinner;
pub mod status_badge;
pub mod switch;
pub mod theme;
pub mod toast;
pub mod tooltip;

pub use action_status::ActionStatus;
pub use alert::{Alert, AlertDescription, AlertTitle, AlertVariant};
pub use badge::{Badge, BadgeVariant};
pub use button::{Button, ButtonLink, ButtonSize, ButtonVariant, ToggleButton};
pub use card::{Card, CardContent, CardDescription, CardFooter, CardHeader, CardTitle};
pub use checkbox::Checkbox;
pub use confirm_dialog::ConfirmDialog;
pub use empty_state::{EmptyState, EmptyStateVariant};
pub use input::INPUT_CLASS;
pub use label::Label;
pub use modal::{Modal, ModalSize};
pub use navigation_progress::NavigationProgress;
pub use search_input::SearchInput;
pub use select::{DynSelect, StyledSelect};
pub use skeleton::Skeleton;
pub use spinner::Spinner;
pub use status_badge::{StatusBadge, StatusBadgeVariant};
pub use switch::Switch;
pub use theme::ThemeProvider;
pub use tooltip::Tooltip;
