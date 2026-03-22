// SPDX-License-Identifier: AGPL-3.0-or-later

//! Dashboard components — rendering, layout, and interaction.

pub mod chart_info_modal;
pub mod history_panel;
pub mod markdown_renderer;
pub mod parameters;
pub mod save_dashboard_modal;

pub use chart_info_modal::ChartInfoModal;
pub use history_panel::HistoryPanel;
pub use markdown_renderer::MarkdownRenderer;
pub use parameters::DashboardParameters;
pub use save_dashboard_modal::SaveDashboardModal;
