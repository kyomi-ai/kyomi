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

use leptos_icons::Icon;

use crate::components::{
    Alert, AlertDescription, AlertVariant, Card, CardContent, CardDescription, CardHeader,
    CardTitle, Skeleton,
};
use crate::server_fns::context::UserContext;
use crate::server_fns::usage::{get_ai_usage_status, UsageData};

use super::billing::format_number;
/// Included analytics events per month for Cloud subscribers.
/// Mirrors `kyomi_core::capability::ANALYTICS_EVENTS_INCLUDED` — defined here
/// because kyomi-core is SSR-only and not available on the WASM target.
const ANALYTICS_EVENTS_INCLUDED: u64 = 100_000;

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

/// Usage settings page content.
///
/// Fetches AI usage status via server function and displays:
/// - Workspace AI Usage card with progress bar
/// - Feature breakdown card with stacked bar chart and legend
/// - Warning/exhaustion alerts
#[component]
pub fn UsagePage() -> impl IntoView {
    let user_ctx = expect_context::<LocalResource<Result<UserContext, ServerFnError>>>();
    let usage_resource = Resource::new(|| (), |_| get_ai_usage_status());

    view! {
        <div class="p-4 sm:p-6">
            <h2 class="text-xl font-display text-foreground mb-6">"Usage"</h2>
            <Transition fallback=move || view! { <UsageLoadingSkeleton/> }>
                {move || Suspend::new(async move {
                    let is_owner = user_ctx.await.map(|c| c.is_owner).unwrap_or(false);
                    match usage_resource.await {
                        Ok(data) => view! { <UsageContent data=data is_owner=is_owner/> }.into_any(),
                        Err(_) => {
                            // Don't leak raw SQL or internal errors to the UI.
                            view! {
                                <UsageContent is_owner=is_owner data=UsageData {
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
fn UsageContent(data: UsageData, is_owner: bool) -> impl IntoView {
    let percentage = data.percentage_used;
    let is_exhausted = data.blocked;
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
                ai_bundle_balance_usd=ai_bundle_balance_usd
                is_owner=is_owner
            />

            // Analytics Events card
            <AnalyticsEventsCard
                events_used=analytics_events_used
                events_included=analytics_events_included
                bundle_events=analytics_bundle_events
                is_owner=is_owner
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
/// percentage label, horizontal progress bar, and bundle balance.
#[component]
fn WorkspaceUsageCard(
    percentage: f64,
    is_exhausted: bool,
    ai_bundle_balance_usd: f64,
    is_owner: bool,
) -> impl IntoView {
    let bar_class = usage_bar_class(percentage, is_exhausted);
    let bar_width = format!("{}%", percentage.min(100.0));

    view! {
        <Card>
            <CardHeader>
                <CardTitle class="flex items-center gap-2">
                    <Icon icon=icondata_lu::LuSparkles width="20" height="20"/>
                    "Workspace AI Usage"
                </CardTitle>
                <CardDescription>"AI credit consumption across the workspace."</CardDescription>
            </CardHeader>
            <CardContent>
                <div class="space-y-4">
                    <div>
                        <div class="flex justify-between items-baseline mb-2">
                            <span class="text-sm font-medium text-foreground">
                                "AI Credits Used"
                            </span>
                            <span class="font-mono tabular-nums text-sm font-medium text-foreground">
                                {format!("{:.1}%", percentage)}
                            </span>
                        </div>
                        <div class="w-full bg-muted rounded-full h-2">
                            <div
                                class=bar_class
                                style:width=bar_width
                            />
                        </div>
                    </div>

                    // Bundle balance row
                    <div class="flex items-center justify-between pt-3 border-t border-border">
                        <div class="flex items-center gap-2">
                            <Icon icon=icondata_lu::LuPackage width="16" height="16" attr:class="text-muted-foreground"/>
                            <div>
                                <div class="text-sm font-medium text-foreground">
                                    "Token Bundle Balance"
                                </div>
                                <div class="text-xs text-muted-foreground">
                                    "Non-expiring. Draw down as AI requests are made."
                                </div>
                            </div>
                        </div>
                        <span class="font-mono tabular-nums text-sm font-medium text-foreground">
                            {format!("${:.2} remaining", ai_bundle_balance_usd)}
                        </span>
                    </div>

                    {is_exhausted.then(|| view! {
                        <div class="rounded-md bg-error/10 border border-error/30 px-3 py-2">
                            <p class="text-xs text-error-foreground">
                                "AI budget exhausted. Add an AI token bundle or connect your own API key to continue."
                            </p>
                        </div>
                    })}
                    {(!is_owner).then(|| view! {
                        <p class="text-xs text-muted-foreground">
                            "Contact your workspace owner to purchase additional AI token bundles."
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
    is_owner: bool,
) -> impl IntoView {
    // Progress bar is against the included monthly quota (what resets).
    // Bundle reserve is shown separately because it's non-expiring.
    let usage_pct = if events_included > 0 {
        ((events_used as f64 / events_included as f64) * 100.0).min(999.0)
    } else {
        0.0
    };

    let over_included = events_used > events_included;
    let bundle_u = bundle_events.max(0) as u64;
    let drawing_from_reserve = over_included && bundle_u > 0;
    let bundle_exhausted = over_included && bundle_u == 0;
    let over_by = events_used.saturating_sub(events_included);

    let bar_class = if bundle_exhausted {
        "h-2 rounded-full transition-all bg-error-foreground"
    } else if drawing_from_reserve {
        "h-2 rounded-full transition-all bg-primary"
    } else if usage_pct >= 80.0 {
        "h-2 rounded-full transition-all bg-warning-foreground"
    } else {
        "h-2 rounded-full transition-all bg-success-foreground"
    };
    let bar_width = format!("{}%", usage_pct.min(100.0));

    view! {
        <Card>
            <CardHeader>
                <CardTitle class="flex items-center gap-2">
                    <Icon icon=icondata_lu::LuActivity width="20" height="20"/>
                    "Analytics Events"
                </CardTitle>
                <CardDescription>"Event usage against your monthly quota and bundle reserve."</CardDescription>
            </CardHeader>
            <CardContent>
                <div class="space-y-4">
                    // Primary usage bar — monthly included quota
                    <div>
                        <div class="flex justify-between items-baseline mb-2">
                            <span class="text-sm font-medium text-foreground">
                                "Events This Month"
                            </span>
                            <span class="font-mono tabular-nums text-sm font-medium text-foreground">
                                {format!(
                                    "{} / {}",
                                    format_number(events_used),
                                    format_number(events_included),
                                )}
                                <span class="text-muted-foreground ml-1.5">
                                    {format!("({:.0}%)", usage_pct)}
                                </span>
                            </span>
                        </div>
                        <div class="w-full bg-muted rounded-full h-2">
                            <div
                                class=bar_class
                                style:width=bar_width
                            />
                        </div>
                        <p class="text-xs text-muted-foreground mt-1.5">
                            "Included monthly quota — resets each billing period."
                        </p>
                    </div>

                    // Bundle reserve row
                    <div class="flex items-center justify-between pt-3 border-t border-border">
                        <div class="flex items-center gap-2">
                            <Icon icon=icondata_lu::LuPackage width="16" height="16" attr:class="text-muted-foreground"/>
                            <div>
                                <div class="text-sm font-medium text-foreground">
                                    "Bundle Reserve"
                                </div>
                                <div class="text-xs text-muted-foreground">
                                    "Non-expiring. Used after monthly quota is consumed."
                                </div>
                            </div>
                        </div>
                        <span class="font-mono tabular-nums text-sm font-medium text-foreground">
                            {format!("{} events", format_number(bundle_u))}
                        </span>
                    </div>

                    // Status messages
                    {drawing_from_reserve.then(|| view! {
                        <div class="rounded-md bg-accent-light/30 border border-accent/30 px-3 py-2">
                            <p class="text-xs text-foreground">
                                <span class="font-medium">"Drawing from bundle reserve: "</span>
                                {format!("{} events used beyond this month's included quota.", format_number(over_by))}
                            </p>
                        </div>
                    })}
                    {bundle_exhausted.then(|| view! {
                        <div class="rounded-md bg-error/10 border border-error/30 px-3 py-2">
                            <p class="text-xs text-error-foreground">
                                "Monthly quota exceeded and no bundle reserve remaining."
                            </p>
                        </div>
                    })}
                    {(!is_owner).then(|| view! {
                        <p class="text-xs text-muted-foreground">
                            "Contact your workspace owner to purchase additional event bundles."
                        </p>
                    })}
                </div>
            </CardContent>
        </Card>
    }
}
