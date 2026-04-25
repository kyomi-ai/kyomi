// SPDX-License-Identifier: AGPL-3.0-or-later

//! Skeleton loading placeholders for content loading states.

use leptos::prelude::*;

/// Base classes: `bg-accent animate-pulse rounded-md`.
const BASE: &str = "bg-accent animate-pulse rounded-md";

/// Animated placeholder pulse matching shadcn/ui Skeleton.
#[component]
pub fn Skeleton(
    #[prop(optional, into)]
    class: String,
) -> impl IntoView {
    let classes = format!("{} {}", BASE, class);

    view! {
        <div data-slot="skeleton" class=classes />
    }
}

#[component]
pub fn ListPageSkeleton() -> impl IntoView {
    view! {
        <div class="p-4 md:p-6 @container">
            <div class="grid grid-cols-1 gap-4 @2xl:grid-cols-2 @4xl:grid-cols-3">
                {(0..6).map(|_| view! {
                    <div class="bg-card border border-border rounded-lg p-6 space-y-3">
                        <Skeleton class="h-5 w-3/5" />
                        <Skeleton class="h-4 w-2/5" />
                        <Skeleton class="h-4 w-1/3" />
                    </div>
                }).collect_view()}
            </div>
        </div>
    }
}

#[component]
pub fn DetailPageSkeleton() -> impl IntoView {
    view! {
        <div class="p-4 md:p-6 space-y-6 max-w-[860px]">
            <Skeleton class="h-8 w-2/5" />
            <Skeleton class="h-4 w-3/5" />
            <div class="border border-border rounded-md">
                <div class="px-5 py-4 border-b border-border flex items-center justify-between">
                    <Skeleton class="h-4 w-1/4" />
                    <Skeleton class="h-4 w-16" />
                </div>
                <div class="p-6">
                    <Skeleton class="h-48 w-full" />
                </div>
            </div>
            <Skeleton class="h-6 w-1/3" />
            <Skeleton class="h-4 w-2/5" />
        </div>
    }
}

#[component]
pub fn SettingsPageSkeleton() -> impl IntoView {
    view! {
        <div class="p-4 md:p-6 space-y-6 max-w-[600px]">
            {(0..4).map(|_| view! {
                <div class="space-y-2">
                    <Skeleton class="h-4 w-24" />
                    <Skeleton class="h-10 w-full" />
                </div>
            }).collect_view()}
            <Skeleton class="h-10 w-32" />
        </div>
    }
}

/// Skeleton for a modal that will render a list of selectable items (e.g. dashboards list).
///
/// Mimics the layout of save_dashboard_modal: each row has a square icon placeholder,
/// a title line, and a subtitle line — matching the real DashboardListEntry card dimensions.
/// One prominent "create new" card at the top, then 4 narrower existing-item rows.
#[component]
pub fn ModalListSkeleton() -> impl IntoView {
    view! {
        <div class="space-y-2 min-h-[420px]">
            // "Create new" card placeholder — taller (matches the bordered card with icon + 2 text lines)
            <div class="border-2 border-border rounded-lg p-4">
                <div class="flex items-center gap-3">
                    <Skeleton class="flex-shrink-0 w-10 h-10 rounded-lg" />
                    <div class="flex-1 space-y-1.5">
                        <Skeleton class="h-4 w-2/5" />
                        <Skeleton class="h-3 w-3/5" />
                    </div>
                </div>
            </div>
            // Divider placeholder
            <div class="relative py-2">
                <Skeleton class="h-px w-full" />
            </div>
            // Existing item rows — 4 rows matching DashboardListEntry height
            {(0..4).map(|_| view! {
                <div class="border border-border rounded-lg p-4 flex items-center gap-3">
                    <Skeleton class="flex-shrink-0 w-10 h-10 rounded-lg" />
                    <div class="flex-1 space-y-1.5">
                        <Skeleton class="h-4 w-1/2" />
                        <Skeleton class="h-3 w-1/3" />
                    </div>
                </div>
            }).collect_view()}
        </div>
    }
}

/// Skeleton for a modal or page section that will render a list of alert/event rows.
///
/// Mimics the layout of AlertsHistory: each row has a checkbox placeholder, an icon,
/// a title line, a summary line, and a small metadata row — matching the real alert
/// card collapsed dimensions.
#[component]
pub fn AlertsListSkeleton() -> impl IntoView {
    view! {
        <div class="space-y-4">
            // Toolbar skeleton — filter row
            <div class="flex items-center gap-3 min-h-10">
                <Skeleton class="h-9 w-[200px]" />
                <Skeleton class="h-6 w-16 rounded-full" />
                <Skeleton class="h-6 w-28 ml-auto" />
            </div>
            // Alert row skeletons — 5 rows approximating collapsed alert cards
            {(0..5).map(|_| view! {
                <div class="rounded-lg border border-border flex items-center">
                    // Checkbox placeholder
                    <div class="pl-4 flex items-center shrink-0">
                        <Skeleton class="w-4 h-4 rounded" />
                    </div>
                    // Content area
                    <div class="flex-1 p-4 pl-2 space-y-1.5">
                        <Skeleton class="h-4 w-2/5" />
                        <Skeleton class="h-3 w-3/5" />
                        <Skeleton class="h-3 w-1/3" />
                    </div>
                    // Action buttons placeholder
                    <div class="pr-3 flex items-center gap-1 shrink-0">
                        <Skeleton class="h-8 w-8 rounded-md" />
                        <Skeleton class="h-8 w-8 rounded-md" />
                    </div>
                </div>
            }).collect_view()}
        </div>
    }
}
