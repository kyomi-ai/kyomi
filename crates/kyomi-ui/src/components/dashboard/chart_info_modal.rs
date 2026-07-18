// SPDX-License-Identifier: AGPL-3.0-or-later

//! Chart info modal — displays datasource, optional SQL query, and full
//! ChartML YAML source, matching React's ChartInfoModal component exactly.

use leptos::prelude::*;
use crate::components::modal::{Modal, ModalSize};
use crate::components::dashboard::highlighted_code_block::HighlightedCodeBlock;

// ─── Helpers ────────────────────────────────────────────────────────────────

/// Extract the datasource name from a raw ChartML YAML string.
///
/// Matches React: `spec.data?.datasource || spec.data?.source || 'Not specified'`
fn extract_datasource(yaml: &str) -> String {
    serde_yaml::from_str::<serde_yaml::Value>(yaml)
        .ok()
        .and_then(|v| {
            let data = v.get("data")?;
            data.get("datasource")
                .or_else(|| data.get("source"))
                .and_then(|s| s.as_str())
                .map(|s| s.to_string())
        })
        .unwrap_or_else(|| "Not specified".to_string())
}

/// Extract the SQL query from a raw ChartML YAML string, if present.
///
/// Matches React: `spec.data?.query || null`
fn extract_query(yaml: &str) -> Option<String> {
    serde_yaml::from_str::<serde_yaml::Value>(yaml)
        .ok()
        .and_then(|v| {
            v.get("data")
                .and_then(|d| d.get("query"))
                .and_then(|q| q.as_str())
                .map(|s| s.trim().to_string())
        })
        .filter(|s| !s.is_empty())
}

// ─── Component ───────────────────────────────────────────────────────────────

/// Modal that displays chart source information.
///
/// Shows datasource, optional SQL query, and full ChartML YAML source —
/// matching React's `ChartInfoModal` component (title "Chart Info", three
/// sections, no footer close button, no line numbers).
#[component]
pub fn ChartInfoModal(
    /// Whether the modal is open.
    #[prop(into)]
    open: Signal<bool>,
    /// The raw ChartML YAML spec to display (reactive signal).
    #[prop(into)]
    yaml: Signal<String>,
    /// Callback to close the modal.
    on_close: Callback<()>,
) -> impl IntoView {
    let datasource = Memo::new(move |_| extract_datasource(&yaml.get()));
    let query = Memo::new(move |_| extract_query(&yaml.get()));

    view! {
        <Modal
            show=open
            on_close=on_close
            title="Chart Info"
            size=ModalSize::Lg
        >
            <div class="space-y-6">
                // Datasource section — always shown
                // Matches React: <span>Datasource</span> + <p>{datasource}</p>
                <div>
                    <span class="text-sm font-medium text-foreground">"Datasource"</span>
                    <p class="mt-1 text-sm text-muted-foreground font-mono bg-muted px-3 py-2 rounded-md">
                        {move || datasource.get()}
                    </p>
                </div>

                // SQL Query section — only shown when present
                // Uses HighlightedCodeBlock for syntax highlighting with arborium
                {move || query.get().map(|sql| {
                    let sql_signal = Signal::derive(move || sql.clone());
                    view! {
                        <HighlightedCodeBlock
                            code=sql_signal
                            language=Signal::stored("sql".to_string())
                            label=Signal::stored("SQL Query".to_string())
                        />
                    }
                })}

                // ChartML Source section — always shown
                // Uses HighlightedCodeBlock for YAML syntax highlighting
                <HighlightedCodeBlock
                    code=yaml
                    language=Signal::stored("yaml".to_string())
                    label=Signal::stored("ChartML Source".to_string())
                />
            </div>
        </Modal>
    }
}
