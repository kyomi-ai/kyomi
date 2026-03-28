# Style Audit Fixes

**Date:** 2026-03-26
**Status:** Design approved

## Goal

Fix all design system violations found in the Leptos UI audit. Bring every page into compliance with `docs/DESIGN_SYSTEM.md`.

## Tasks

### Task 1: Page title sizing and font weight
Standardize all page titles to `text-xl font-semibold` per design system.

**Files:**
- `pages/dashboards/dashboards_list.rs` — `text-2xl font-semibold` → `text-xl font-semibold`
- `pages/knowledge/knowledge_page.rs` — `text-2xl font-bold` → `text-xl font-semibold`
- `pages/settings/settings_shell.rs` — `text-3xl font-bold` → `text-xl font-semibold`
- `pages/settings/billing.rs` — `text-2xl font-bold` → `text-xl font-semibold` (plan title)
- `pages/settings/datasources.rs` — `text-xl font-bold` → `text-xl font-semibold`
- `pages/welcome.rs` — `text-3xl font-bold` → `text-xl font-semibold`
- `pages/connect_setup/connect_setup_page.rs` — `text-3xl font-bold` → `text-xl font-semibold`

### Task 2: Replace inline button styles with Button component
Replace custom inline button classes with the `<Button>` component.

**Files:**
- `pages/dashboards/dashboards_list.rs` — Create dashboard button
- `pages/chat/chat_list.rs` — New Chat button
- `pages/dashboard/dashboard_viewer.rs` — Toolbar buttons
- `pages/sql_editor/mod.rs` — Sidebar toggle buttons

### Task 3: Fix search input focus styles
Replace hardcoded focus styles with design system standard `focus-visible:ring-1 focus-visible:ring-ring`.

**Files:**
- `pages/dashboards/dashboards_list.rs` — search input `focus:ring-2 focus:ring-primary/20` → `focus-visible:ring-1 focus-visible:ring-ring`
- `pages/chat/chat_list.rs` — search input `focus:ring-2 focus:ring-amber-500` → `focus-visible:ring-1 focus-visible:ring-ring`

### Task 4: Header padding and height consistency
Standardize headers to consistent padding.

**Files:**
- `pages/dashboards/dashboards_list.rs` — `min-h-16` → `h-16`, `py-3` → `py-4`
- `pages/chat/chat_page.rs` — `px-4 md:px-12` → `px-4 md:px-6` (px-12 is non-standard)
- Other headers: verify `px-6 py-4` or responsive `px-4 md:px-6 py-4`

### Task 5: Rounded corners, shadows, and overlay fixes
- Replace `rounded-2xl` and `rounded-xl` on cards with `rounded-lg`
- Standardize card shadows to `shadow` (not `shadow-sm`)
- Fix mobile overlay in `layout.rs` from `bg-black/50` to use overlay variable

**Files:**
- `pages/dashboards/dashboards_list.rs` — empty state `rounded-2xl` → `rounded-lg`
- `pages/connect_setup/connect_setup_page.rs` — option cards `rounded-xl` → `rounded-lg`
- `pages/setup/personal_setup.rs` — option cards `rounded-xl` → `rounded-lg`
- `pages/dashboards/dashboard_viewer.rs` — content card `shadow-sm` → `shadow`
- `pages/settings/settings_shell.rs` — tab container `shadow-sm` → `shadow`
- `components/layout.rs` — overlay `bg-black/50` → `bg-overlay`

### Task 6: Settings page padding consistency
Standardize all settings pages to `p-4 sm:p-6` (responsive pattern).

**Files:**
- `pages/settings/profile.rs` — `p-6` → `p-4 sm:p-6`
- `pages/settings/team.rs` — `p-6` → `p-4 sm:p-6`
- `pages/settings/workspace.rs` — `p-6` → `p-4 sm:p-6`
- `pages/settings/usage.rs` — `p-6` → `p-4 sm:p-6`
- `pages/settings/analytics.rs` — `p-6` → `p-4 sm:p-6`
- `pages/settings/datasources.rs` — already `p-4 sm:p-6` (reference)
