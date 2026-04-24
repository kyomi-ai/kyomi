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
