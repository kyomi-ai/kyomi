// SPDX-License-Identifier: AGPL-3.0-or-later

//! Dashboard components — rendering, layout, and interaction.

pub mod chart_builder;
pub mod chart_header_bar;
pub mod chart_info_modal;
pub mod chartml_completion;
pub mod chartml_extension;
pub mod copilot_sidebar;
pub mod history_panel;
pub mod insert_link_modal;
pub mod markdown_renderer;
pub mod parameters;
pub mod save_dashboard_modal;
pub(crate) mod shared;
pub mod source_cache;

pub use chart_builder::ChartBuilderModal;
pub use chart_info_modal::ChartInfoModal;
pub use copilot_sidebar::CopilotSidebar;
pub use history_panel::HistoryPanel;
pub use insert_link_modal::InsertDashboardLinkModal;
pub use markdown_renderer::MarkdownRenderer;
pub use parameters::DashboardParameters;
pub use save_dashboard_modal::SaveDashboardModal;
pub use source_cache::DashboardSourceCache;
