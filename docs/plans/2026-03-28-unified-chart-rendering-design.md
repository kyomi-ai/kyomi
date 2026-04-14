# Unified Chart Rendering Architecture

**Date:** 2026-03-28
**Status:** Approved

## Summary

Replace Kyomi's split inline/remote chart rendering with a single path through `chartml-leptos::ChartMLChart`. Implement a Kyomi-specific `DataSource` plugin that fetches data via server functions, register `chartml-datafusion` for in-browser transforms, switch from the JS `<chart-header-bar>` web component to chartml-leptos's native Rust `ChartHeaderBar`, and wire up type/orientation/mode overrides with reactive state.

## Architecture

```
ChartBlock (thin wrapper)
├── type_override / orientation_override / mode_override signals
├── effective_spec = Memo(apply overrides to original YAML)
├── ChartHeaderBar (chartml-leptos, native Rust)
│   ├── on_type_change → set type_override
│   ├── on_orientation_change → set orientation_override
│   ├── on_mode_change → set mode_override
│   ├── on_refresh → increment refresh_count
│   ├── on_info / on_save / on_edit / on_delete → dashboard callbacks
│   └── Tailwind classes → automatic dark mode
└── ChartMLChart (chartml-leptos)
    ├── spec = effective_spec signal
    ├── chartml = Arc<ChartML> with:
    │   ├── CartesianRenderer, PieRenderer, ScatterRenderer, MetricRenderer
    │   ├── KyomiProxyDataSource (fetches via server function)
    │   ├── DataFusionTransform (in-browser SQL/aggregate/forecast)
    │   └── default_palette (user's preference)
    ├── ResizeObserver → measures container → passes width to render
    ├── Tooltip context → mouse handlers → overlay
    └── Renders SVG reactively on spec/width/param changes
```

## Key Decisions

### 1. Everything goes through ChartMLChart

Implement `KyomiProxyDataSource` (chartml-core `DataSource` trait). Its `fetch()` calls `query_datasource_arrow` server function. Register on ChartML instance. ChartMLChart handles the full pipeline for ALL charts.

Mirrors React's `genericProxyDataSource.js`.

### 2. DataFusion as transform middleware

Register `chartml-datafusion::DataFusionTransform`. Gives SQL joins, aggregations, forecasting in the browser. Same role as React's DuckDB middleware.

### 3. Native Rust ChartHeaderBar replaces JS web component

chartml-leptos's `ChartHeaderBar` uses Tailwind classes (automatic dark mode), typed props and callbacks. Eliminates chart-header.js asset, script tag, CSS variables, and addEventListener wiring.

### 4. Type/orientation/mode override state

Three signals in ChartBlock. Header bar callbacks update them. Memo derives effective YAML spec with overrides applied. Passed to ChartMLChart. Matches React's `ChartWithChrome` pattern.

### 5. Tooltips and dark mode work automatically

ChartMLChart provides tooltip context. ChartHeaderBar uses Tailwind. No extra work.

## What gets deleted

- `RemoteChartState` enum and remote data fetch Effect
- `render_element()` import and usage
- `chart-header.js` asset file
- `<script>` tag for chart-header.js in index.html
- `--chb-*` CSS custom properties
- All addEventListener wiring for web component custom events
- `extract_chart_type/orientation/mode` helpers

## What gets added

- `KyomiProxyDataSource` — ~40 lines, implements `DataSource` trait
- Type/orientation/mode override signals + spec derivation Memo — ~30 lines
- `chartml-datafusion` dependency
- `ChartHeaderBar` import from chartml-leptos
