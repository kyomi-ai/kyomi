// SPDX-License-Identifier: AGPL-3.0-or-later

//! ScheduleSelector component — matches
//! `apps/frontend/src/components/watches/ScheduleSelector.jsx` exactly.
//!
//! Dual-mode schedule editor: a visual UI mode (type/time/day pickers)
//! and a raw 5-field cron input mode. All times are displayed in local
//! timezone but stored as UTC cron expressions.

use leptos::prelude::*;
use leptos_icons::Icon;

use crate::components::{DynSelect, Label, Switch, INPUT_CLASS};
use crate::utils::cron::{describe_cron, get_tz_offset_minutes, local_hour_to_utc, utc_to_local_hour};

// ---------------------------------------------------------------------------
// Button CSS constants (from button.rs) for toggle buttons that need
// reactive variant switching. The Button component takes a non-reactive
// `variant` prop, so we use raw `<button>` elements with the same classes.
// ---------------------------------------------------------------------------

const BTN_BASE: &str = "inline-flex items-center justify-center gap-2 whitespace-nowrap rounded-md text-sm font-medium transition-colors focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring disabled:pointer-events-none disabled:opacity-50 [&_svg]:pointer-events-none [&_svg]:size-4 [&_svg]:shrink-0";
const BTN_DEFAULT: &str = "bg-primary text-primary-foreground shadow hover:bg-primary/90";
const BTN_OUTLINE: &str = "border border-input bg-background text-foreground shadow-sm hover:bg-accent hover:text-accent-foreground";
const BTN_GHOST: &str = "text-foreground hover:bg-accent hover:text-accent-foreground";
const BTN_SM: &str = "h-8 rounded-md px-3 text-xs";

// ---------------------------------------------------------------------------
// Pure helper functions (ported from the React source)
// ---------------------------------------------------------------------------

/// Adjust weekdays array for day offset (when timezone conversion crosses midnight).
fn adjust_weekdays(weekdays: &[u32], day_offset: i32) -> Vec<u32> {
    if day_offset == 0 || weekdays.is_empty() {
        return weekdays.to_vec();
    }
    weekdays
        .iter()
        .map(|&day| ((day as i32 + day_offset + 7) % 7) as u32)
        .collect()
}

/// Adjust day of month for day offset.
fn adjust_day_of_month(day: u32, day_offset: i32) -> u32 {
    if day_offset == 0 {
        return day;
    }
    let adjusted = day as i32 + day_offset;
    adjusted.clamp(1, 31) as u32
}

/// Build a cron expression from UI selections.
/// Converts local time to UTC for the cron expression.
fn build_cron(
    schedule_type: &str,
    minute: u32,
    hour: u32,
    weekdays: &[u32],
    day_of_month: u32,
    selected_hours: &[u32],
) -> String {
    let tz = get_tz_offset_minutes();
    let min = minute.to_string();

    match schedule_type {
        "hourly" => {
            if selected_hours.is_empty() {
                return format!("{min} * * * *");
            }
            let mut utc_hours: Vec<u32> = selected_hours
                .iter()
                .map(|&local_hour| local_hour_to_utc(local_hour, tz).hour)
                .collect();
            utc_hours.sort();
            let hours_str = utc_hours
                .iter()
                .map(|h| h.to_string())
                .collect::<Vec<_>>()
                .join(",");
            format!("{min} {hours_str} * * *")
        }
        "daily" => {
            let utc = local_hour_to_utc(hour, tz);
            format!("{min} {} * * *", utc.hour)
        }
        "weekly" => {
            let utc = local_hour_to_utc(hour, tz);
            let adjusted_days = adjust_weekdays(weekdays, utc.day_offset);
            let days = if adjusted_days.is_empty() {
                "1".to_string()
            } else {
                adjusted_days
                    .iter()
                    .map(|d| d.to_string())
                    .collect::<Vec<_>>()
                    .join(",")
            };
            format!("{min} {} * * {days}", utc.hour)
        }
        "monthly" => {
            let utc = local_hour_to_utc(hour, tz);
            let adjusted_day = adjust_day_of_month(day_of_month, utc.day_offset);
            format!("{min} {} {adjusted_day} * *", utc.hour)
        }
        _ => format!("{min} * * * *"),
    }
}

/// Parsed UI state from a cron expression.
struct ParsedCron {
    schedule_type: String,
    minute: u32,
    hour: u32,
    weekdays: Vec<u32>,
    day_of_month: u32,
    selected_hours: Vec<u32>,
}

/// Parse a cron expression back to UI selections.
/// Converts UTC times in cron to local time for display.
fn parse_cron_to_selections(cron: &str) -> Option<ParsedCron> {
    let parts: Vec<&str> = cron.trim().split_whitespace().collect();
    if parts.len() != 5 {
        return None;
    }

    let (minute_str, hour_str, day_of_month_str, _month, day_of_week_str) =
        (parts[0], parts[1], parts[2], parts[3], parts[4]);

    let parsed_minute: u32 = minute_str.parse().ok()?;
    let tz = get_tz_offset_minutes();

    // Every hour pattern: "N * * * *"
    if hour_str == "*" && day_of_month_str == "*" && day_of_week_str == "*" {
        return Some(ParsedCron {
            schedule_type: "hourly".into(),
            minute: parsed_minute,
            hour: 0,
            weekdays: vec![],
            day_of_month: 1,
            selected_hours: vec![],
        });
    }

    // Hourly patterns with specific hours
    if hour_str != "*" && day_of_month_str == "*" && day_of_week_str == "*" {
        // Step syntax (e.g., */2)
        if let Some(step_str) = hour_str.strip_prefix("*/") {
            if let Ok(step) = step_str.parse::<u32>() {
                if step > 0 && step <= 12 {
                    let utc_hours: Vec<u32> = (0..24).step_by(step as usize).collect();
                    let mut local_hours: Vec<u32> = utc_hours
                        .iter()
                        .map(|&h| utc_to_local_hour(h, tz).hour)
                        .collect();
                    local_hours.sort();
                    return Some(ParsedCron {
                        schedule_type: "hourly".into(),
                        minute: parsed_minute,
                        hour: 0,
                        weekdays: vec![],
                        day_of_month: 1,
                        selected_hours: local_hours,
                    });
                }
            }
        }

        // Comma-separated hours (multiple hours = hourly mode)
        if hour_str.contains(',') {
            let hour_parts: Vec<u32> = hour_str
                .split(',')
                .filter_map(|h| h.trim().parse::<u32>().ok())
                .collect();

            if !hour_parts.is_empty() {
                let mut local_hours: Vec<u32> = hour_parts
                    .iter()
                    .map(|&h| utc_to_local_hour(h, tz).hour)
                    .collect();
                local_hours.sort();
                return Some(ParsedCron {
                    schedule_type: "hourly".into(),
                    minute: parsed_minute,
                    hour: 0,
                    weekdays: vec![],
                    day_of_month: 1,
                    selected_hours: local_hours,
                });
            }
        }
    }

    // Parse single hour for remaining patterns
    let parsed_hour: u32 = hour_str.parse().ok()?;
    let conversion = utc_to_local_hour(parsed_hour, tz);

    // Daily: "N H * * *"
    if day_of_month_str == "*" && day_of_week_str == "*" {
        return Some(ParsedCron {
            schedule_type: "daily".into(),
            minute: parsed_minute,
            hour: conversion.hour,
            weekdays: vec![],
            day_of_month: 1,
            selected_hours: vec![],
        });
    }

    // Weekly: "N H * * D,D,..."
    if day_of_month_str == "*" && day_of_week_str != "*" {
        let mut weekdays_list: Vec<u32> = day_of_week_str
            .split(',')
            .filter_map(|d| d.parse::<u32>().ok())
            .collect();
        if weekdays_list.is_empty() {
            return None;
        }
        if conversion.day_offset != 0 {
            weekdays_list = weekdays_list
                .iter()
                .map(|&d| ((d as i32 + conversion.day_offset + 7) % 7) as u32)
                .collect();
        }
        return Some(ParsedCron {
            schedule_type: "weekly".into(),
            minute: parsed_minute,
            hour: conversion.hour,
            weekdays: weekdays_list,
            day_of_month: 1,
            selected_hours: vec![],
        });
    }

    // Monthly: "N H D * *"
    if day_of_month_str != "*" && day_of_week_str == "*" {
        let parsed_dom: i32 = day_of_month_str.parse().ok()?;
        let mut dom = parsed_dom;
        if conversion.day_offset != 0 {
            dom -= conversion.day_offset;
            dom = dom.clamp(1, 31);
        }
        return Some(ParsedCron {
            schedule_type: "monthly".into(),
            minute: parsed_minute,
            hour: conversion.hour,
            weekdays: vec![],
            day_of_month: dom as u32,
            selected_hours: vec![],
        });
    }

    // Complex expression, can't map to simple UI
    None
}

// ---------------------------------------------------------------------------
// 12-hour time helpers
// ---------------------------------------------------------------------------

/// Get 12-hour value from 24-hour.
fn get_hour_12(hour_24: u32) -> u32 {
    if hour_24 == 0 {
        12
    } else if hour_24 > 12 {
        hour_24 - 12
    } else {
        hour_24
    }
}

/// Get AM/PM from 24-hour value.
fn get_am_pm(hour_24: u32) -> &'static str {
    if hour_24 < 12 {
        "AM"
    } else {
        "PM"
    }
}

/// Convert 12-hour + AM/PM to 24-hour.
fn to_24_hour(hour_12: u32, am_pm: &str) -> u32 {
    if hour_12 == 12 {
        if am_pm == "AM" {
            0
        } else {
            12
        }
    } else if am_pm == "AM" {
        hour_12
    } else {
        hour_12 + 12
    }
}

/// Get ordinal suffix for a day number.
fn ordinal_suffix(day: u32) -> &'static str {
    if (11..=13).contains(&(day % 100)) {
        return "th";
    }
    match day % 10 {
        1 => "st",
        2 => "nd",
        3 => "rd",
        _ => "th",
    }
}

// ---------------------------------------------------------------------------
// Static option data
// ---------------------------------------------------------------------------

/// Generate hour options for the hourly mode grid (0-23 in 12-hour format).
fn hour_options() -> Vec<(u32, String)> {
    (0..24)
        .map(|i| {
            let hour_12 = if i == 0 {
                12
            } else if i > 12 {
                i - 12
            } else {
                i
            };
            let ampm = if i < 12 { "AM" } else { "PM" };
            (i, format!("{hour_12} {ampm}"))
        })
        .collect()
}

/// Minute options at 5-minute intervals.
const MINUTE_VALUES: [u32; 12] = [0, 5, 10, 15, 20, 25, 30, 35, 40, 45, 50, 55];

/// Weekday options: (value, short label, full label).
const WEEKDAY_OPTIONS: [(u32, &str, &str); 7] = [
    (0, "Sun", "Sunday"),
    (1, "Mon", "Monday"),
    (2, "Tue", "Tuesday"),
    (3, "Wed", "Wednesday"),
    (4, "Thu", "Thursday"),
    (5, "Fri", "Friday"),
    (6, "Sat", "Saturday"),
];

// ---------------------------------------------------------------------------
// Component
// ---------------------------------------------------------------------------

/// Comprehensive schedule picker for watches.
///
/// Features:
/// - Schedule type: hourly, daily, weekly, monthly
/// - Time picker: hour and minute
/// - Day selector: weekdays for weekly, day of month for monthly
/// - Cron mode: raw cron input with human-readable description
///
/// Ported from `apps/frontend/src/components/watches/ScheduleSelector.jsx`.
#[component]
pub fn ScheduleSelector(
    /// Current cron expression.
    #[prop(into)]
    value: Signal<String>,
    /// Called with new cron expression when the schedule changes.
    on_change: Callback<String>,
) -> impl IntoView {
    // Parse the initial value to determine initial UI state
    let initial_value = value.get_untracked();
    let parsed = parse_cron_to_selections(&initial_value);
    let can_use_ui_mode = parsed.is_some();

    // -- State signals --
    let cron_mode = RwSignal::new(!can_use_ui_mode);
    let cron_input = RwSignal::new(initial_value.clone());
    let schedule_type = RwSignal::new(
        parsed
            .as_ref()
            .map(|p| p.schedule_type.clone())
            .unwrap_or_else(|| "daily".into()),
    );
    let minute = RwSignal::new(parsed.as_ref().map(|p| p.minute).unwrap_or(0));
    let hour = RwSignal::new(parsed.as_ref().map(|p| p.hour).unwrap_or(9));
    let weekdays = RwSignal::new(
        parsed
            .as_ref()
            .map(|p| p.weekdays.clone())
            .unwrap_or_else(|| vec![1]),
    );
    let day_of_month = RwSignal::new(parsed.as_ref().map(|p| p.day_of_month).unwrap_or(1));
    let selected_hours = RwSignal::new(
        parsed
            .as_ref()
            .map(|p| p.selected_hours.clone())
            .unwrap_or_default(),
    );
    let show_hour_selection = RwSignal::new(
        parsed
            .as_ref()
            .map(|p| !p.selected_hours.is_empty())
            .unwrap_or(false),
    );

    // Track whether user has interacted (to avoid firing onChange on mount)
    let user_has_interacted = RwSignal::new(false);

    // Track the last external value to detect parent-driven changes
    let last_value = RwSignal::new(initial_value);

    // Sync state when value prop changes from parent
    Effect::new(move |_| {
        let v = value.get();
        if v != last_value.get_untracked() {
            last_value.set(v.clone());
            cron_input.set(v.clone());
            if let Some(p) = parse_cron_to_selections(&v) {
                schedule_type.set(p.schedule_type);
                minute.set(p.minute);
                hour.set(p.hour);
                weekdays.set(p.weekdays);
                day_of_month.set(p.day_of_month);
                selected_hours.set(p.selected_hours);
            }
        }
    });

    // Fire onChange when UI state changes (only after user interaction)
    Effect::new(move |_| {
        // Subscribe to all UI state
        let st = schedule_type.get();
        let m = minute.get();
        let h = hour.get();
        let wd = weekdays.get();
        let dom = day_of_month.get();
        let sh = selected_hours.get();
        let cm = cron_mode.get();

        if !user_has_interacted.get_untracked() {
            return;
        }
        if !cm {
            let new_cron = build_cron(&st, m, h, &wd, dom, &sh);
            on_change.run(new_cron);
        }
    });

    // Fire onChange when cron input changes in cron mode
    Effect::new(move |_| {
        let ci = cron_input.get();
        let cm = cron_mode.get();
        if !user_has_interacted.get_untracked() {
            return;
        }
        if cm {
            on_change.run(ci);
        }
    });

    // Check if current cron can be switched to UI mode
    let can_switch_to_ui = Memo::new(move |_| {
        parse_cron_to_selections(&cron_input.get()).is_some()
    });

    // -- Handlers --

    let handle_mode_switch = move |use_cron: bool| {
        user_has_interacted.set(true);
        if !use_cron {
            // Switching to UI mode -- try to parse current cron
            if let Some(p) = parse_cron_to_selections(&cron_input.get()) {
                schedule_type.set(p.schedule_type);
                minute.set(p.minute);
                hour.set(p.hour);
                weekdays.set(p.weekdays);
                day_of_month.set(p.day_of_month);
                selected_hours.set(p.selected_hours.clone());
                show_hour_selection.set(!p.selected_hours.is_empty());
                cron_mode.set(false);
            }
            // If parsing fails, stay in cron mode
        } else {
            // Switching to cron mode -- set cron from current UI
            let new_cron = build_cron(
                &schedule_type.get_untracked(),
                minute.get_untracked(),
                hour.get_untracked(),
                &weekdays.get_untracked(),
                day_of_month.get_untracked(),
                &selected_hours.get_untracked(),
            );
            cron_input.set(new_cron);
            cron_mode.set(true);
        }
    };

    let handle_schedule_type_change = move |new_type: String| {
        user_has_interacted.set(true);
        if new_type != "hourly" && !selected_hours.get_untracked().is_empty() {
            selected_hours.set(vec![]);
            show_hour_selection.set(false);
        }
        schedule_type.set(new_type);
    };

    let handle_hour_12_change = move |val: String| {
        user_has_interacted.set(true);
        if let Ok(h12) = val.parse::<u32>() {
            let ampm = get_am_pm(hour.get_untracked());
            hour.set(to_24_hour(h12, ampm));
        }
    };

    let handle_am_pm_change = move |val: String| {
        user_has_interacted.set(true);
        let h12 = get_hour_12(hour.get_untracked());
        hour.set(to_24_hour(h12, &val));
    };

    let handle_minute_change = move |val: String| {
        user_has_interacted.set(true);
        if let Ok(m) = val.parse::<u32>() {
            minute.set(m);
        }
    };

    let handle_day_of_month_change = move |val: String| {
        user_has_interacted.set(true);
        if let Ok(d) = val.parse::<u32>() {
            day_of_month.set(d);
        }
    };

    let handle_cron_input_change = move |ev: web_sys::Event| {
        user_has_interacted.set(true);
        let target = event_target_value(&ev);
        cron_input.set(target);
    };

    // Cron description (reactive)
    let cron_description = Memo::new(move |_| {
        let tz = get_tz_offset_minutes();
        let cron = if cron_mode.get() {
            cron_input.get()
        } else {
            build_cron(
                &schedule_type.get(),
                minute.get(),
                hour.get(),
                &weekdays.get(),
                day_of_month.get(),
                &selected_hours.get(),
            )
        };
        describe_cron(&cron, tz)
    });

    // Pre-compute static option lists
    let hour_opts = hour_options();

    view! {
        <div class="space-y-4">
            // Mode toggle
            <div class="flex items-center justify-between">
                <Label>
                    <span class="flex items-center gap-2">
                        <Icon icon=icondata_lu::LuClock attr:class="h-4 w-4"/>
                        "Schedule"
                    </span>
                </Label>
                <div class="flex items-center gap-2">
                    <span class="text-xs text-muted-foreground">"Cron mode"</span>
                    {move || {
                        let is_disabled = cron_mode.get() && !can_switch_to_ui.get();
                        view! {
                            <Switch
                                checked=Signal::derive(move || cron_mode.get())
                                on_change=Callback::new(move |val: bool| handle_mode_switch(val))
                                disabled=is_disabled
                            />
                        }
                    }}
                    <Icon icon=icondata_lu::LuCode attr:class="h-4 w-4 text-muted-foreground"/>
                </div>
            </div>

            // Warning when cron can't be converted to UI mode
            {move || {
                (cron_mode.get() && !can_switch_to_ui.get()).then(|| view! {
                    <p class="text-xs text-muted-foreground">
                        "This schedule uses advanced cron syntax that can\u{2019}t be edited in simple mode."
                    </p>
                })
            }}

            // Cron mode vs UI mode
            {move || {
                if cron_mode.get() {
                    // -- Cron mode --
                    view! {
                        <div class="space-y-3">
                            <input
                                type="text"
                                class=format!("{INPUT_CLASS} font-mono")
                                prop:value=move || cron_input.get()
                                on:input=handle_cron_input_change
                                placeholder="0 9 * * *"
                            />
                            <p class="text-xs text-muted-foreground">
                                "Format: minute hour day-of-month month day-of-week (e.g., 0 9 * * 1-5 for weekdays at 9 AM UTC)"
                            </p>
                            <p class="text-xs text-warning-foreground">
                                "Note: Cron times are in UTC"
                            </p>
                        </div>
                    }.into_any()
                } else {
                    // -- UI mode --
                    let hour_opts_clone = hour_opts.clone();
                    view! {
                        <div class="space-y-4">
                            // Schedule type selector
                            <ScheduleTypeSelect
                                value=Signal::derive(move || schedule_type.get())
                                on_change=Callback::new(handle_schedule_type_change)
                            />

                            // Time selector (not for hourly)
                            {move || {
                                (schedule_type.get() != "hourly").then(|| view! {
                                    <div class="flex items-center gap-2">
                                        <span class="text-sm text-muted-foreground">"At"</span>
                                        // Hour (12-hour format)
                                        <Hour12Select
                                            value=Signal::derive(move || get_hour_12(hour.get()).to_string())
                                            on_change=Callback::new(handle_hour_12_change)
                                        />
                                        <span class="text-muted-foreground">":"</span>
                                        // Minute
                                        <MinuteSelect
                                            value=Signal::derive(move || minute.get().to_string())
                                            on_change=Callback::new(handle_minute_change)
                                            hourly_mode=false
                                        />
                                        // AM/PM
                                        <AmPmSelect
                                            value=Signal::derive(move || get_am_pm(hour.get()).to_string())
                                            on_change=Callback::new(handle_am_pm_change)
                                        />
                                    </div>
                                })
                            }}

                            // Hourly mode: hour selection + minute
                            {move || {
                                let hour_opts_inner = hour_opts_clone.clone();
                                (schedule_type.get() == "hourly").then(move || view! {
                                    <div class="space-y-3">
                                        // Toggle for hour selection
                                        {move || {
                                            let hour_opts_grid = hour_opts_inner.clone();
                                            if !show_hour_selection.get() {
                                                view! {
                                                    <div class="space-y-2">
                                                        <p class="text-sm text-muted-foreground">"Runs every hour"</p>
                                                        <button
                                                            type="button"
                                                            class=format!("{BTN_BASE} {BTN_OUTLINE} {BTN_SM}")
                                                            on:click=move |_| {
                                                                user_has_interacted.set(true);
                                                                show_hour_selection.set(true);
                                                            }
                                                        >
                                                            "Select which hours to run"
                                                        </button>
                                                    </div>
                                                }.into_any()
                                            } else {
                                                view! {
                                                    <div class="space-y-2">
                                                        <div class="flex items-center justify-between">
                                                            <span class="text-sm text-muted-foreground">"Select hours to run"</span>
                                                            <button
                                                                type="button"
                                                                class=format!("{BTN_BASE} {BTN_GHOST} {BTN_SM} text-xs h-7")
                                                                on:click=move |_| {
                                                                    user_has_interacted.set(true);
                                                                    show_hour_selection.set(false);
                                                                    selected_hours.set(vec![]);
                                                                }
                                                            >
                                                                "Run every hour instead"
                                                            </button>
                                                        </div>
                                                        <div class="grid grid-cols-4 sm:grid-cols-6 gap-1">
                                                            {hour_opts_grid.into_iter().map(|(val, label)| {
                                                                view! {
                                                                    <button
                                                                        type="button"
                                                                        class=move || {
                                                                            let variant = if selected_hours.get().contains(&val) {
                                                                                BTN_DEFAULT
                                                                            } else {
                                                                                BTN_OUTLINE
                                                                            };
                                                                            format!("{BTN_BASE} {variant} {BTN_SM} text-xs px-2")
                                                                        }
                                                                        on:click=move |_| {
                                                                            user_has_interacted.set(true);
                                                                            let mut hrs = selected_hours.get();
                                                                            if hrs.contains(&val) {
                                                                                hrs.retain(|&h| h != val);
                                                                            } else {
                                                                                hrs.push(val);
                                                                                hrs.sort();
                                                                            }
                                                                            selected_hours.set(hrs);
                                                                        }
                                                                    >
                                                                        {label}
                                                                    </button>
                                                                }
                                                            }).collect_view()}
                                                        </div>
                                                        {move || {
                                                            selected_hours.get().is_empty().then(|| view! {
                                                                <p class="text-xs text-warning-foreground">"Select at least one hour"</p>
                                                            })
                                                        }}
                                                    </div>
                                                }.into_any()
                                            }
                                        }}

                                        // Minute past the hour
                                        <div class="flex items-center gap-2">
                                            <span class="text-sm text-muted-foreground">"At"</span>
                                            <MinuteSelect
                                                value=Signal::derive(move || minute.get().to_string())
                                                on_change=Callback::new(handle_minute_change)
                                                hourly_mode=true
                                            />
                                            <span class="text-sm text-muted-foreground">"past the hour"</span>
                                        </div>
                                    </div>
                                })
                            }}

                            // Weekday selector for weekly
                            {move || {
                                (schedule_type.get() == "weekly").then(|| view! {
                                    <div class="space-y-2">
                                        <span class="text-sm text-muted-foreground">"On"</span>
                                        <div class="flex flex-wrap gap-1">
                                            {WEEKDAY_OPTIONS.iter().map(|&(val, label, _full_label)| {
                                                view! {
                                                    <button
                                                        type="button"
                                                        class=move || {
                                                            let variant = if weekdays.get().contains(&val) {
                                                                BTN_DEFAULT
                                                            } else {
                                                                BTN_OUTLINE
                                                            };
                                                            format!("{BTN_BASE} {variant} {BTN_SM} w-10")
                                                        }
                                                        on:click=move |_| {
                                                            user_has_interacted.set(true);
                                                            let mut wd = weekdays.get();
                                                            if wd.contains(&val) {
                                                                wd.retain(|&d| d != val);
                                                            } else {
                                                                wd.push(val);
                                                                wd.sort();
                                                            }
                                                            weekdays.set(wd);
                                                        }
                                                    >
                                                        {label}
                                                    </button>
                                                }
                                            }).collect_view()}
                                        </div>
                                        {move || {
                                            weekdays.get().is_empty().then(|| view! {
                                                <p class="text-xs text-warning-foreground">"Select at least one day"</p>
                                            })
                                        }}
                                    </div>
                                })
                            }}

                            // Day of month selector for monthly
                            {move || {
                                (schedule_type.get() == "monthly").then(|| view! {
                                    <div class="flex items-center gap-2">
                                        <span class="text-sm text-muted-foreground">"On the"</span>
                                        <DayOfMonthSelect
                                            value=Signal::derive(move || day_of_month.get().to_string())
                                            on_change=Callback::new(handle_day_of_month_change)
                                        />
                                        <span class="text-sm text-muted-foreground">"of each month"</span>
                                    </div>
                                })
                            }}
                        </div>
                    }.into_any()
                }
            }}

            // Schedule description
            {move || {
                let desc = cron_description.get();
                let container_class = if desc.valid {
                    "rounded-lg p-3 text-sm bg-muted/50"
                } else {
                    "rounded-lg p-3 text-sm bg-error/10 border border-error-border"
                };
                let text_class = if desc.valid {
                    "text-foreground"
                } else {
                    "text-error-foreground"
                };

                view! {
                    <div class=container_class>
                        <div class="flex items-start gap-2">
                            {(!desc.valid).then(|| view! {
                                <Icon icon=icondata_lu::LuAlertCircle attr:class="h-4 w-4 text-error-foreground mt-0.5 shrink-0"/>
                            })}
                            <div class="flex-1">
                                <span class=text_class>{desc.description}</span>
                                {desc.valid.then(|| {
                                    let cm = cron_mode.get();
                                    view! {
                                        <div class="text-xs text-muted-foreground mt-1">
                                            {if cm {
                                                let ci = cron_input.get();
                                                view! {
                                                    <span>"Cron: "<code class="bg-muted px-1 rounded">{ci}</code>" (UTC)"</span>
                                                }.into_any()
                                            } else {
                                                view! {
                                                    <span>"Times shown in your local timezone"</span>
                                                }.into_any()
                                            }}
                                        </div>
                                    }
                                })}
                            </div>
                        </div>
                    </div>
                }
            }}
        </div>
    }
}

// ---------------------------------------------------------------------------
// Small sub-components for Select dropdowns (using DynSelect)
// ---------------------------------------------------------------------------

/// Schedule type dropdown.
#[component]
fn ScheduleTypeSelect(
    value: Signal<String>,
    on_change: Callback<String>,
) -> impl IntoView {
    let options = Signal::derive(move || {
        vec![
            ("hourly".to_string(), "Hourly".to_string()),
            ("daily".to_string(), "Daily".to_string()),
            ("weekly".to_string(), "Weekly".to_string()),
            ("monthly".to_string(), "Monthly".to_string()),
        ]
    });

    view! {
        <DynSelect
            value=value
            options=options
            on_change=move |v| on_change.run(v)
            placeholder="Select frequency"
        />
    }
}

/// 12-hour format hour dropdown (1-12) with w-[70px] width.
#[component]
fn Hour12Select(
    value: Signal<String>,
    on_change: Callback<String>,
) -> impl IntoView {
    let options = Signal::derive(move || {
        (1..=12)
            .map(|i| (i.to_string(), i.to_string()))
            .collect::<Vec<_>>()
    });

    view! {
        <div class="w-[70px]">
            <DynSelect
                value=value
                options=options
                on_change=move |v| on_change.run(v)
            />
        </div>
    }
}

/// Minute dropdown (5-minute intervals).
///
/// In hourly mode, width is `w-[80px]` and labels are prefixed with ":".
/// In non-hourly mode, width is `w-[70px]`.
#[component]
fn MinuteSelect(
    value: Signal<String>,
    on_change: Callback<String>,
    hourly_mode: bool,
) -> impl IntoView {
    let options = Signal::derive(move || {
        MINUTE_VALUES
            .iter()
            .map(|&m| {
                let label = if hourly_mode {
                    format!(":{m:02}")
                } else {
                    format!("{m:02}")
                };
                (m.to_string(), label)
            })
            .collect::<Vec<_>>()
    });

    let width = if hourly_mode { "w-[80px]" } else { "w-[70px]" };

    view! {
        <div class=width>
            <DynSelect
                value=value
                options=options
                on_change=move |v| on_change.run(v)
            />
        </div>
    }
}

/// AM/PM dropdown with w-[70px] width.
#[component]
fn AmPmSelect(
    value: Signal<String>,
    on_change: Callback<String>,
) -> impl IntoView {
    let options = Signal::derive(move || {
        vec![
            ("AM".to_string(), "AM".to_string()),
            ("PM".to_string(), "PM".to_string()),
        ]
    });

    view! {
        <div class="w-[70px]">
            <DynSelect
                value=value
                options=options
                on_change=move |v| on_change.run(v)
            />
        </div>
    }
}

/// Day of month dropdown (1-31) with w-[100px] width.
#[component]
fn DayOfMonthSelect(
    value: Signal<String>,
    on_change: Callback<String>,
) -> impl IntoView {
    let options = Signal::derive(move || {
        (1..=31)
            .map(|d| {
                let label = format!("{d}{}", ordinal_suffix(d));
                (d.to_string(), label)
            })
            .collect::<Vec<_>>()
    });

    view! {
        <div class="w-[100px]">
            <DynSelect
                value=value
                options=options
                on_change=move |v| on_change.run(v)
            />
        </div>
    }
}
