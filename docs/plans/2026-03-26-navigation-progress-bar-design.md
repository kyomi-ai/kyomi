# Navigation Progress Bar

**Date:** 2026-03-26
**Status:** Design approved

## Goal

Eliminate the loading flash when navigating between pages. Show a thin top progress bar during navigation while keeping the old page visible until the new one is ready.

## Approach

1. Wire `<Router set_is_routing>` to a global `is_routing` signal
2. Build `<NavigationProgress>` component — thin animated bar at top of viewport
3. Swap page-level `<Suspense>` → `<Transition>` so old content stays visible

## NavigationProgress component

- Fixed position, top of viewport, z-50
- 2px height, `bg-primary` (amber)
- When `is_routing=true`: show bar, animate width 0% → 90% over ~2s (ease-out)
- When `is_routing=false`: snap to 100%, fade out
- Pure CSS animation

## Files to change

| File | Change |
|---|---|
| `components/mod.rs` | Add `navigation_progress` module |
| `components/navigation_progress.rs` | New component |
| `app.rs` | Add `is_routing` signal, wire to Router + NavigationProgress |
| `pages/dashboards/dashboard_viewer.rs` | Suspense → Transition |
| `pages/dashboards/dashboards_list.rs` | Suspense → Transition |
| `pages/dashboards/dashboard_editor.rs` | Suspense → Transition |
| `pages/chat/chat_list.rs` | Suspense → Transition |
| `pages/watches/watches_page.rs` | Suspense → Transition |
| `pages/settings/profile.rs` | Suspense → Transition |
| `pages/settings/datasources.rs` | Suspense → Transition |
| `pages/settings/team.rs` | Suspense → Transition (page-level only) |
| `pages/sql_editor/catalog_tree.rs` | Suspense → Transition |
| `components/layout.rs` | Suspense → Transition (sidebar user data) |

## Not changed

Modal/panel-level Suspense stays as-is: chart_builder, history_panel, save_dashboard_modal, insert_link_modal, alerts_history.

## Implementation order

1. Create NavigationProgress component
2. Wire into app.rs Router
3. Swap Suspense → Transition across pages
4. Test: verify no flash, progress bar appears, old content stays
