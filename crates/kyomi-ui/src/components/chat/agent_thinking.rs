// SPDX-License-Identifier: AGPL-3.0-or-later

//! Agent Thinking UI Component
//!
//! Displays agent thinking events in an expandable/collapsible panel with a
//! live timer, auto-scroll, event icons, and multiple rendering variants.
//!
//! Ported from `apps/frontend/src/components/AgentThinking.jsx` (240 lines React).
//! CSS classes are copied verbatim from the React source.

use leptos::prelude::*;
use leptos_icons::Icon;

use super::thinking::{ThinkingEvent, TokenUsage};
use super::tool_schema_renderer;

/// Rendering variant for the agent thinking panel.
///
/// Matches React's `variant` prop: `"inset" | "header-bar" | "tab" | "default"`.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub enum ThinkingVariant {
    /// Deep inset — recessed behind the message.
    #[default]
    Inset,
    /// Slim bar at top that expands downward.
    HeaderBar,
    /// Tab sticking out from top-left.
    Tab,
    /// Plain card (original floating card).
    Default,
}

impl ThinkingVariant {
    /// Parse from a string prop value — matches React's string variant prop.
    pub fn parse(s: &str) -> Self {
        match s {
            "inset" => Self::Inset,
            "header-bar" => Self::HeaderBar,
            "tab" => Self::Tab,
            _ => Self::Default,
        }
    }
}

/// Get the Lucide icon for a thinking event type.
///
/// Matches React's `getEventIcon()` semantics with proper Lucide icons.
fn get_event_icon(event_type: &str) -> AnyView {
    match event_type {
        "agent_start" => view! { <Icon icon=icondata_lu::LuBot width="14" height="14"/> }.into_any(),
        "agent_thought" => view! { <Icon icon=icondata_lu::LuMessageCircle width="14" height="14"/> }.into_any(),
        "tool_execution_start" => view! { <Icon icon=icondata_lu::LuWrench width="14" height="14"/> }.into_any(),
        "tool_execution_end" => view! { <Icon icon=icondata_lu::LuCircleCheck width="14" height="14"/> }.into_any(),
        "agent_decision" => view! { <Icon icon=icondata_lu::LuTarget width="14" height="14"/> }.into_any(),
        "agent_complete" => view! { <Icon icon=icondata_lu::LuFlag width="14" height="14"/> }.into_any(),
        "error" => view! { <Icon icon=icondata_lu::LuTriangleAlert width="14" height="14"/> }.into_any(),
        _ => view! { <Icon icon=icondata_lu::LuFileText width="14" height="14"/> }.into_any(),
    }
}

/// Format a duration in milliseconds to a human-readable string.
///
/// Matches React's `formatDuration()` exactly:
/// - `< 1000ms` → `"Xms"`
/// - `>= 1000ms` → `"X.Xs"`
fn format_duration(duration_ms: u64) -> String {
    if duration_ms == 0 {
        return String::new();
    }
    if duration_ms < 1000 {
        format!("{}ms", duration_ms)
    } else {
        format!("{:.1}s", duration_ms as f64 / 1000.0)
    }
}

/// Format a timestamp string to HH:MM:SS.sss (24-hour, milliseconds).
///
/// Format a timestamp to `HH:MM:SS` (no milliseconds — the user doesn't
/// need that precision).
fn format_timestamp(timestamp: &str) -> String {
    if let Ok(dt) = chrono::DateTime::parse_from_rfc3339(timestamp) {
        dt.format("%H:%M:%S").to_string()
    } else if let Ok(dt) = chrono::NaiveDateTime::parse_from_str(timestamp, "%Y-%m-%dT%H:%M:%S%.f")
    {
        dt.format("%H:%M:%S").to_string()
    } else {
        timestamp.to_string()
    }
}

/// Strip emojis from text — they clash with the sparkly logo in the header.
///
/// Matches React's `stripEmojis()` — removes Unicode emoji ranges and trims.
fn strip_emojis(text: &str) -> String {
    text.chars()
        .filter(|c| {
            let cp = *c as u32;
            // Filter out common emoji ranges (matching the React regex ranges).
            // Broad ranges are listed first; specific sub-ranges within them are
            // omitted to avoid unreachable pattern warnings.
            !matches!(cp,
                0x1F300..=0x1F9FF |  // Misc Symbols, Emoticons, Transport, etc.
                0x2300..=0x23FF |     // Misc Technical (covers 231A-231B, 23CF, 23E9-23FA, 23F1-23F2)
                0x25AA..=0x25FE |     // Geometric shapes subset
                0x2600..=0x26FF |     // Misc Symbols (covers 2614-2615, 2648-2653, 267F, 2693, etc.)
                0x2700..=0x27BF |     // Dingbats (covers 2702, 2705, 2708-270D, 270F, 2712, etc.)
                0x2934..=0x2935 |
                0x2B05..=0x2B07 |
                0x2B1B..=0x2B1C |
                0x2B50 |
                0x2B55 |
                0x3030 |
                0x303D |
                0x3297 |
                0x3299 |
                0x200D |             // Zero-width joiner (used in emoji sequences)
                0xFE0F              // Variation selector (emoji presentation)
            )
        })
        .collect::<String>()
        .trim()
        .to_string()
}

/// Render tool schema data from a thinking event.
///
/// Delegates to `tool_schema_renderer::render_tool_schema()` which has dedicated
/// renderers for each tool type (query results, cost estimates, dashboards, etc.).
fn render_tool_schema(schema: serde_json::Value) -> impl IntoView {
    view! {
        <div class="mt-1.5 text-xs bg-muted rounded px-2 py-1.5">
            {tool_schema_renderer::render_tool_schema(schema)}
        </div>
    }
}

/// Agent Thinking UI component.
///
/// Displays agent thinking events in an expandable/collapsible panel with a
/// live timer while active, auto-scroll on new events, and variant-based styling.
///
/// Matches React's `AgentThinking` component exactly (layout, classes, behavior).
#[component]
pub fn AgentThinking(
    /// List of thinking events to display.
    #[prop(default = vec![])]
    thinking_events: Vec<ThinkingEvent>,
    /// Whether the agent is currently actively thinking.
    #[prop(default = false)]
    is_active: bool,
    /// Visual variant: "inset", "header-bar", "tab", or "default".
    #[prop(default = "inset")]
    variant: &'static str,
    /// Token usage information (prompt + completion counts).
    #[prop(optional)]
    token_usage: Option<TokenUsage>,
    /// Optional start time in milliseconds (from `js_sys::Date::now()`).
    /// When provided, the live timer measures elapsed time from this value
    /// instead of from the moment the component mounts. This prevents the
    /// timer from resetting when the parent re-renders and re-mounts the
    /// component with new events.
    #[prop(optional)]
    start_time_ms: Option<f64>,
) -> impl IntoView {
    let variant = ThinkingVariant::parse(variant);

    // -- State --
    let (is_expanded, set_is_expanded) = signal(false);
    let (elapsed_time, set_elapsed_time) = signal(0u64);

    // Refs for auto-scroll
    let thinking_end_ref = NodeRef::<leptos::html::Div>::new();
    let scroll_container_ref = NodeRef::<leptos::html::Div>::new();

    // -- Live timer: counts up while processing --
    // Uses gloo_timers::callback::Interval on WASM, no-op on SSR.
    #[cfg(target_arch = "wasm32")]
    {
        use send_wrapper::SendWrapper;

        let is_active = is_active;
        let set_elapsed_time = set_elapsed_time;

        // Store interval handle in a signal so we can clean up
        let interval_handle: StoredValue<Option<SendWrapper<gloo_timers::callback::Interval>>> =
            StoredValue::new(None);

        // Start/stop interval based on is_active
        if is_active {
            let start_time = start_time_ms.unwrap_or_else(js_sys::Date::now);
            let interval = gloo_timers::callback::Interval::new(100, move || {
                let now = js_sys::Date::now();
                set_elapsed_time.set((now - start_time) as u64);
            });
            interval_handle.set_value(Some(SendWrapper::new(interval)));
        }

        // Clean up interval on unmount
        on_cleanup(move || {
            interval_handle.set_value(None);
        });
    }

    // Suppress unused warnings on SSR
    #[cfg(not(target_arch = "wasm32"))]
    {
        let _ = (set_elapsed_time, start_time_ms);
    }

    // -- Auto-scroll: scroll to bottom on mount when expanded --
    // NOTE: `thinking_events` is a non-reactive `Vec`, so `events_len` is a plain
    // `usize` captured at construction time. This effect re-runs only when
    // `is_expanded` changes, not when new events arrive. This is intentional —
    // the parent re-mounts the component each time it passes a new Vec of events,
    // so the scroll-on-mount behavior is sufficient in practice.
    #[cfg(target_arch = "wasm32")]
    {
        let is_expanded = is_expanded;
        let events_len = thinking_events.len();
        let thinking_end_ref = thinking_end_ref;

        Effect::new(move |_| {
            if is_expanded.get() && events_len > 0 {
                // Use try_read_untracked to avoid panicking if AgentThinking is
                // unmounted before this Effect's microtask runs.
                if let Some(guard) = thinking_end_ref.try_read_untracked() {
                    if let Some(el) = guard.as_ref() {
                        let opts = web_sys::ScrollIntoViewOptions::new();
                        opts.set_behavior(web_sys::ScrollBehavior::Smooth);
                        el.scroll_into_view_with_scroll_into_view_options(&opts);
                    }
                }
            }
        });
    }

    // -- Derived data --
    let latest_event = thinking_events.last().cloned();
    let tool_executions_count = thinking_events
        .iter()
        .filter(|e| {
            e.event_type == "tool_execution_start"
                || e.event_type == "tool_execution_end"
                || e.event_type == "tool_start"
                || e.event_type == "tool_end"
        })
        .count();

    let total_duration = latest_event
        .as_ref()
        .and_then(|e| e.duration_ms)
        .unwrap_or(0);

    // Current title: only show while actively processing, strip emojis
    let current_title = if is_active {
        latest_event.as_ref().map(|e| strip_emojis(&e.title))
    } else {
        None
    };

    // Clone data for closures
    let current_title_for_view = current_title.clone();
    let events_for_list = thinking_events.clone();

    // -- Content renderer (shared across variants) --
    let render_content = move || {
        let current_title = current_title_for_view.clone();
        let events = events_for_list.clone();

        view! {
            // Header — always visible, clickable to expand/collapse
            <div
                class="flex items-center justify-between cursor-pointer py-1.5"
                on:click=move |_| set_is_expanded.update(|v| *v = !*v)
            >
                <div class="flex items-center gap-2 min-w-0 flex-1">
                    {if is_active {
                        view! {
                            <img src="/kyomi_animated_logo.svg" alt="Processing" class="w-4 h-4 flex-shrink-0" />
                        }.into_any()
                    } else {
                        view! {
                            <img src="/kyomi_small_logo.svg" alt="Thinking" class="w-4 h-4 flex-shrink-0" />
                        }.into_any()
                    }}
                    {current_title.map(|title| {
                        view! {
                            <span class="text-xs text-muted-foreground truncate">{title}</span>
                        }
                    })}
                </div>
                <div class="flex items-center gap-2 flex-shrink-0">
                    <span class="text-xs text-muted-foreground font-mono whitespace-nowrap">
                        {if is_active {
                            let tool_count = tool_executions_count;
                            view! {
                                <span>{move || format!("{} tools \u{2022} {}", tool_count, format_duration(elapsed_time.get()))}</span>
                            }.into_any()
                        } else {
                            let tool_count = tool_executions_count;
                            let duration = total_duration;
                            view! {
                                <span>{format!("{} tools \u{2022} {}", tool_count, format_duration(duration))}</span>
                            }.into_any()
                        }}
                    </span>
                    <Icon
                        icon=icondata_lu::LuChevronDown
                        attr:class=move || format!(
                            "w-3.5 h-3.5 text-muted-foreground transition-transform {}",
                            if is_expanded.get() { "rotate-180" } else { "" }
                        )
                        width="14"
                        height="14"
                    />
                </div>
            </div>

            // Expanded event list — simple rows, no nested cards
            <div
                node_ref=scroll_container_ref
                class="overflow-y-auto transition-all duration-200 ease-in-out"
                style=move || format!(
                    "max-height: {}; opacity: {}; scroll-behavior: smooth;",
                    if is_expanded.get() { "24rem" } else { "0" },
                    if is_expanded.get() { "1" } else { "0" },
                )
            >
                <div class="border-t border-border mt-1 pt-1">
                    {events.iter().map(|event| {
                        let icon = get_event_icon(&event.event_type);
                        let title = event.title.clone();
                        let description = event.description.clone();
                        let duration_ms = event.duration_ms;
                        let timestamp = format_timestamp(&event.timestamp);

                        // Extract schema from event.data for tool result rendering.
                        // Matches React: event.data?.schema → ToolSchemaRenderer
                        let schema = event.data.as_ref()
                            .and_then(|d| d.get("schema"))
                            .cloned();

                        view! {
                            <div class="flex items-start gap-2.5 py-2 px-1">
                                <div class="flex-shrink-0 mt-0.5 text-muted-foreground">
                                    {icon}
                                </div>
                                <div class="flex-1 min-w-0">
                                    <div class="flex items-center justify-between gap-2">
                                        <span class="text-xs font-medium text-foreground truncate">
                                            {title}
                                        </span>
                                        <span class="text-xs text-muted-foreground font-mono whitespace-nowrap flex-shrink-0">
                                            {duration_ms.map(|d| format!("{} ", format_duration(d))).unwrap_or_default()}
                                            {timestamp}
                                        </span>
                                    </div>
                                    {description.map(|desc| {
                                        view! {
                                            <p class="text-xs text-muted-foreground mt-0.5">
                                                {desc}
                                            </p>
                                        }
                                    })}
                                    // Tool schema/result rendering
                                    {schema.map(render_tool_schema)}
                                </div>
                            </div>
                        }
                    }).collect_view()}
                    <div node_ref=thinking_end_ref />
                </div>
            </div>
        }
    };

    // -- Token usage display (appended after thinking content if present) --
    let _token_usage = token_usage;

    // -- Variant rendering --
    // All variants now use a clean, minimal container. The Inset variant
    // (default for chat messages) no longer bleeds with bg-accent.
    match variant {
        ThinkingVariant::Inset => {
            // Subtle top section inside the message card — no background bleed
            view! {
                <div class="mb-3 pb-3 border-b border-border" data-testid="agent-thinking">
                    {render_content()}
                </div>
            }
            .into_any()
        }
        ThinkingVariant::HeaderBar => {
            // Slim bar at top that expands downward
            view! {
                <div class="mb-3 -mx-6 -mt-4 border-b border-border overflow-hidden" data-testid="agent-thinking">
                    <div class="py-1 px-4">
                        {render_content()}
                    </div>
                </div>
            }
            .into_any()
        }
        ThinkingVariant::Tab => {
            // Tab — same as header bar (tab variant was unused and had emoji)
            view! {
                <div class="mb-3 border-b border-border" data-testid="agent-thinking">
                    {render_content()}
                </div>
            }
            .into_any()
        }
        ThinkingVariant::Default => {
            // Plain section with bottom border
            view! {
                <div class="mb-3 pb-3 border-b border-border" data-testid="agent-thinking">
                    {render_content()}
                </div>
            }
            .into_any()
        }
    }
}
