// SPDX-License-Identifier: AGPL-3.0-or-later

//! Shared layout component for all auth pages (login, signup, recovery).
//!
//! Matches `apps/frontend/src/pages/Login.jsx` layout structure and CSS classes.
//! Left panel: dark background with Kyomi branding (desktop only).
//! Right panel: centered content slot with title, subtitle, footer.

use leptos::prelude::*;

/// Shared layout for all authentication pages.
///
/// Renders the two-panel auth layout from React Login.jsx:
/// - Left: dark branded panel (hidden on mobile, `lg:flex lg:w-1/2`)
/// - Right: centered content with mobile logo, title/subtitle, children slot, and footer
///
/// Title and subtitle are reactive (`Signal<String>`) so the login page can
/// toggle between "Welcome back" / "Create your account" without remounting.
#[component]
pub fn AuthLayout(
    /// Title text (e.g., "Welcome back" or "Create your account").
    #[prop(into)]
    title: Signal<String>,
    /// Subtitle text (e.g., "Sign in to your account to continue").
    #[prop(into)]
    subtitle: Signal<String>,
    /// Main content slot (form fields, buttons, etc.).
    children: Children,
) -> impl IntoView {
    view! {
        // Outer container — theme-aware, respects user's localStorage preference
        <div class="min-h-screen bg-background flex">

            // ── Left side — Branding (desktop only) ─────────────────────────
            // Colors/gradients live in main.css (.auth-brand-panel, .auth-brand-gradient-*)
            // per DESIGN.md "no inline styles for color".
            <div class="hidden lg:flex lg:w-1/2 relative overflow-hidden auth-brand-panel">
                <div class="absolute inset-0 auth-brand-gradient-1"></div>
                <div class="absolute inset-0 opacity-30 auth-brand-gradient-2"></div>
                <div class="relative z-10 flex flex-col justify-center items-center px-12 text-white">
                    <img src="/kyomi_full_logo_white.svg" alt="Kyomi" class="h-24 mb-0"/>
                    <p class="text-2xl font-semibold text-white text-right w-full max-w-xs -mt-6">
                        "Data Intelligence Platform"
                    </p>
                </div>
            </div>

            // ── Right side — Form content ───────────────────────────────────
            // React: className="w-full lg:w-1/2 flex items-center justify-center p-8"
            <div class="w-full lg:w-1/2 flex items-center justify-center p-8">
                // React: className="w-full max-w-md"
                <div class="w-full max-w-md">

                    // ── Mobile logo + title/subtitle ────────────────────────
                    // React: className="text-center mb-8"
                    <div class="text-center mb-8">
                        // Mobile logo — React: className="lg:hidden mb-6"
                        <div class="lg:hidden mb-6">
                            <img src="/kyomi_full_logo.svg" alt="Kyomi" class="h-12 mx-auto dark:hidden"/>
                            <img src="/kyomi_full_logo_white.svg" alt="Kyomi" class="h-12 mx-auto hidden dark:block"/>
                        </div>
                        // Title — page-level, DESIGN.md: 2xl token = 30px = text-3xl, Instrument Serif
                        <h2 class="text-3xl font-display text-foreground mb-2">
                            {title}
                        </h2>
                        // Subtitle — DESIGN.md: sm token = 14px for secondary descriptive text
                        <p class="text-sm text-muted-foreground mb-4">
                            {subtitle}
                        </p>
                    </div>

                    // ── Main content slot ────────────────────────────────────
                    {children()}

                    // ── Footer ───────────────────────────────────────────────
                    // React: className="mt-8 pt-6 border-t border-border space-y-3"
                    <div class="mt-8 pt-6 border-t border-border space-y-3">
                        // React: className="flex justify-center items-center space-x-1 text-sm text-muted-foreground"
                        <div class="flex justify-center items-center space-x-1 text-sm text-muted-foreground">
                            <a
                                href="https://kyomi.ai/privacy"
                                target="_blank"
                                rel="noopener noreferrer"
                                class="hover:text-foreground transition-colors"
                            >
                                "Privacy"
                            </a>
                            <span>"·"</span>
                            <a
                                href="https://kyomi.ai/terms"
                                target="_blank"
                                rel="noopener noreferrer"
                                class="hover:text-foreground transition-colors"
                            >
                                "Terms"
                            </a>
                            <span>"·"</span>
                            <a
                                href="https://kyomi.ai"
                                target="_blank"
                                rel="noopener noreferrer"
                                class="hover:text-foreground transition-colors"
                            >
                                "About"
                            </a>
                            <span>"·"</span>
                            <a
                                href="https://status.kyomi.ai"
                                target="_blank"
                                rel="noopener noreferrer"
                                class="hover:text-foreground transition-colors"
                            >
                                "Status"
                            </a>
                        </div>
                        // React: className="text-xs text-muted-foreground text-center"
                        <p class="text-xs text-muted-foreground text-center">
                            "All trademarks are property of their respective owners."
                        </p>
                    </div>
                </div>
            </div>
        </div>
    }
}
