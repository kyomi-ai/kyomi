// SPDX-License-Identifier: AGPL-3.0-or-later

//! Agent Thinking UI Component
//!
//! Displays agent thinking events in an expandable/collapsible panel with a
//! live timer, auto-scroll, event icons, and multiple rendering variants.
//!
//! Ported from `apps/frontend/src/components/AgentThinking.jsx` (240 lines React).
//! CSS classes are copied verbatim from the React source.

use leptos::prelude::*;

use super::thinking::{ThinkingEvent, TokenUsage};
use crate::components::button::{Button, ButtonSize, ButtonVariant};
use crate::components::card::Card;

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
    pub fn from_str(s: &str) -> Self {
        match s {
            "inset" => Self::Inset,
            "header-bar" => Self::HeaderBar,
            "tab" => Self::Tab,
            _ => Self::Default,
        }
    }
}

/// Get the emoji icon for a thinking event type.
///
/// Matches React's `getEventIcon()` exactly.
fn get_event_icon(event_type: &str) -> &'static str {
    match event_type {
        "agent_start" => "\u{1F916}",          // 🤖
        "agent_thought" => "\u{1F4AD}",        // 💭
        "tool_execution_start" => "\u{1F527}",  // 🔧
        "tool_execution_end" => "\u{2705}",     // ✅
        "agent_decision" => "\u{1F3AF}",        // 🎯
        "agent_complete" => "\u{1F389}",        // 🎉
        "error" => "\u{26A0}\u{FE0F}",         // ⚠️
        _ => "\u{1F4DD}",                       // 📝
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

/// Format a timestamp string to HH:MM:SS.f (24-hour).
///
/// Matches React's `formatTimestamp()` — parses ISO 8601 and formats
/// with 24-hour clock and fractional seconds.
fn format_timestamp(timestamp: &str) -> String {
    // Parse ISO 8601 timestamp using chrono
    if let Ok(dt) = chrono::DateTime::parse_from_rfc3339(timestamp) {
        // Format as HH:MM:SS.f (1 fractional digit)
        dt.format("%H:%M:%S%.1f").to_string()
    } else if let Ok(dt) = chrono::NaiveDateTime::parse_from_str(timestamp, "%Y-%m-%dT%H:%M:%S%.f")
    {
        dt.format("%H:%M:%S%.1f").to_string()
    } else {
        // Fallback: return the raw timestamp if we can't parse it
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
    let variant = ThinkingVariant::from_str(variant);

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
                if let Some(el) = thinking_end_ref.get() {
                    let opts = web_sys::ScrollIntoViewOptions::new();
                    opts.set_behavior(web_sys::ScrollBehavior::Smooth);
                    el.scroll_into_view_with_scroll_into_view_options(&opts);
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
            // Header - Always visible
            <div
                class="flex items-center justify-between cursor-pointer py-1"
                on:click=move |_| set_is_expanded.update(|v| *v = !*v)
            >
                <div class="flex items-center space-x-2 min-w-0 flex-1">
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
                            <span class="text-xs text-muted-foreground truncate animate-subtle-breathe">{title}</span>
                        }
                    })}
                </div>
                <div class="flex items-center space-x-2 flex-shrink-0">
                    <div class="text-xs text-muted-foreground font-mono whitespace-nowrap">
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
                    </div>
                    <Button variant=ButtonVariant::Ghost size=ButtonSize::Sm attr:class="h-6 w-6 text-muted-foreground hover:text-foreground p-0">
                        <svg
                            class=move || format!(
                                "w-3 h-3 transform transition-transform {}",
                                if is_expanded.get() { "rotate-180" } else { "" }
                            )
                            fill="none"
                            stroke="currentColor"
                            viewBox="0 0 24 24"
                        >
                            <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M19 9l-7 7-7-7" />
                        </svg>
                    </Button>
                </div>
            </div>

            // Expanded Content
            <div
                node_ref=scroll_container_ref
                class="space-y-2 overflow-y-auto transition-all duration-300 ease-in-out"
                style=move || format!(
                    "max-height: {}; margin-top: {}; opacity: {}; scroll-behavior: smooth;",
                    if is_expanded.get() { "24rem" } else { "0" },
                    if is_expanded.get() { "0.75rem" } else { "0" },
                    if is_expanded.get() { "1" } else { "0" },
                )
            >
                {events.iter().map(|event| {
                    let icon = get_event_icon(&event.event_type);
                    let title = event.title.clone();
                    let description = event.description.clone();
                    let duration_ms = event.duration_ms;
                    let timestamp = format_timestamp(&event.timestamp);

                    view! {
                        <div class="flex items-start space-x-3 py-2 px-3 bg-card rounded-lg border border-border">
                            <div class="flex-shrink-0 mt-0.5">
                                <span class="text-sm">{icon}</span>
                            </div>
                            <div class="flex-1 min-w-0">
                                <div class="flex items-center justify-between">
                                    <h4 class="text-sm font-medium text-foreground">
                                        {title}
                                    </h4>
                                    <div class="flex items-center space-x-2 text-xs text-muted-foreground">
                                        {duration_ms.map(|d| {
                                            let formatted = format_duration(d);
                                            view! { <span>{formatted}</span> }
                                        })}
                                        <span>{timestamp}</span>
                                    </div>
                                </div>
                                {description.map(|desc| {
                                    view! {
                                        <p class="text-sm text-muted-foreground mt-1">
                                            {desc}
                                        </p>
                                    }
                                })}
                            </div>
                        </div>
                    }
                }).collect_view()}
                <div node_ref=thinking_end_ref />
            </div>
        }
    };

    // -- Token usage display (appended after thinking content if present) --
    let _token_usage = token_usage;

    // -- Variant rendering --
    match variant {
        ThinkingVariant::Inset => {
            // Deep inset — looks like it's recessed behind the message
            view! {
                <div class="mb-4 -mx-6 -mt-2 bg-accent border-l-4 border-primary shadow-inner" data-testid="agent-thinking">
                    <div class="p-4 pl-6">
                        {render_content()}
                    </div>
                </div>
            }
            .into_any()
        }
        ThinkingVariant::HeaderBar => {
            // Slim bar at top that expands downward
            view! {
                <div class="mb-3 -mx-6 -mt-4 bg-muted border-b border-border overflow-hidden transition-all duration-300 ease-in-out" data-testid="agent-thinking">
                    <div class="py-1 px-4">
                        {render_content()}
                    </div>
                </div>
            }
            .into_any()
        }
        ThinkingVariant::Tab => {
            // Tab sticking out from top-left
            view! {
                <div class="mb-3 relative" data-testid="agent-thinking">
                    <div class="absolute -top-3 left-0 bg-primary text-white px-3 py-1 rounded-t-lg text-xs font-medium shadow">
                        "\u{1F9E0} Thinking"
                    </div>
                    <Card class="mt-2 bg-muted border-border pt-6">
                        <div class="p-3">
                            {render_content()}
                        </div>
                    </Card>
                </div>
            }
            .into_any()
        }
        ThinkingVariant::Default => {
            // Original floating card
            view! {
                <Card class="mt-2 mb-3 bg-muted border-border" attr:data-testid="agent-thinking">
                    <div class="p-2">
                        {render_content()}
                    </div>
                </Card>
            }
            .into_any()
        }
    }
}
