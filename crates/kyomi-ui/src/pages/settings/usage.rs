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
            <h2 class="text-xl font-semibold text-foreground mb-6">"Usage"</h2>
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

    view! {
        <div class="space-y-6" style:display="block">
            // Workspace AI Usage card
            <WorkspaceUsageCard
                percentage=percentage
                is_exhausted=is_exhausted
                ai_reset_date=ai_reset_date
            />

            // Feature Breakdown card
            <FeatureBreakdownCard by_feature=by_feature/>

            // Warning alert when near limit
            {data.warning_level.as_ref().map(|level| {
                let (variant, message) = match level.as_str() {
                    "blocked" => (
                        AlertVariant::Error,
                        "AI budget exhausted. Upgrade to continue using AI features.".to_string(),
                    ),
                    "critical" => (
                        AlertVariant::Warning,
                        format!(
                            "AI budget critically low ({:.1}% used). Consider upgrading to avoid interruption.",
                            percentage,
                        ),
                    ),
                    "warning" => (
                        AlertVariant::Warning,
                        format!(
                            "AI budget at {:.1}%. You may want to upgrade your plan soon.",
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
                    {is_exhausted.then(|| view! {
                        <p class="text-sm text-error-foreground mt-2">
                            "AI budget exhausted. Upgrade to continue using AI features."
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
                            view! { <></> }.into_any()
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
