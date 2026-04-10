// SPDX-License-Identifier: AGPL-3.0-or-later

//! Usage settings page — AI usage tracking for workspace and team members.
//!
//! Replaces `apps/frontend/src/components/UsagePanel.jsx` (266 lines).
//!
//! Features:
//! - Workspace-level AI usage progress bar
//! - Feature breakdown with CSS-only stacked horizontal bar chart
//! - Warning/exhaustion alerts
//! - Uses Card, Alert, Skeleton

use leptos::prelude::*;

use crate::components::{
    Alert, AlertDescription, AlertVariant, Card, CardContent, CardDescription, CardHeader,
    CardTitle, Skeleton,
};
use crate::server_fns::usage::{get_ai_usage_status, UsageData};

use super::billing::format_number;
use kyomi_core::capability::ANALYTICS_EVENTS_INCLUDED;

/// Feature definition for the stacked bar chart legend.
struct FeatureDef {
    key: &'static str,
    label: &'static str,
    color: &'static str,
}

/// Feature color palette — first 4 colors from `chartPalettes.js` balanced palette.
const FEATURES: &[FeatureDef] = &[
    FeatureDef {
        key: "chat",
        label: "Chat Interface",
        color: "#1A75C9",
    },
    FeatureDef {
        key: "kyomi_watch",
        label: "Watch",
        color: "#8B5CF6",
    },
    FeatureDef {
        key: "dashboard_copilot",
        label: "Dashboard Copilot",
        color: "#B8405A",
    },
    FeatureDef {
        key: "chart_builder_copilot",
        label: "Chart Builder Copilot",
        color: "#3D8A5A",
    },
];

/// Determine the CSS class for usage bar color based on percentage.
///
/// Matches the React logic exactly:
/// - 0-79%:  `bg-success-foreground` (green)
/// - 80-89%: `bg-warning-foreground` (orange)
/// - 90%+:   `bg-error-foreground` (red)
fn usage_bar_class(percentage: f64, is_exhausted: bool) -> &'static str {
    if is_exhausted || percentage >= 90.0 {
        "h-2 rounded-full transition-all bg-error-foreground"
    } else if percentage >= 80.0 {
        "h-2 rounded-full transition-all bg-warning-foreground"
    } else {
        "h-2 rounded-full transition-all bg-success-foreground"
    }
}

/// Format an RFC3339 date string to "Mon DD, YYYY" display format.
///
/// Matches the React `toLocaleDateString('en-US', { month: 'short', day: 'numeric', year: 'numeric' })`.
fn format_reset_date(iso_str: &str) -> String {
    // Parse the ISO date and format as "Mar 21, 2026"
    if let Ok(dt) = chrono::DateTime::parse_from_rfc3339(iso_str) {
        dt.format("%b %-d, %Y").to_string()
    } else {
        iso_str.to_string()
    }
}

/// Usage settings page content.
///
/// Fetches AI usage status via server function and displays:
/// - Workspace AI Usage card with progress bar
/// - Feature breakdown card with stacked bar chart and legend
/// - Warning/exhaustion alerts
#[component]
pub fn UsagePage() -> impl IntoView {
    let usage_resource = Resource::new(|| (), |_| get_ai_usage_status());

    view! {
        <div class="p-4 sm:p-6">
            <h2 class="text-xl font-display text-foreground mb-6">"Usage"</h2>
            <Transition fallback=move || view! { <UsageLoadingSkeleton/> }>
                {move || Suspend::new(async move {
                    match usage_resource.await {
                        Ok(data) => view! { <UsageContent data=data/> }.into_any(),
                        Err(_) => {
                            // Don't leak raw SQL or internal errors to the UI.
                            // Match React: show the usage cards with zero data when the
                            // billing service is unavailable (e.g. SQLite without billing).
                            view! {
                                <UsageContent data=UsageData {
                                    percentage_used: 0.0,
                                    warning_level: None,
                                    allowed: true,
                                    blocked: false,
                                    ai_reset_date: None,
                                    trial_ends_at: None,
                                    per_user: crate::server_fns::usage::PerUserUsage {
                                        percentage_used: 0.0,
                                        fair_share_percentage: 100.0,
                                    },
                                    by_feature: std::collections::HashMap::new(),
                                    ai_bundle_balance_usd: 0.0,
                                    analytics_events_used: 0,
                                    analytics_events_included: ANALYTICS_EVENTS_INCLUDED,
                                    analytics_bundle_events: 0,
                                }/>
                            }.into_any()
                        }
                    }
                })}
            </Transition>
        </div>
    }
}

/// Loading skeleton shown while usage data is being fetched.
///
/// Matches the spinner pattern from the React component.
#[component]
fn UsageLoadingSkeleton() -> impl IntoView {
    view! {
        <div class="space-y-6" style:display="block">
            <Card>
                <CardHeader>
                    <Skeleton class="h-5 w-48"/>
                    <Skeleton class="h-4 w-32 mt-1"/>
                </CardHeader>
                <CardContent>
                    <div class="space-y-2">
                        <Skeleton class="h-4 w-full"/>
                        <Skeleton class="h-2 w-full"/>
                    </div>
                </CardContent>
            </Card>
            <Card>
                <CardHeader>
                    <Skeleton class="h-5 w-40"/>
                    <Skeleton class="h-4 w-56 mt-1"/>
                </CardHeader>
                <CardContent>
                    <div class="space-y-2">
                        <Skeleton class="h-4 w-full"/>
                        <Skeleton class="h-2 w-full"/>
                    </div>
                </CardContent>
            </Card>
            <Card>
                <CardHeader>
                    <Skeleton class="h-5 w-40"/>
                    <Skeleton class="h-4 w-64 mt-1"/>
                </CardHeader>
                <CardContent>
                    <Skeleton class="h-8 w-full mb-4"/>
                    <div class="space-y-2">
                        <Skeleton class="h-4 w-full"/>
                        <Skeleton class="h-4 w-full"/>
                        <Skeleton class="h-4 w-full"/>
                        <Skeleton class="h-4 w-full"/>
                    </div>
                </CardContent>
            </Card>
        </div>
    }
}

/// Main usage content — renders all cards from the fetched data.
#[component]
fn UsageContent(data: UsageData) -> impl IntoView {
    let percentage = data.percentage_used;
    let is_exhausted = data.blocked;
    let ai_reset_date = data.ai_reset_date.clone();
    let by_feature = data.by_feature.clone();
    let ai_bundle_balance_usd = data.ai_bundle_balance_usd;
    let analytics_events_used = data.analytics_events_used;
    let analytics_events_included = data.analytics_events_included;
    let analytics_bundle_events = data.analytics_bundle_events;

    view! {
        <div class="space-y-6" style:display="block">
            // Workspace AI Usage card
            <WorkspaceUsageCard
                percentage=percentage
                is_exhausted=is_exhausted
                ai_reset_date=ai_reset_date
                ai_bundle_balance_usd=ai_bundle_balance_usd
            />

            // Analytics Events card
            <AnalyticsEventsCard
                events_used=analytics_events_used
                events_included=analytics_events_included
                bundle_events=analytics_bundle_events
            />

            // Feature Breakdown card
            <FeatureBreakdownCard by_feature=by_feature/>

            // Warning alert when near limit
            {data.warning_level.as_ref().map(|level| {
                let (variant, message) = match level.as_str() {
                    "blocked" => (
                        AlertVariant::Error,
                        "AI budget exhausted. Add an AI token bundle or connect your own API key to continue.".to_string(),
                    ),
                    "critical" => (
                        AlertVariant::Warning,
                        format!(
                            "AI budget critically low ({:.1}% used). Consider purchasing an AI token bundle to avoid interruption.",
                            percentage,
                        ),
                    ),
                    "warning" => (
                        AlertVariant::Warning,
                        format!(
                            "AI budget at {:.1}%. Consider purchasing an AI token bundle.",
                            percentage,
                        ),
                    ),
                    _ => return view! { <div/> }.into_any(),
                };

                view! {
                    <Alert variant=variant>
                        <AlertDescription>{message}</AlertDescription>
                    </Alert>
                }.into_any()
            })}
        </div>
    }
}

/// Workspace AI Usage card with progress bar.
///
/// React: first Card in UsagePanel — "Workspace AI Usage" header,
/// percentage label, horizontal progress bar, optional exhaustion text,
/// and reset date.
#[component]
fn WorkspaceUsageCard(
    percentage: f64,
    is_exhausted: bool,
    ai_reset_date: Option<String>,
    ai_bundle_balance_usd: f64,
) -> impl IntoView {
    let bar_class = usage_bar_class(percentage, is_exhausted);
    let bar_width = format!("{}%", percentage.min(100.0));

    view! {
        <Card>
            <CardHeader>
                <CardTitle>"Workspace AI Usage"</CardTitle>
                <CardDescription>"Track your AI usage"</CardDescription>
            </CardHeader>
            <CardContent>
                <div>
                    <div class="flex justify-between mb-2">
                        <span class="text-sm font-medium text-foreground">
                            "AI Usage This Month"
                        </span>
                        <span class="text-sm font-medium text-foreground">
                            {format!("{:.1}% used", percentage)}
                        </span>
                    </div>
                    <div class="w-full bg-muted rounded-full h-2">
                        <div
                            class=bar_class
                            style:width=bar_width
                        />
                    </div>
                    {(ai_bundle_balance_usd > 0.0).then(|| view! {
                        <p class="text-sm text-muted-foreground mt-2">
                            {format!("Token Bundle Balance: ${:.2} remaining", ai_bundle_balance_usd)}
                        </p>
                    })}
                    {is_exhausted.then(|| view! {
                        <p class="text-sm text-error-foreground mt-2">
                            "AI budget exhausted. Add an AI token bundle or connect your own API key to continue."
                        </p>
                    })}
                    {ai_reset_date.map(|date| view! {
                        <p class="text-xs text-muted-foreground mt-1">
                            {format!("Resets {}", format_reset_date(&date))}
                        </p>
                    })}
                </div>
            </CardContent>
        </Card>
    }
}

/// Feature Breakdown card with CSS-only stacked horizontal bar chart.
///
/// React: third Card in UsagePanel — "Usage by Feature" header,
/// stacked bar (colored divs with percentage widths), and legend.
#[component]
fn FeatureBreakdownCard(by_feature: std::collections::HashMap<String, f64>) -> impl IntoView {
    view! {
        <Card>
            <CardHeader>
                <CardTitle>"Usage by Feature"</CardTitle>
                <CardDescription>
                    "Distribution of AI usage across different features"
                </CardDescription>
            </CardHeader>
            <CardContent>
                // Stacked horizontal bar showing distribution
                <div class="w-full h-8 bg-muted rounded-lg overflow-hidden flex mb-4">
                    {FEATURES.iter().map(|feat| {
                        let pct = *by_feature.get(feat.key).unwrap_or(&0.0);
                        let width = format!("{}%", pct);
                        let title = format!("{}: {}%", feat.label, pct as u32);
                        let color = feat.color.to_string();

                        // Only render segment if percentage > 0
                        if pct > 0.0 {
                            view! {
                                <div
                                    class="transition-all flex items-center justify-center"
                                    style:width=width
                                    style:background-color=color
                                    title=title
                                />
                            }.into_any()
                        } else {
                            ().into_any()
                        }
                    }).collect_view()}
                </div>

                // Legend — always show all features
                <div class="space-y-2">
                    {FEATURES.iter().map(|feat| {
                        let pct = *by_feature.get(feat.key).unwrap_or(&0.0);
                        let color = feat.color.to_string();
                        let label = feat.label;

                        view! {
                            <div class="flex items-center justify-between">
                                <div class="flex items-center gap-2">
                                    <div
                                        class="w-3 h-3 rounded-sm"
                                        style:background-color=color
                                    />
                                    <span class="text-sm text-foreground">{label}</span>
                                </div>
                                <span class="text-sm font-medium text-foreground">
                                    {format!("{}%", pct as u32)}
                                </span>
                            </div>
                        }
                    }).collect_view()}
                </div>
            </CardContent>
        </Card>
    }
}

/// Analytics Events card — shows monthly event usage against the included quota.
///
/// Mirrors the workspace AI usage card pattern: progress bar with color thresholds,
/// optional bundle balance display.
#[component]
fn AnalyticsEventsCard(
    events_used: u64,
    events_included: u64,
    bundle_events: i64,
) -> impl IntoView {
    let total_available = events_included as i64 + bundle_events.max(0);
    let percentage = if total_available > 0 {
        ((events_used as f64 / total_available as f64) * 100.0).min(100.0)
    } else {
        0.0
    };
    let is_exhausted = events_used as i64 >= total_available;
    let bar_class = usage_bar_class(percentage, is_exhausted);
    let bar_width = format!("{}%", percentage.min(100.0));

    view! {
        <Card>
            <CardHeader>
                <CardTitle>"Analytics Events"</CardTitle>
                <CardDescription>"Track your analytics event usage"</CardDescription>
            </CardHeader>
            <CardContent>
                <div>
                    <div class="flex justify-between mb-2">
                        <span class="text-sm font-medium text-foreground">
                            "Events This Month"
                        </span>
                        <span class="text-sm font-medium text-foreground">
                            {format!(
                                "{} / {} used",
                                format_number(events_used),
                                format_number(events_included),
                            )}
                        </span>
                    </div>
                    <div class="w-full bg-muted rounded-full h-2">
                        <div
                            class=bar_class
                            style:width=bar_width
                        />
                    </div>
                    {(bundle_events > 0).then(|| view! {
                        <p class="text-sm text-muted-foreground mt-2">
                            {format!("Bundle balance: {} events remaining", format_number(bundle_events as u64))}
                        </p>
                    })}
                    {is_exhausted.then(|| view! {
                        <p class="text-sm text-error-foreground mt-2">
                            "Analytics event quota exhausted. Purchase an analytics event bundle to continue."
                        </p>
                    })}
                </div>
            </CardContent>
        </Card>
    }
}
