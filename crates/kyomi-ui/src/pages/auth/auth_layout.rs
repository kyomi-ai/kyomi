// SPDX-License-Identifier: AGPL-3.0-or-later

//! Shared layout component for all auth pages (login, signup, recovery).
//!
//! Matches `apps/frontend/src/pages/Login.jsx` layout structure and CSS classes.
//! Left panel: dark background with Kyomi branding (desktop only).
//! Right panel: centered content slot with title, subtitle, footer.

use leptos::prelude::*;
use phosphor_leptos::Icon;

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

            // ── Left side — Heroic nameplate (Option B2) ────────────────────
            // Warm-stone dark panel with huge amber "KYOMI" masthead.
            // Hidden below 1024px (lg breakpoint) — the form fills the whole
            // viewport on mobile and tablets per the Phase 3 responsive spec.
            // Decorative nameplate is `aria-hidden` so screen readers skip it;
            // the small top-left mark remains in the reading order as the
            // branded "masthead" for the page.
            <div class="hidden lg:flex lg:w-1/2 relative overflow-hidden auth-brand-panel">
                // Top-left marginalia — small brand mark with Phosphor SPARKLE.
                // 18px Instrument Serif, 0.16em tracking.
                <div class="absolute top-10 left-12 z-10 flex items-center gap-2 text-[18px] font-display text-[color:rgba(245,243,239,0.85)]" style="letter-spacing:0.16em;">
                    <span aria-hidden="true" class="text-primary inline-flex">
                        <Icon icon=phosphor_leptos::SPARKLE size="18px" />
                    </span>
                    <span>"KYOMI"</span>
                </div>

                // Centered heroic wordmark — decorative, aria-hidden.
                // 180px on desktop, scales down between lg (1024px) and xl (1280px).
                <div
                    class="relative z-0 flex-1 flex items-center justify-center select-none"
                    aria-hidden="true"
                >
                    <span
                        class="font-display text-primary leading-none text-[130px] xl:text-[180px]"
                        style="letter-spacing:0.01em;"
                    >
                        "KYOMI"
                    </span>
                </div>

                // Bottom marginalia — "DATA INTELLIGENCE" / "EST · 2025".
                // Geist Mono 10px small caps, 0.18em tracking, 30% opacity.
                <div class="absolute bottom-10 left-12 right-12 z-10 flex items-center justify-between font-mono text-[10px] uppercase text-[color:rgba(245,243,239,0.30)]" style="letter-spacing:0.18em;">
                    <span>"DATA INTELLIGENCE"</span>
                    <span>"EST \u{00B7} 2025"</span>
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
                        // Subtitle — editorial voice: Instrument Serif italic 18px.
                        // Literal #3D3835 per Phase 3 plan spec: pinned warm-stone
                        // tone that stays correct regardless of dark-mode theme
                        // state (the tokenised `--color-secondary-foreground`
                        // flips to cool slate in dark mode).
                        <p class="text-[18px] text-[color:#3D3835] italic font-display leading-tight mb-8">
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
