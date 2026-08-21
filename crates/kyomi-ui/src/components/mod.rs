// SPDX-License-Identifier: AGPL-3.0-or-later

//! UI components for the Leptos frontend.
//!
//! Shared primitives (Select, Button, Card, etc.) live in `kyomi-ui-components`
//! and are re-exported here. App-specific components (chat, dashboard, layout,
//! etc.) live directly in this crate.
//!
//! Design system: `DESIGN.md`

// ── App-specific modules (not in kyomi-ui-components) ──────────────────────
pub mod chat;
pub mod dashboard;
pub mod documents;
pub mod feedback_modal;
pub mod invitation_status_bar;
pub mod layout;
pub mod right_panel;
pub mod skeleton;
pub mod watches;

// ── Re-export all shared primitives from kyomi-ui-components ───────────────
pub use kyomi_ui_components::components::action_status;
pub use kyomi_ui_components::components::alert;
pub use kyomi_ui_components::components::badge;
pub use kyomi_ui_components::components::button;
pub use kyomi_ui_components::components::card;
pub use kyomi_ui_components::components::checkbox;
pub use kyomi_ui_components::components::confirm_dialog;
pub use kyomi_ui_components::components::empty_state;
pub use kyomi_ui_components::components::input;
pub use kyomi_ui_components::components::label;
pub use kyomi_ui_components::components::modal;
pub use kyomi_ui_components::components::navigation_progress;
pub use kyomi_ui_components::components::popover;
pub use kyomi_ui_components::components::search_input;
pub use kyomi_ui_components::components::select;
pub use kyomi_ui_components::components::spinner;
pub use kyomi_ui_components::components::status_badge;
pub use kyomi_ui_components::components::status_bar;
pub use kyomi_ui_components::components::switch;
pub use kyomi_ui_components::components::theme;
pub use kyomi_ui_components::components::toast;
pub use kyomi_ui_components::components::tooltip;

// ── Re-export commonly used types ──────────────────────────────────────────
pub use kyomi_ui_components::components::{
    ActionStatus,
    Alert, AlertDescription, AlertTitle, AlertVariant,
    Badge, BadgeVariant,
    Button, ButtonLink, ButtonSize, ButtonVariant, ToggleButton,
    Card, CardContent, CardDescription, CardFooter, CardHeader, CardTitle,
    Checkbox,
    ConfirmDialog,
    EmptyState, EmptyStateVariant,
    INPUT_CLASS,
    Label,
    Modal, ModalSize,
    NavigationProgress,
    SearchInput,
    Select, StaticSelect,
    Skeleton,
    Spinner,
    StatusBadge, StatusBadgeVariant,
    StatusBar, StatusBarVariant,
    Switch,
    ThemeProvider,
    Tooltip,
};

// App-specific re-exports
pub use feedback_modal::FeedbackModal;
pub use invitation_status_bar::InvitationStatusBar;
pub use layout::{FeedbackAccessRequestHandle, Layout};
pub use right_panel::RightPanel;
pub use skeleton::{
    AlertsListSkeleton, DetailPageSkeleton, ListPageSkeleton, ModalListSkeleton,
    SettingsPageSkeleton,
};
