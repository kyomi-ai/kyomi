# ChartML Reusable Components & Scoping

**Version:** 1.0
**Date:** 2025-10-25
**Status:** Design Document

---

## Overview

This document defines ChartML's reusable component system and scoping hierarchy. It enables teams to define common elements once (colors, themes, data sources) and reference them across multiple charts and dashboards.

**Core Principle:** Components are defined at various scope levels (workspace → user → dashboard → chart) and inherit down the hierarchy, with more specific scopes overriding less specific ones.

---

## Component Types

### First-Class Reusable Types

ChartML v1.0 supports these reusable component types:

1. **`type: source`** - Data providers (already in spec)
2. **`type: params`** - Interactive controls (already in spec)
3. **`type: style`** - Visual theming (NEW)
4. **`type: config`** - Scope-level configuration (NEW)
5. **`type: chart`** - Visualization specifications (already in spec)

### What's NOT a First-Class Type (and Why)

**Aggregate** - A stage within the `transform:` pipeline, inline within charts
- Aggregations are chart-specific "last-minute data prep"
- Reusability across different sources is rare
- Measures are context-specific to dimensions/filters
- **Decision:** Keep aggregation as a stage within the `transform:` pipeline, inline within charts (not a first-class reusable type)

**Axes** - Stays inline within charts
- Axis labels are context-specific (not reusable)
- Min/max ranges are data-dependent (not reusable)
- Only formatting is reusable → put in `type: style` instead
- **Decision:** No `type: axes`, formatting goes in style

---

## Scoping Hierarchy

Components are resolved using a five-level hierarchy:

```
system → workspace → user → dashboard → chart
```

### Scope Definitions

| Scope | Location | Purpose |
|-------|----------|---------|
| **System** | `system-defaults.chartml` in codebase | Built-in default styles and configuration |
| **Workspace** | `.chartml/` directory | Team-shared components (corporate theme, shared sources) |
| **User** | `~/.chartml/` directory | Personal preferences and overrides |
| **Dashboard** | Inline in markdown file | Dashboard-specific components |
| **Chart** | Inline in chart definition | Chart-specific overrides |

### File System Structure

```
workspace/
  .chartml/
    sources/
      company_sales.chartml
      marketing_data.chartml
    styles/
      corporate_theme.chartml
      dark_theme.chartml
      presentation_mode.chartml
    config.chartml  # Workspace-level defaults

users/alice/
  .chartml/
    styles/
      alice_custom_theme.chartml
    config.chartml  # User-level defaults

dashboards/
  q4-review.md  # Contains dashboard-level components
```

### Resolution Algorithm

When a chart references `style: corporate_theme`:

1. **Chart scope** - Inline style definition in chart block
2. **Dashboard scope** - `type: style` blocks in same markdown file
3. **User scope** - User's `~/.chartml/styles/corporate_theme.chartml`
4. **Workspace scope** - Team's `.chartml/styles/corporate_theme.chartml`
5. **System scope** - Built-in styles in `system-defaults.chartml`
6. **Error** - Component not found, throw validation error

---

## Component 1: `type: style`

Visual theming component that defines default appearance for charts.

### Mental Model

Styles are **default bundles** that cascade down the scope hierarchy, with more specific scopes overriding less specific ones.

```
System defaults (system-defaults.chartml)
    ↓ overridden by
Workspace style
    ↓ overridden by
User style
    ↓ overridden by
Dashboard style
    ↓ overridden by
Chart inline style
```

### Structure

```chartml
type: style
version: 1
name: corporate_theme

# Color palette (already in spec)
colors: ["#4285f4", "#ea4335", "#fbbc04", "#34a853"]

# Grid defaults (already in spec)
grid:
  x: false
  y: true
  color: "#e0e0e0"
  opacity: 0.5
  dashArray: "2,2"

# Default height (already in spec)
height: 400

# Line chart defaults (already in spec)
showDots: false
strokeWidth: 2

# Typography (NEW - essential for branding)
fonts:
  title:
    family: "Inter, sans-serif"
    size: 18
    weight: 600
    color: "#202124"

  axis:
    family: "Inter, sans-serif"
    size: 12
    color: "#5f6368"

  dataLabel:
    family: "Inter, sans-serif"
    size: 11
    weight: 500
```

### What's Included in v1.0

**Included (immediate value, low complexity):**
- ✅ `colors` - Color palette for multi-series charts
- ✅ `grid` - Grid line appearance (x/y visibility, color, opacity, dash pattern)
- ✅ `height` - Default chart height
- ✅ `showDots` - Line chart dot markers
- ✅ `strokeWidth` - Line thickness
- ✅ `legend` - Legend configuration (position, orientation)
- ✅ `fonts` - Typography (title, axis labels, data labels)

**Deferred to v1.1+ (wait for pain points):**
- ⏭️ Number format presets (e.g., `numberFormats: { currency: "$,.0f" }`)
- ⏭️ Chart-type specific defaults (e.g., `bar: { orientation: vertical }`)
- ⏭️ Axis formatting defaults (axes are too context-specific)

### Usage in Charts

**Top-level reference:**
```chartml
type: chart
version: 1
title: "Revenue Trend"
style: corporate_theme  # Reference style by name

data: sales_data
visualize:
  type: line
  columns: month
  rows: revenue
  # Inherits colors, grid, height, fonts from corporate_theme
```

**Inline override (deep merge):**
```chartml
type: chart
version: 1
style: corporate_theme  # Use as base

data: sales_data
visualize:
  type: bar
  style:  # Override specific properties
    height: 600           # Override just height
    grid:
      color: "#ff0000"    # Override just grid color
    # Colors, fonts, other grid props still from corporate_theme
```

### Deep Merge Behavior

Chart inline styles are **deep merged** with referenced styles:

```chartml
# Referenced style has
grid:
  x: false
  y: true
  color: "#e0e0e0"
  opacity: 0.5

# Chart overrides just color
visualize:
  style:
    grid:
      color: "#ff0000"

# Effective result (deep merge)
grid:
  x: false           # From style
  y: true            # From style
  color: "#ff0000"   # From chart override
  opacity: 0.5       # From style
```

**Shallow merge would be wrong** - it would discard x/y/opacity when overriding color.

---

## Component 2: `type: config`

Scope-level configuration that sets defaults for all charts within that scope.

### Purpose

Sets dashboard-level (or workspace/user-level) defaults without repeating `style:` on every chart.

**Without config:**
```chartml
type: chart
style: corporate_theme  # Repeat
...

type: chart
style: corporate_theme  # Repeat
...

type: chart
style: corporate_theme  # Repeat
...
```

**With config:**
```chartml
type: config
version: 1
style: corporate_theme  # Define once

type: chart  # Inherits corporate_theme
...

type: chart  # Inherits corporate_theme
...

type: chart  # Inherits corporate_theme
...
```

### Structure

```chartml
type: config
version: 1

# Dashboard-level default style
style: theme_name  # String reference to named style

# OR inline style definition
style:
  colors: ["#4285f4", "#ea4335"]
  grid:
    y: true
    color: "#e0e0e0"
  height: 400

# Future extensibility (v1.1+):
# params:
#   default_date_range: last_30_days
# locale: "en-US"
# timezone: "America/New_York"
```

### Resolution with Config

**Style resolution order (updated):**

1. **Start with system config:**
   - System config in `system-defaults.chartml` provides base defaults
   - All charts inherit these unless overridden

2. **Override with workspace/user/dashboard config:**
   - Workspace-level `.chartml/config.chartml`
   - User-level `~/.chartml/config.chartml`
   - Dashboard-level `type: config` block
   - Each level deep merges with the previous

3. **Chart has explicit `style:` field?**
   - Resolve style name (system → workspace → user → dashboard)
   - Deep merge with accumulated config defaults

4. **Chart has inline `visualize.style`?**
   - Deep merge inline overrides with all accumulated defaults

**Charts can always override:**
```chartml
type: config
style: corporate_theme  # Dashboard default

# Chart 1: Uses dashboard default
type: chart
data: sales_data
visualize:
  type: bar
  # corporate_theme applied

# Chart 2: Override to different theme
type: chart
style: dark_theme  # Override dashboard default
data: sales_data
visualize:
  type: line

# Chart 3: Inline override specific properties
type: chart
data: sales_data
visualize:
  type: pie
  style:
    colors: ["#ff0000", "#00ff00"]  # Override colors
    # height, grid, fonts still from corporate_theme
```

### Validation Rules

- **Maximum one `type: config` per scope level**
- Dashboard scope = one config per markdown file
- User scope = one `~/.chartml/config.chartml` file
- Workspace scope = one `.chartml/config.chartml` file

### Future Extensibility

`type: config` is designed to hold other dashboard-level configuration in future versions:

```chartml
type: config
version: 1
style: corporate_theme

# Future additions (v1.1+):
params:  # Dashboard-level parameter defaults
  default_date_range: last_30_days

locale: "en-US"  # Number/date formatting locale
timezone: "America/New_York"  # Timezone for date displays

cache:  # Dashboard-level cache settings
  ttl: 24h
```

---

## Complete Example: Multi-Scope Dashboard

### Workspace-Level Style

**File: `.chartml/styles/corporate_theme.chartml`**
```chartml
type: style
version: 1
name: corporate_theme

colors: ["#4285f4", "#ea4335", "#fbbc04", "#34a853"]
grid:
  y: true
  color: "#e0e0e0"
  opacity: 0.5
height: 400
fonts:
  title:
    family: "Inter, sans-serif"
    size: 18
    weight: 600
```

### Dashboard with Config

**File: `dashboards/q4-review.md`**

```markdown
# Q4 Sales Dashboard

Dashboard configuration - all charts use corporate_theme by default:

```chartml
type: config
version: 1
style: corporate_theme
```

Shared data source:

```chartml
type: source
version: 1
name: q4_sales
provider: bigquery
query: |
  SELECT region, product, revenue, sale_date
  FROM sales
  WHERE quarter = 'Q4-2024'
cache:
  ttl: 6h
```

## Revenue Trend

This chart inherits corporate_theme from config:

```chartml
type: chart
version: 1
title: "Revenue Trend"

data: q4_sales

transform:
  aggregate:
    dimensions: [sale_date]
    measures:
      - column: revenue
        aggregation: sum
        name: total_revenue

visualize:
  type: line
  columns: sale_date
  rows: total_revenue
  # corporate_theme colors, grid, fonts applied automatically
```

## Regional Breakdown

This chart needs to be taller than default:

```chartml
type: chart
version: 1
title: "Revenue by Region"

data: q4_sales

transform:
  aggregate:
    dimensions: [region]
    measures:
      - column: revenue
        aggregation: sum
        name: total_revenue

visualize:
  type: bar
  columns: region
  rows: total_revenue
  style:
    height: 600  # Override just height, keep colors/grid/fonts from theme
```

## Special Dark Chart

This one chart uses a different theme:

```chartml
type: chart
version: 1
title: "Product Analysis"
style: dark_theme  # Override dashboard default

data: q4_sales

transform:
  aggregate:
    dimensions: [product]
    measures:
      - column: revenue
        aggregation: sum
        name: total_revenue

visualize:
  type: pie
  columns: product
  rows: total_revenue
  # dark_theme applied instead of corporate_theme
```
```

---

## Benefits of This System

### 1. DRY Dashboards
Define visual theme once, use in 20 charts. Change colors workspace-wide by editing one file.

### 2. Brand Consistency
Corporate theme defined at workspace level ensures all dashboards follow brand guidelines.

### 3. Personal Customization
Users can override workspace styles with personal preferences without affecting others.

### 4. Scope Flexibility
- **Workspace:** Team-shared components
- **User:** Personal overrides and test data
- **Dashboard:** Dashboard-specific themes
- **Chart:** One-off customizations

### 5. Clean Override Pattern
Deep merge allows surgical overrides (change just height, keep everything else from theme).

### 6. Version Control Friendly
Components evolve independently. Update corporate theme without touching 50 chart definitions.

### 7. Testing & Development
- Workspace = production sources/themes
- User = test data and experimental themes
- Same charts, different scopes

---

## Implementation Considerations

### Parser Changes
- Recognize `type: style` and `type: config` blocks
- Load components from file system (`.chartml/` directories)
- Resolve references across scope hierarchy

### Validation
- Component names must be unique within scope
- Circular references not allowed
- Schema validation for each component type

### Deep Merge Algorithm
Standard recursive merge:
- Objects: Merge keys recursively
- Arrays: Replace (not merge)
- Primitives: Replace (override)

### Cache Invalidation
When workspace/user styles change, invalidate rendered charts that reference them.

---

## Future Enhancements (v1.1+)

**Format Presets:**
```chartml
type: style
numberFormats:
  currency: "$,.0f"
  percentage: ".1%"

# Charts use: format: "{{currency}}"
```

**Chart-Type Defaults:**
```chartml
type: style
bar:
  orientation: vertical
  mode: grouped
table:
  rowHeight: 32
```

**Config Extensions:**
```chartml
type: config
params:
  default_date_range: last_30_days
locale: "en-US"
timezone: "America/New_York"
```

**Aggregate Libraries** (if pain point emerges):
```chartml
type: aggregate
name: profit_calculations
measures:
  - expression: "revenue - cost"
    name: profit
```

---

## Summary

### First-Class Types (v1.0)
- ✅ `type: source` (data providers)
- ✅ `type: params` (interactive controls)
- ✅ `type: style` (visual theming)
- ✅ `type: config` (scope defaults)
- ✅ `type: chart` (visualizations)

### Scoping
- ✅ Five-level hierarchy: system → workspace → user → dashboard → chart
- ✅ System defaults in `system-defaults.chartml` provide built-in themes
- ✅ Resolution cascades from general to specific
- ✅ Deep merge for overrides

### Key Design Decisions
- ✅ Styles are "default bundles" not magic syntax
- ✅ System scope provides discoverable, versioned defaults
- ✅ Config provides scope-level defaults at all levels
- ✅ No first-class aggregate or axes (aggregation is a stage within `transform:`, inline only)
- ✅ Format presets deferred to v1.1

**End of Document**
