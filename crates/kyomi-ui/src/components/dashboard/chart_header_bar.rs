// SPDX-License-Identifier: AGPL-3.0-or-later

//! Reusable chart header bar component.
//!
//! Displays a "Last refreshed" timestamp on the left, and a type selector,
//! modifier chips (orientation/mode), action buttons (refresh, save, info),
//! and an overflow menu on the right. Matches the React `<chart-header-bar>`
//! web component layout.

use leptos::prelude::*;
use phosphor_leptos::Icon;

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Chart types available in the type selector dropdown.
const CHART_TYPES: &[(&str, &str)] = &[
    ("bar", "Bar"),
    ("line", "Line"),
    ("area", "Area"),
    ("scatter", "Scatter"),
    ("pie", "Pie"),
    ("doughnut", "Doughnut"),
    ("table", "Table"),
    ("metric", "Metric"),
];

// ---------------------------------------------------------------------------
// Icons — chart-type glyphs (Phosphor for everything with a direct equivalent;
// `area` is a small hand-rolled SVG because phosphor-leptos 0.8 has no
// dedicated chart-area glyph. Its paths live in Phosphor's 256×256 viewBox
// and use the same stroke weight as the Regular-weight line icon so the two
// read as visually related in the dropdown.)
// ---------------------------------------------------------------------------

mod icons {
    use leptos::prelude::*;
    use phosphor_leptos::{Icon, IconWeight};

    /// Return the view for a chart type icon.
    pub fn chart_type_icon(chart_type: &str) -> impl IntoView + use<> {
        match chart_type {
            "bar" => view! {
                <Icon icon=phosphor_leptos::CHART_BAR weight=IconWeight::Regular size="16px" />
            }.into_any(),
            "line" => view! {
                <Icon icon=phosphor_leptos::CHART_LINE weight=IconWeight::Regular size="16px" />
            }.into_any(),
            // Hand-rolled area icon — line on top, filled polygon under the
            // curve. Paths use Phosphor's 256 viewBox + 16px stroke so it
            // visually matches the other Regular-weight chart icons.
            "area" => view! {
                <svg width="16" height="16" viewBox="0 0 256 256" fill="currentColor">
                    // Filled area under the curve (20% opacity)
                    <path
                        d="M40,164 L90,104 L160,148 L216,96 L216,200 L40,200 Z"
                        fill="currentColor"
                        opacity="0.2"
                    />
                    // Axis frame (L-shape: left + bottom) — matches Phosphor CHART_LINE styling
                    <path
                        d="M232,208a8,8,0,0,1-8,8H32a8,8,0,0,1-8-8V48a8,8,0,0,1,16,0V200H224A8,8,0,0,1,232,208Z"
                    />
                    // Curve on top
                    <path
                        d="M34.34,169.66a8,8,0,0,1,0-11.32l50-50a8,8,0,0,1,10.07-.38l61.43,46.07,55.51-51.43a8,8,0,0,1,10.88,11.74l-60,55.6a8,8,0,0,1-10.07.38L90.73,124.3,45.66,169.66A8,8,0,0,1,34.34,169.66Z"
                    />
                </svg>
            }.into_any(),
            "scatter" => view! {
                <Icon icon=phosphor_leptos::CHART_SCATTER weight=IconWeight::Regular size="16px" />
            }.into_any(),
            "pie" => view! {
                <Icon icon=phosphor_leptos::CHART_PIE weight=IconWeight::Regular size="16px" />
            }.into_any(),
            "doughnut" => view! {
                <Icon icon=phosphor_leptos::CHART_DONUT weight=IconWeight::Regular size="16px" />
            }.into_any(),
            "table" => view! {
                <Icon icon=phosphor_leptos::TABLE weight=IconWeight::Regular size="16px" />
            }.into_any(),
            "metric" => view! {
                <Icon icon=phosphor_leptos::GAUGE weight=IconWeight::Regular size="16px" />
            }.into_any(),
            _ => view! { <span /> }.into_any(),
        }
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Format millisecond timestamp as relative time string.
fn format_relative_time(timestamp_ms: f64) -> String {
    let now = js_sys::Date::now();
    let diff_ms = now - timestamp_ms;
    let diff_secs = (diff_ms / 1000.0) as i64;

    if diff_secs < 5 { return "just now".to_string(); }
    if diff_secs < 60 { return format!("{diff_secs}s ago"); }
    let mins = diff_secs / 60;
    if mins < 60 { return format!("{mins}m ago"); }
    let hours = mins / 60;
    if hours < 24 { return format!("{hours}h ago"); }
    let days = hours / 24;
    format!("{days}d ago")
}

// ---------------------------------------------------------------------------
// Sub-components
// ---------------------------------------------------------------------------

/// Chart type selector dropdown.
#[component]
fn ChartTypeSelector(
    current_type: String,
    on_select: Callback<String>,
) -> impl IntoView {
    let (open, set_open) = signal(false);
    let trigger_ref = NodeRef::<leptos::html::Div>::new();
    let current = StoredValue::new(current_type);

    // Icon-only type selector button — no text label, just icon + chevron.
    // Wrapped in a <div> that carries the trigger ref so Popover can measure
    // its bounding rect for positioning + outside-click detection.
    view! {
        <div node_ref=trigger_ref>
            <button
                class="flex items-center gap-1 px-2 py-1 text-xs font-medium rounded-md hover:bg-secondary text-muted-foreground hover:text-foreground transition-colors focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring"
                on:click=move |_| set_open.update(|v| *v = !*v)
            >
                {icons::chart_type_icon(&current.get_value())}
                <Icon icon=phosphor_leptos::CARET_DOWN size="12px" />
            </button>

            <crate::components::popover::Popover
                trigger_ref=trigger_ref
                open=Signal::derive(move || open.get())
                on_close=Callback::new(move |()| set_open.set(false))
                placement=crate::components::popover::Placement::BOTTOM_START
                class="w-40 bg-popover border border-border rounded-md shadow-lg py-1 overflow-y-auto"
            >
                {CHART_TYPES.iter().map(|(t, label)| {
                    let chart_type = t.to_string();
                    let ct_for_click = chart_type.clone();
                    let ct_for_class = chart_type.clone();
                    let is_active = current.get_value() == chart_type;
                    view! {
                        <button
                            class=if is_active {
                                "w-full flex items-center gap-2 px-3 py-1.5 text-sm text-foreground bg-accent"
                            } else {
                                "w-full flex items-center gap-2 px-3 py-1.5 text-sm text-popover-foreground transition-colors hover:bg-secondary"
                            }
                            on:click=move |_| {
                                on_select.run(ct_for_click.clone());
                                set_open.set(false);
                            }
                        >
                            {icons::chart_type_icon(&ct_for_class)}
                            <span>{*label}</span>
                        </button>
                    }
                }).collect_view()}
            </crate::components::popover::Popover>
        </div>
    }
}

/// Toggle chip for orientation/mode modifiers.
#[component]
fn ModifierChip(
    label: &'static str,
    active: bool,
    on_toggle: Callback<()>,
) -> impl IntoView {
    let class = if active {
        "px-2 py-0.5 text-xs font-medium rounded-full bg-accent text-foreground border border-border cursor-pointer transition-colors focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring"
    } else {
        "px-2 py-0.5 text-xs font-medium rounded-full text-muted-foreground hover:bg-secondary/50 border border-transparent cursor-pointer transition-colors focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring"
    };

    view! {
        <button class=class on:click=move |_| on_toggle.run(())>
            {label}
        </button>
    }
}

// ---------------------------------------------------------------------------
// Main component
// ---------------------------------------------------------------------------

/// Chart header bar with "Last refreshed" on the left, type selector, modifier
/// chips, and action buttons (refresh, save, info) plus overflow menu on the
/// right. Matches the React `<chart-header-bar>` web component layout:
///
/// ```text
/// [Last refreshed just now]     [type-icon ▾] [Horizontal] [Grouped]  [refresh] [save] [info] [⋮]
/// ```
///
/// Used by:
/// - Dashboard viewer (`ChartBlock` in `markdown_renderer.rs`)
/// - MCP chart app (`app.rs`)
#[component]
pub fn ChartHeaderBar(
    /// Timestamp (ms since epoch) of last data refresh.
    #[prop(optional, into)]
    last_updated: Option<Signal<Option<f64>>>,

    /// Whether a refresh is in progress (spins the refresh icon).
    #[prop(optional, into)]
    is_refreshing: Option<Signal<bool>>,

    /// Current chart type (bar, line, area, etc.).
    #[prop(optional, into)]
    chart_type: Option<String>,

    /// Current chart orientation (Some("horizontal") or None for vertical).
    #[prop(optional, into)]
    chart_orientation: Option<String>,

    /// Current chart mode (Some("stacked"|"grouped"|"normalized") or None).
    #[prop(optional, into)]
    chart_mode: Option<String>,

    /// Show the chart type selector dropdown.
    #[prop(optional)]
    show_type_selector: bool,

    /// Show the refresh button.
    #[prop(optional)]
    show_refresh: bool,

    // -- Feature flags for menu items --
    #[prop(optional)] show_edit: bool,
    #[prop(optional)] show_delete: bool,
    #[prop(optional)] show_save_to_dashboard: bool,
    #[prop(optional)] show_info: bool,
    #[prop(optional)] show_ask_about: bool,

    // -- Callbacks --
    #[prop(optional)] on_refresh: Option<Callback<()>>,
    #[prop(optional)] on_type_change: Option<Callback<String>>,
    #[prop(optional)] on_orientation_change: Option<Callback<Option<String>>>,
    #[prop(optional)] on_mode_change: Option<Callback<Option<String>>>,
    #[prop(optional)] on_edit: Option<Callback<()>>,
    #[prop(optional)] on_delete: Option<Callback<()>>,
    #[prop(optional)] on_save_to_dashboard: Option<Callback<()>>,
    #[prop(optional)] on_info: Option<Callback<()>>,
    #[prop(optional)] on_ask_about: Option<Callback<()>>,

    /// Slot: content rendered before the left-side content (e.g., drag handle).
    #[prop(optional)]
    before: Option<Children>,
) -> impl IntoView {
    let (menu_open, set_menu_open) = signal(false);
    let menu_trigger_ref = NodeRef::<leptos::html::Div>::new();

    // Store optional values for use in closures
    let ct = StoredValue::new(chart_type.clone());
    let co = StoredValue::new(chart_orientation.clone());
    let cm = StoredValue::new(chart_mode.clone());

    // Determine which modifier chips to show based on chart type
    let show_orientation_chip = chart_type.as_deref() == Some("bar") && show_type_selector;
    let show_mode_chip = matches!(chart_type.as_deref(), Some("bar") | Some("area")) && show_type_selector;

    // Overflow menu only for Delete (Edit is a direct icon button now).
    // Escape + click-outside handling lives inside <Popover>.
    let has_menu_items = show_delete;

    view! {
        <div class="flex items-center justify-between px-4 py-1 bg-secondary border-b border-border">
            // ── Left side: before slot + "Last refreshed ..." ──
            <div class="flex items-center gap-2 min-w-0">
                {before.map(|children| children())}

                // "Last refreshed ..." text
                {last_updated.map(|sig| view! {
                    <span class="text-xs text-muted-foreground truncate">
                        {move || {
                            let text = sig.get().map(format_relative_time).unwrap_or_default();
                            if text.is_empty() {
                                String::new()
                            } else {
                                format!("Last refreshed {text}")
                            }
                        }}
                    </span>
                })}
            </div>

            // ── Right side: type selector + modifier chips + action icons + overflow menu ──
            <div class="flex items-center gap-1 flex-shrink-0">
                // Type selector
                {(show_type_selector && ct.get_value().is_some()).then(|| {
                    let current = ct.get_value().unwrap();
                    let on_type = on_type_change.unwrap_or(Callback::new(|_| {}));
                    view! { <ChartTypeSelector current_type=current on_select=on_type /> }
                })}

                // Orientation chip (bar charts)
                {show_orientation_chip.then(|| {
                    let is_horizontal = co.get_value().as_deref() == Some("horizontal");
                    let on_orient = on_orientation_change.unwrap_or(Callback::new(|_| {}));
                    view! {
                        <ModifierChip
                            label="Horizontal"
                            active=is_horizontal
                            on_toggle=Callback::new(move |()| {
                                if is_horizontal {
                                    on_orient.run(None);
                                } else {
                                    on_orient.run(Some("horizontal".to_string()));
                                }
                            })
                        />
                    }
                })}

                // Mode chip (bar: grouped, area: normalized)
                {show_mode_chip.then(|| {
                    let chart_type_val = ct.get_value().unwrap_or_default();
                    let current_mode = cm.get_value();
                    let on_m = on_mode_change.unwrap_or(Callback::new(|_| {}));
                    let (label, mode_value) = if chart_type_val == "area" {
                        ("Normalized", "normalized")
                    } else {
                        ("Grouped", "grouped")
                    };
                    let is_active = current_mode.as_deref() == Some(mode_value);
                    let mode_str = mode_value.to_string();
                    view! {
                        <ModifierChip
                            label=label
                            active=is_active
                            on_toggle=Callback::new(move |()| {
                                if is_active {
                                    on_m.run(None);
                                } else {
                                    on_m.run(Some(mode_str.clone()));
                                }
                            })
                        />
                    }
                })}

                // Refresh button
                {(show_refresh && on_refresh.is_some()).then(|| {
                    let on_ref = on_refresh.unwrap();
                    let spin_class = move || {
                        let spinning = is_refreshing
                            .map(|s| s.get())
                            .unwrap_or(false);
                        if spinning {
                            "p-1.5 rounded-md hover:bg-secondary text-muted-foreground hover:text-foreground transition-colors focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring animate-spin"
                        } else {
                            "p-1.5 rounded-md hover:bg-secondary text-muted-foreground hover:text-foreground transition-colors focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring"
                        }
                    };
                    view! {
                        <button
                            class=spin_class
                            title="Refresh"
                            on:click=move |_| on_ref.run(())
                        >
                            <Icon icon=phosphor_leptos::ARROWS_CLOCKWISE size="16px" />
                        </button>
                    }
                })}

                // Save to dashboard button (direct icon, not in overflow menu)
                {(show_save_to_dashboard && on_save_to_dashboard.is_some()).then(|| {
                    let cb = on_save_to_dashboard.unwrap();
                    view! {
                        <button
                            class="p-1.5 rounded-md hover:bg-secondary text-muted-foreground hover:text-foreground transition-colors focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring"
                            title="Save to Dashboard"
                            on:click=move |_| cb.run(())
                        >
                            <Icon icon=phosphor_leptos::PLUS_SQUARE size="16px" />
                        </button>
                    }
                })}

                // Ask about this chart button (direct icon, matches React)
                {(show_ask_about && on_ask_about.is_some()).then(|| {
                    let cb = on_ask_about.unwrap();
                    view! {
                        <button
                            class="p-1.5 rounded-md hover:bg-secondary text-muted-foreground hover:text-foreground transition-colors focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring"
                            title="Ask about this chart"
                            on:click=move |_| cb.run(())
                        >
                            <Icon icon=phosphor_leptos::CHATS size="16px" />
                        </button>
                    }
                })}

                // Info button (direct icon, not in overflow menu)
                {(show_info && on_info.is_some()).then(|| {
                    let cb = on_info.unwrap();
                    view! {
                        <button
                            class="p-1.5 rounded-md hover:bg-secondary text-muted-foreground hover:text-foreground transition-colors focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring"
                            title="Chart Info"
                            on:click=move |_| cb.run(())
                        >
                            <Icon icon=phosphor_leptos::INFO size="16px" />
                        </button>
                    }
                })}

                // Edit button (direct icon, rightmost action)
                {(show_edit && on_edit.is_some()).then(|| {
                    let cb = on_edit.unwrap();
                    view! {
                        <button
                            class="p-1.5 rounded-md hover:bg-secondary text-muted-foreground hover:text-foreground transition-colors focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring"
                            title="Edit Chart"
                            on:click=move |_| cb.run(())
                        >
                            <Icon icon=phosphor_leptos::PENCIL_SIMPLE size="16px" />
                        </button>
                    }
                })}

                // Overflow menu (Delete only — shows kebab when delete is available)
                {has_menu_items.then(|| {
                    let delete_cb = StoredValue::new(on_delete);

                    view! {
                        <div node_ref=menu_trigger_ref>
                            <button
                                class="p-1.5 rounded-md hover:bg-secondary text-muted-foreground hover:text-foreground transition-colors focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring"
                                title="Actions"
                                on:click=move |_| set_menu_open.update(|v| *v = !*v)
                            >
                                <Icon icon=phosphor_leptos::DOTS_THREE_VERTICAL size="16px" />
                            </button>

                            <crate::components::popover::Popover
                                trigger_ref=menu_trigger_ref
                                open=Signal::derive(move || menu_open.get())
                                on_close=Callback::new(move |()| set_menu_open.set(false))
                                placement=crate::components::popover::Placement::BOTTOM_END
                                class="w-48 bg-popover border border-border rounded-md shadow-lg py-1 overflow-y-auto"
                            >
                                {show_delete.then(|| {
                                    let cb = delete_cb.get_value();
                                    cb.map(|cb| view! {
                                        <button
                                            class="w-full text-left px-3 py-2 text-sm text-destructive transition-colors hover:bg-destructive/10"
                                            on:click=move |_| { cb.run(()); set_menu_open.set(false); }
                                        >"Delete"</button>
                                    })
                                })}
                            </crate::components::popover::Popover>
                        </div>
                    }
                })}
            </div>
        </div>
    }
}
