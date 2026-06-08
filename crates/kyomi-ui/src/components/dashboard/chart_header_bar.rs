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

/// Format millisecond timestamp as relative time string against a caller-supplied "now".
///
/// Pure math — no clock reads — so the caller controls when the label re-evaluates.
/// The `ChartHeaderBar` reactive `now_ms` signal drives those re-evaluations.
fn format_relative_time(timestamp_ms: f64, now_ms: f64) -> String {
    let diff_ms = now_ms - timestamp_ms;
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

/// Format millisecond timestamp as a compact relative time string (clock icon companion).
///
/// Used by the compact timestamp tier (320–447px container width) alongside a
/// `CLOCK` icon. Returns a short label with no trailing "ago":
/// - `< 60s`  → "now"
/// - `< 60m`  → "{N}m"
/// - `< 24h`  → "{N}h"
/// - `>= 24h` → "{N}d"
fn format_relative_time_compact(timestamp_ms: f64, now_ms: f64) -> String {
    let diff_ms = now_ms - timestamp_ms;
    let diff_secs = (diff_ms / 1000.0) as i64;

    if diff_secs < 60 { return "now".to_string(); }
    let mins = diff_secs / 60;
    if mins < 60 { return format!("{mins}m"); }
    let hours = mins / 60;
    if hours < 24 { return format!("{hours}h"); }
    let days = hours / 24;
    format!("{days}d")
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
///
/// Uses Tailwind v4 container queries (driven by the `@container` ancestor on
/// `ChartHeaderBar`) so the chip and its label adapt to the chart card's
/// width rather than the viewport:
///
/// - Below `@xs` (< 320px container): the whole chip is hidden.
/// - `@xs`–`@md` (320–447px): the chip is visible with `short_label`.
/// - `@md+` (448px+): the chip is visible with the full `label`.
#[component]
fn ModifierChip(
    label: &'static str,
    active: bool,
    on_toggle: Callback<()>,
    /// Abbreviated label shown at the Compact tier (320–447px container).
    /// Falls back to `label` when not supplied.
    #[prop(optional, into)]
    short_label: Option<&'static str>,
) -> impl IntoView {
    let class = if active {
        "hidden @xs:inline-flex items-center px-2 py-0.5 text-xs font-medium rounded-full bg-accent text-foreground border border-border cursor-pointer transition-colors focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring"
    } else {
        "hidden @xs:inline-flex items-center px-2 py-0.5 text-xs font-medium rounded-full text-muted-foreground hover:bg-secondary/50 border border-transparent cursor-pointer transition-colors focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring"
    };

    let short = short_label.unwrap_or(label);

    view! {
        <button class=class on:click=move |_| on_toggle.run(())>
            <span class="hidden @md:inline">{label}</span>
            <span class="@md:hidden">{short}</span>
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

    // Outer `@container` div ref — observed by `ResizeObserver` below so the
    // portalled overflow menu can gate its Narrow-tier-only items off a real
    // signal instead of a container query. (Portals mount at `document.body`,
    // so `@xs:hidden` inside them has no `@container` ancestor and evaluates
    // against the viewport — which means it permanently hides those items on
    // every real screen.)
    let container_ref = NodeRef::<leptos::html::Div>::new();

    // Narrow tier = container width < 320px (`@xs` breakpoint). Starts `false`
    // so the first paint / SSR mirrors the toolbar-side icon visibility; the
    // ResizeObserver below corrects it synchronously after mount.
    let (is_narrow, set_is_narrow) = signal(false);

    // Store optional values for use in closures
    let ct = StoredValue::new(chart_type.clone());
    let co = StoredValue::new(chart_orientation.clone());
    let cm = StoredValue::new(chart_mode.clone());

    // Reactive "now" used by the "Last refreshed" label so it advances while the
    // chart sits untouched. Native/SSR builds get a stable zero — the client
    // hydrates and re-evaluates with a real clock. See timer Effect below.
    #[cfg(target_arch = "wasm32")]
    let (now_ms, set_now_ms) = signal(js_sys::Date::now());
    #[cfg(not(target_arch = "wasm32"))]
    let (now_ms, _) = signal(0.0_f64);

    // Wall-clock tick — re-evaluates the relative-time label every 30s while a
    // timestamp exists; matches React's `_startTimestampUpdater`. Effect re-runs
    // when `last_updated` toggles None↔Some so the interval is torn down once
    // there's nothing to display and (re)established when data arrives.
    #[cfg(target_arch = "wasm32")]
    if let Some(last_sig) = last_updated {
        Effect::new(move |_| {
            let Some(last_val) = last_sig.try_get() else { return };
            if last_val.is_none() {
                return;
            }
            use wasm_bindgen::prelude::*;
            use wasm_bindgen::JsCast;

            let Some(window) = web_sys::window() else { return };

            let closure = Closure::<dyn Fn()>::new(move || {
                set_now_ms.set(js_sys::Date::now());
            });

            let interval_id = window
                .set_interval_with_callback_and_timeout_and_arguments_0(
                    closure.as_ref().unchecked_ref(),
                    30_000,
                )
                .unwrap_or(0);

            let wrapper = send_wrapper::SendWrapper::new(closure);
            on_cleanup(move || {
                if let Some(window) = web_sys::window() {
                    window.clear_interval_with_handle(interval_id);
                }
                drop(wrapper);
            });
        });
    }

    // ResizeObserver on the outer `@container` div — drives `is_narrow` so the
    // portalled overflow menu can show Edit / Save to Dashboard / Ask about /
    // Chart Info at the Narrow tier. Container queries don't work inside a
    // portal (no `@container` ancestor once mounted at `document.body`), so
    // these items need a real Leptos signal instead of a CSS class.
    #[cfg(target_arch = "wasm32")]
    Effect::new(move |_| {
        use wasm_bindgen::prelude::*;
        use wasm_bindgen::JsCast;

        let Some(el) = container_ref.get() else {
            return;
        };
        let el: web_sys::Element = el.into();

        let closure = Closure::<dyn Fn(js_sys::Array)>::new(move |entries: js_sys::Array| {
            if let Some(entry) = entries.get(0).dyn_ref::<web_sys::ResizeObserverEntry>() {
                let width = entry.content_rect().width();
                set_is_narrow.set(width < 320.0);
            }
        });

        let Ok(observer) = web_sys::ResizeObserver::new(closure.as_ref().unchecked_ref()) else {
            return;
        };
        observer.observe(&el);

        let wrapper = send_wrapper::SendWrapper::new((closure, observer));
        on_cleanup(move || {
            let (_closure, observer) = wrapper.take();
            observer.disconnect();
        });
    });

    // Keep `container_ref` alive on non-wasm targets so SSR still compiles
    // (the ref is attached to the outermost div below).
    #[cfg(not(target_arch = "wasm32"))]
    let _ = (container_ref, set_is_narrow);

    // Determine which modifier chips to show based on chart type
    let show_orientation_chip = chart_type.as_deref() == Some("bar") && show_type_selector;
    let show_mode_chip = matches!(chart_type.as_deref(), Some("bar") | Some("area")) && show_type_selector;

    // Overflow menu holds Delete plus any secondary actions that collapse into
    // the kebab at the Narrow tier (< 320px container). Escape + click-outside
    // handling lives inside <Popover>.
    let has_menu_items =
        show_delete || show_edit || show_save_to_dashboard || show_ask_about || show_info || show_type_selector;

    // Kebab wrapper visibility:
    //  - Delete always needs the kebab (it lives there at every tier).
    //  - Without Delete, the kebab only exists to hold the Narrow-tier overflow
    //    (type selector + secondary action icons), so it is hidden from `@xs`
    //    upward where those items are visible in the toolbar.
    let kebab_wrapper_class = if show_delete {
        "flex"
    } else if show_type_selector || show_edit || show_save_to_dashboard || show_ask_about || show_info {
        "flex @xs:hidden"
    } else {
        "hidden"
    };

    view! {
        <div
            node_ref=container_ref
            class="@container flex items-center justify-between px-4 py-1 bg-secondary border-b border-border overflow-hidden"
        >
            // ── Left side: before slot + "Last refreshed ..." ──
            <div class="flex items-center gap-2 min-w-0">
                {before.map(|children| children())}

                // "Last refreshed ..." text — hidden below @md (< 448px container).
                // A compact clock + short label is shown at the @xs–@md tier (320–447px).
                {last_updated.map(|sig| view! {
                    // Full text: shown at @md+ (448px+).
                    <span class="hidden @md:inline text-xs text-muted-foreground truncate">
                        {move || {
                            let now = now_ms.try_get().unwrap_or(0.0);
                            let text = sig.try_get()
                                .flatten()
                                .map(|ts| format_relative_time(ts, now))
                                .unwrap_or_default();
                            if text.is_empty() {
                                String::new()
                            } else {
                                format!("Last refreshed {text}")
                            }
                        }}
                    </span>
                    // Compact clock + short label: shown at @xs–@md (320–447px).
                    <span
                        class="hidden @xs:flex @md:hidden items-center gap-1 text-xs text-muted-foreground"
                        title={move || {
                            let now = now_ms.try_get().unwrap_or(0.0);
                            sig.try_get()
                                .flatten()
                                .map(|ts| format!("Last refreshed {}", format_relative_time(ts, now)))
                                .unwrap_or_default()
                        }}
                    >
                        <Icon icon=phosphor_leptos::CLOCK size="14px" />
                        {move || {
                            let now = now_ms.try_get().unwrap_or(0.0);
                            sig.try_get()
                                .flatten()
                                .map(|ts| format_relative_time_compact(ts, now))
                                .unwrap_or_default()
                        }}
                    </span>
                })}
            </div>

            // ── Right side: type selector + modifier chips + action icons + overflow menu ──
            <div class="flex items-center gap-1 min-w-0">
                // Type selector — hidden below @xs (< 320px); visible from @xs up.
                // At narrow tier the user changes type via the overflow menu instead.
                {if show_type_selector {
                    ct.get_value().map(|current| {
                        let on_type = on_type_change.unwrap_or(Callback::new(|_| {}));
                        view! {
                            <div class="hidden @xs:flex">
                                <ChartTypeSelector current_type=current on_select=on_type />
                            </div>
                        }
                    })
                } else {
                    None
                }}

                // Orientation chip (bar charts)
                {show_orientation_chip.then(|| {
                    let is_horizontal = co.get_value().as_deref() == Some("horizontal");
                    let on_orient = on_orientation_change.unwrap_or(Callback::new(|_| {}));
                    view! {
                        <ModifierChip
                            label="Horizontal"
                            short_label="Horiz"
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
                    let (label, short_label, mode_value) = if chart_type_val == "area" {
                        ("Normalized", "Norm", "normalized")
                    } else {
                        ("Grouped", "Grpd", "grouped")
                    };
                    let is_active = current_mode.as_deref() == Some(mode_value);
                    let mode_str = mode_value.to_string();
                    view! {
                        <ModifierChip
                            label=label
                            short_label=short_label
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
                {if show_refresh {
                    on_refresh.map(|on_ref| {
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
                    })
                } else {
                    None
                }}

                // Save to dashboard button — hidden at Narrow tier; available in the kebab.
                {show_save_to_dashboard.then_some(on_save_to_dashboard).flatten().map(|cb| view! {
                    <div class="hidden @xs:flex">
                        <button
                            class="p-1.5 rounded-md hover:bg-secondary text-muted-foreground hover:text-foreground transition-colors focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring"
                            title="Save to Dashboard"
                            on:click=move |_| cb.run(())
                        >
                            <Icon icon=phosphor_leptos::PLUS_SQUARE size="16px" />
                        </button>
                    </div>
                })}

                // Ask about this chart — hidden at Narrow tier; available in the kebab.
                {show_ask_about.then_some(on_ask_about).flatten().map(|cb| view! {
                    <div class="hidden @xs:flex">
                        <button
                            class="p-1.5 rounded-md hover:bg-secondary text-muted-foreground hover:text-foreground transition-colors focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring"
                            title="Ask about this chart"
                            on:click=move |_| cb.run(())
                        >
                            <Icon icon=phosphor_leptos::CHATS size="16px" />
                        </button>
                    </div>
                })}

                // Info button — hidden at Narrow tier; available in the kebab.
                {show_info.then_some(on_info).flatten().map(|cb| view! {
                    <div class="hidden @xs:flex">
                        <button
                            class="p-1.5 rounded-md hover:bg-secondary text-muted-foreground hover:text-foreground transition-colors focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring"
                            title="Chart Info"
                            on:click=move |_| cb.run(())
                        >
                            <Icon icon=phosphor_leptos::INFO size="16px" />
                        </button>
                    </div>
                })}

                // Edit button — hidden at Narrow tier; available in the kebab.
                {show_edit.then_some(on_edit).flatten().map(|cb| view! {
                    <div class="hidden @xs:flex">
                        <button
                            class="p-1.5 rounded-md hover:bg-secondary text-muted-foreground hover:text-foreground transition-colors focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring"
                            title="Edit Chart"
                            on:click=move |_| cb.run(())
                        >
                            <Icon icon=phosphor_leptos::PENCIL_SIMPLE size="16px" />
                        </button>
                    </div>
                })}

                // Overflow menu. Holds Delete at every tier (when available) and
                // acts as the landing spot for Edit/Save/Ask/Info at the Narrow
                // tier (< 320px container), where those icons are hidden from the
                // toolbar. Secondary items are wrapped in `@xs:hidden` so they
                // only appear in the menu when they've been hidden outside it.
                {has_menu_items.then(|| {
                    let edit_cb = StoredValue::new(on_edit);
                    let save_cb = StoredValue::new(on_save_to_dashboard);
                    let ask_cb = StoredValue::new(on_ask_about);
                    let info_cb = StoredValue::new(on_info);
                    let delete_cb = StoredValue::new(on_delete);
                    let type_cb = StoredValue::new(on_type_change);

                    view! {
                        <div node_ref=menu_trigger_ref class=kebab_wrapper_class>
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
                                // Chart type list — visible in menu only at Narrow tier
                                // (< 320px), where the type selector is hidden in the toolbar.
                                // A separator below the list divides it from the action items.
                                {show_type_selector.then(|| {
                                    let current_ct = ct.get_value().unwrap_or_default();
                                    view! {
                                        <Show when=move || is_narrow.get()>
                                            {CHART_TYPES.iter().map(|(t, label)| {
                                                let chart_type = t.to_string();
                                                let ct_for_click = chart_type.clone();
                                                let ct_for_class = chart_type.clone();
                                                let is_active = current_ct == chart_type;
                                                let cb = type_cb.get_value();
                                                view! {
                                                    <button
                                                        class=if is_active {
                                                            "w-full flex items-center gap-2 px-3 py-2 text-sm text-foreground bg-accent"
                                                        } else {
                                                            "w-full flex items-center gap-2 px-3 py-2 text-sm text-popover-foreground transition-colors hover:bg-secondary"
                                                        }
                                                        on:click=move |_| {
                                                            if let Some(cb) = cb {
                                                                cb.run(ct_for_click.clone());
                                                            }
                                                            set_menu_open.set(false);
                                                        }
                                                    >
                                                        {icons::chart_type_icon(&ct_for_class)}
                                                        <span>{*label}</span>
                                                    </button>
                                                }
                                            }).collect_view()}
                                            // Separator between type items and action items
                                            {(show_edit || show_save_to_dashboard || show_ask_about || show_info || show_delete).then(|| view! {
                                                <div class="border-t border-border my-1" />
                                            })}
                                        </Show>
                                    }
                                })}

                                // Edit — visible in menu only at Narrow tier.
                                // `<Show>` gates on the `is_narrow` signal because
                                // the popover renders inside a Portal (mounted at
                                // `document.body`), so CSS container queries can't
                                // reach the `@container` ancestor from here.
                                {(show_edit).then(|| {
                                    edit_cb.get_value().map(|cb| view! {
                                        <Show when=move || is_narrow.get()>
                                            <button
                                                class="w-full flex items-center gap-2 px-3 py-2 text-sm text-popover-foreground transition-colors hover:bg-secondary"
                                                on:click=move |_| { cb.run(()); set_menu_open.set(false); }
                                            >
                                                <Icon icon=phosphor_leptos::PENCIL_SIMPLE size="14px" />
                                                <span>"Edit Chart"</span>
                                            </button>
                                        </Show>
                                    })
                                })}

                                // Save to Dashboard — visible in menu only at Narrow tier.
                                {(show_save_to_dashboard).then(|| {
                                    save_cb.get_value().map(|cb| view! {
                                        <Show when=move || is_narrow.get()>
                                            <button
                                                class="w-full flex items-center gap-2 px-3 py-2 text-sm text-popover-foreground transition-colors hover:bg-secondary"
                                                on:click=move |_| { cb.run(()); set_menu_open.set(false); }
                                            >
                                                <Icon icon=phosphor_leptos::PLUS_SQUARE size="14px" />
                                                <span>"Save to Dashboard"</span>
                                            </button>
                                        </Show>
                                    })
                                })}

                                // Ask about this chart — visible in menu only at Narrow tier.
                                {(show_ask_about).then(|| {
                                    ask_cb.get_value().map(|cb| view! {
                                        <Show when=move || is_narrow.get()>
                                            <button
                                                class="w-full flex items-center gap-2 px-3 py-2 text-sm text-popover-foreground transition-colors hover:bg-secondary"
                                                on:click=move |_| { cb.run(()); set_menu_open.set(false); }
                                            >
                                                <Icon icon=phosphor_leptos::CHATS size="14px" />
                                                <span>"Ask about this chart"</span>
                                            </button>
                                        </Show>
                                    })
                                })}

                                // Chart Info — visible in menu only at Narrow tier.
                                {(show_info).then(|| {
                                    info_cb.get_value().map(|cb| view! {
                                        <Show when=move || is_narrow.get()>
                                            <button
                                                class="w-full flex items-center gap-2 px-3 py-2 text-sm text-popover-foreground transition-colors hover:bg-secondary"
                                                on:click=move |_| { cb.run(()); set_menu_open.set(false); }
                                            >
                                                <Icon icon=phosphor_leptos::INFO size="14px" />
                                                <span>"Chart Info"</span>
                                            </button>
                                        </Show>
                                    })
                                })}

                                // Delete — always lives in the kebab when available.
                                {show_delete.then(|| {
                                    delete_cb.get_value().map(|cb| view! {
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
