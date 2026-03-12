---
layout: doc
title: "ChartML: Config"
description: "ChartML config component — scope-level defaults for styles, datasources, and cache settings."
---

# Config

The config component sets scope-level defaults that apply to all charts within that scope. This avoids repeating `style:` on every chart.

---

## Config Structure

```yaml
type: config
version: 1

# Reference a named style
style: corporate_theme

# OR define inline style
style:
  colors: ["#4285f4", "#ea4335"]
  grid:
    y: true
    color: "#e0e0e0"
  height: 400
```

---

## Usage

### Dashboard with Config

Define a config block in your dashboard markdown. All charts in the dashboard inherit it:

```yaml
type: config
version: 1
style: corporate_theme    # All charts inherit this by default
```

Charts automatically use the config style:

```yaml
type: chart
version: 1
title: "Revenue"
data: sales_data
visualize:
  type: bar
  columns: region
  rows: revenue
  # corporate_theme applied automatically from config
```

Charts can override the config:

```yaml
type: chart
version: 1
style: dark_theme         # Override config default
data: sales_data
visualize:
  type: line
  columns: month
  rows: revenue
```

---

## Resolution Order

When resolving a chart's style, the system cascades through six levels of specificity (most general → most specific):

| Level | Scope | How Set |
|-------|-------|---------|
| 1. System Config | Base defaults | Built-in (`autumn_forest` palette) |
| 2. Workspace Config | Organization | Settings → Chart Styles (admin) |
| 3. User Config | Personal | Settings → Chart Styles (user) |
| 4. Dashboard Config | Dashboard | `type: config` block in dashboard markdown |
| 5. Chart Style Ref | Chart-specific | `style:` field on chart |
| 6. Inline Style | Surgical override | `visualize.style` in chart spec |

Each level is **deep-merged** with the previous — you only override what you specify, and all other properties are inherited.

### Example: Override Chain

```yaml
# Workspace config sets spectrum_pro palette
# Dashboard config overrides with corporate_theme
# Chart overrides just the height

type: chart
version: 1
data: sales_data
visualize:
  type: bar
  columns: region
  rows: revenue
  style:
    height: 600           # Override just height
    # Colors from corporate_theme (dashboard config)
    # Grid from corporate_theme
    # Fonts from corporate_theme
```
