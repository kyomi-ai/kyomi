// SPDX-License-Identifier: AGPL-3.0-or-later

//! Agent Thinking UI Component
//!
//! Displays agent thinking events in an expandable/collapsible panel with a
//! live timer, auto-scroll, event icons, and multiple rendering variants.
//!
//! Ported from `apps/frontend/src/components/AgentThinking.jsx` (240 lines React).
//! CSS classes are copied verbatim from the React source.

use leptos::prelude::*;
use phosphor_leptos::Icon;
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
        "agent_start" => view! { <Icon icon=phosphor_leptos::ROBOT size="14px"/> }.into_any(),
        "agent_thought" => view! { <Icon icon=phosphor_leptos::CHAT_CIRCLE size="14px"/> }.into_any(),
        "tool_execution_start" => view! { <Icon icon=phosphor_leptos::WRENCH size="14px"/> }.into_any(),
        "tool_execution_end" => view! { <Icon icon=phosphor_leptos::CHECK_CIRCLE size="14px"/> }.into_any(),
        "agent_decision" => view! { <Icon icon=phosphor_leptos::TARGET size="14px"/> }.into_any(),
        "agent_complete" => view! { <Icon icon=phosphor_leptos::FLAG size="14px"/> }.into_any(),
        "error" => view! { <Icon icon=phosphor_leptos::WARNING size="14px"/> }.into_any(),
        _ => view! { <Icon icon=phosphor_leptos::FILE_TEXT size="14px"/> }.into_any(),
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

/// Format a token count with comma separators (e.g., 12345 → "12,345").
fn format_token_count(n: u64) -> String {
    if n == 0 {
        return "0".to_string();
    }
    let s = n.to_string();
    let mut result = String::new();
    for (i, c) in s.chars().rev().enumerate() {
        if i > 0 && i % 3 == 0 {
            result.push(',');
        }
        result.push(c);
    }
    result.chars().rev().collect()
}

/// Format a cost value for display.
///
/// Sub-dollar amounts use 4 decimal places (`$0.0102`).
/// Amounts >= $1.00 use 2 decimal places (`$1.23`).
fn format_cost(cost: f64) -> String {
    if cost >= 1.0 {
        format!("${:.2}", cost)
    } else {
        format!("${:.4}", cost)
    }
}

/// Agent Thinking UI component.
///
/// Displays agent thinking events in an expandable/collapsible panel with a
/// live timer while active, auto-scroll on new events, and variant-based styling.
///
/// Matches React's `AgentThinking` component exactly (layout, classes, behavior).
///
/// Props accept either `Signal<T>` directly or plain `T` values — Leptos's
/// `#[prop(into)]` converts plain values via `Signal::stored(value)`, so
/// callers that pass static data (e.g. `execution_log_viewer`) need no changes.
#[component]
pub fn AgentThinking(
    /// List of thinking events to display.
    #[prop(into, default = Signal::stored(vec![]))]
    thinking_events: Signal<Vec<ThinkingEvent>>,
    /// Whether the agent is currently actively thinking.
    #[prop(into, default = Signal::stored(false))]
    is_active: Signal<bool>,
    /// Visual variant: "inset", "header-bar", "tab", or "default".
    #[prop(default = "inset")]
    variant: &'static str,
    /// Token usage information (prompt + completion counts).
    #[prop(into, default = Signal::stored(None))]
    token_usage: Signal<Option<TokenUsage>>,
    /// Whether to show token count and cost in the metadata bar.
    #[prop(into, default = false.into())]
    show_token_usage: Signal<bool>,
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

    // -- Live timer: reactive, starts/stops based on is_active signal --
    // Uses gloo_timers::callback::Interval on WASM, no-op on SSR.
    #[cfg(target_arch = "wasm32")]
    {
        use send_wrapper::SendWrapper;

        let interval_handle: StoredValue<Option<SendWrapper<gloo_timers::callback::Interval>>> =
            StoredValue::new(None);
        let timer_start: StoredValue<Option<f64>> = StoredValue::new(start_time_ms);

        Effect::new(move |_| {
            if is_active.get() {
                let already_running = interval_handle.with_value(|h| h.is_some());
                if !already_running {
                    let start = timer_start.get_value().unwrap_or_else(|| {
                        let now = js_sys::Date::now();
                        timer_start.set_value(Some(now));
                        now
                    });
                    let interval = gloo_timers::callback::Interval::new(100, move || {
                        let now = js_sys::Date::now();
                        set_elapsed_time.try_set((now - start) as u64);
                    });
                    interval_handle.set_value(Some(SendWrapper::new(interval)));
                }
            } else {
                interval_handle.set_value(None);
            }
        });

        on_cleanup(move || {
            interval_handle.set_value(None);
        });
    }

    // Suppress unused warnings on SSR
    #[cfg(not(target_arch = "wasm32"))]
    {
        let _ = (set_elapsed_time, start_time_ms);
    }

    // -- Auto-scroll: scroll to bottom when events change or panel expands --
    #[cfg(target_arch = "wasm32")]
    {
        Effect::new(move |_| {
            let events = thinking_events.get();
            if is_expanded.get() && !events.is_empty() {
                // Scroll the inner container to the bottom — NOT the page.
                // Using scrollIntoView on the sentinel would scroll the entire
                // page, which is jarring. Instead, set scrollTop on the
                // overflow container directly.
                if let Some(guard) = scroll_container_ref.try_read_untracked()
                    && let Some(el) = guard.as_ref() {
                        el.set_scroll_top(el.scroll_height());
                    }
            }
        });
    }

    // -- Derived data --
    let latest_event = Signal::derive(move || thinking_events.get().last().cloned());

    let tool_executions_count = Signal::derive(move || {
        thinking_events.get().iter().filter(|e| {
            e.event_type == "tool_execution_start"
                || e.event_type == "tool_execution_end"
                || e.event_type == "tool_start"
                || e.event_type == "tool_end"
        }).count()
    });

    let total_duration = Signal::derive(move || {
        latest_event.get()
            .and_then(|e| e.duration_ms)
            .unwrap_or(0)
    });

    // Current title: only show while actively processing, strip emojis
    let current_title = Signal::derive(move || {
        if is_active.get() {
            latest_event.get().map(|e| strip_emojis(&e.title))
        } else {
            None
        }
    });

    // -- Token usage suffix for the metadata bar --
    // Reactive: re-evaluates when show_token_usage or token_usage changes.
    // This is the core fix for the reactivity bug: token_usage arrives via
    // WebSocket after the component mounts, so the suffix must be a derived
    // Signal rather than a static String computed at mount time.
    let token_suffix = Signal::derive(move || {
        if show_token_usage.get() {
            token_usage
                .get()
                .filter(|tu| tu.total_tokens > 0)
                .map(|tu| {
                    format!(
                        " \u{2022} {} tokens \u{2022} {}",
                        format_token_count(tu.total_tokens),
                        format_cost(tu.cost)
                    )
                })
                .unwrap_or_default()
        } else {
            String::new()
        }
    });

    // Unified display duration — elapsed when active, total when complete.
    let display_duration = Signal::derive(move || {
        if is_active.get() { elapsed_time.get() } else { total_duration.get() }
    });

    // -- Content renderer (shared across variants) --
    let render_content = move || {
        view! {
            // Header — always visible, clickable to expand/collapse
            <div
                class="flex items-center justify-between cursor-pointer h-8"
                on:click=move |_| set_is_expanded.update(|v| *v = !*v)
            >
                <div class="flex items-center gap-2 min-w-0 flex-1">
                    <img
                        src=move || if is_active.get() { "/kyomi_animated_logo.svg" } else { "/kyomi_small_logo.svg" }
                        alt=move || if is_active.get() { "Processing" } else { "Thinking" }
                        class="w-4 h-4 flex-shrink-0"
                    />
                    {move || current_title.get().map(|title| {
                        view! {
                            <span class="text-xs text-muted-foreground truncate">{title}</span>
                        }
                    })}
                </div>
                <div class="flex items-center gap-2 flex-shrink-0">
                    <span class="text-xs text-muted-foreground font-mono whitespace-nowrap">
                        <span>{move || format!("{} tools \u{2022} {}{}", tool_executions_count.get(), format_duration(display_duration.get()), token_suffix.get())}</span>
                    </span>
                    <Icon
                        icon=phosphor_leptos::CARET_DOWN
                        attr:class=move || format!(
                            "w-3.5 h-3.5 text-muted-foreground transition-transform {}",
                            if is_expanded.get() { "rotate-180" } else { "" }
                        )
                        size="14px"
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
                    {move || thinking_events.get().iter().map(|event| {
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
