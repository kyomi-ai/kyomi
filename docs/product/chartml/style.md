---
layout: doc
title: "ChartML: Style & Formatting"
description: "ChartML style component — color palettes, themes, number and date formatting, typography, and visual customization."
---

# Style & Formatting

Styles define reusable visual themes for charts. ChartML includes built-in color palettes and supports custom themes with typography, grid lines, and chart defaults.

---

## Style Component

```yaml
type: style
version: 1
name: corporate_theme

colors: ["#4285f4", "#ea4335", "#fbbc04", "#34a853"]

grid:
  x: false
  y: true
  color: "#e0e0e0"
  opacity: 0.5
  dashArray: "2,2"

height: 400

showDots: false
strokeWidth: 2

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

### Properties

| Property | Description |
|----------|-------------|
| `name` | Unique identifier for referencing |
| `colors` | Array of color values for multi-series charts |
| `grid` | Grid line configuration (x/y visibility, color, opacity, dashArray) |
| `height` | Default chart height in pixels |
| `showDots` | Show dots on line charts |
| `strokeWidth` | Line thickness |
| `legend` | Legend configuration (position, orientation) |
| `fonts.title` | Title font (family, size, weight, color) |
| `fonts.axis` | Axis label font |
| `fonts.dataLabel` | Data label font |

---

## Built-in Color Palettes

Three professionally-designed 12-color palettes optimized for data visualization:

### autumn_forest (Default)

Improved BI palette with accessibility focus. Best for general-purpose dashboards and mixed data types.

### spectrum_pro

Inspired by Tableau with warmer tones. Best for professional presentations and warm color schemes.

### horizon_suite

Inspired by Looker with deeper saturation. Best for high-contrast displays and vibrant visualizations.

### Automatic Fallback Colors

- **1-12 series**: Uses the base 12-color palette
- **13-24 series**: Generates 12 additional desaturated variants
- **25+ series**: Cycles through the combined 24-color palette

For charts with >12 categories, consider filtering to top categories or grouping smaller ones into "Other".

---

## Using Styles in Charts

### Reference by Name

```yaml
type: chart
version: 1
style: corporate_theme
data: sales_data
visualize:
  type: line
  columns: month
  rows: revenue
  # Inherits colors, grid, height, fonts from corporate_theme
```

### Inline Override (Deep Merge)

Override specific properties while keeping everything else from the referenced style:

```yaml
type: chart
version: 1
style: corporate_theme
data: sales_data
visualize:
  type: bar
  columns: region
  rows: revenue
  style:
    height: 600           # Override just height
    grid:
      color: "#ff0000"    # Override just grid color
    # Colors, fonts, other grid props still from corporate_theme
```

Deep merge means you only override what you specify — all other properties are inherited.

---

## Number Formatting

ChartML uses [d3-format](https://d3js.org/d3-format) for number formatting.

### Common Formats

| Format | Example | Description |
|--------|---------|-------------|
| `$,.0f` | $1,234 | Currency, thousands separator, no decimals |
| `$,.2f` | $1,234.56 | Currency, 2 decimals |
| `,.0f` | 1,234 | Thousands separator, no decimals |
| `,.2f` | 1,234.56 | Thousands separator, 2 decimals |
| `.1%` | 12.3% | Percentage, 1 decimal |
| `.0%` | 12% | Percentage, no decimals |
| `~s` | 1.2K, 3.4M, 5.6B | SI-prefix notation |
| `.2e` | 1.23e+4 | Scientific notation |
| `+,.0f` | +1,234 | Always show sign |

### Where to Apply Formats

```yaml
# Axis tick labels
axes:
  rows:
    format: "$,.0f"

# Data labels on marks
rows:
  field: revenue
  dataLabels:
    show: true
    format: "$,.0f"

# Metric card value
visualize:
  type: metric
  format: "$,.0f"

# Table column
columns:
  - field: revenue
    format: "$,.0f"
```

---

## Date Formatting

ChartML uses [d3-time-format](https://d3js.org/d3-time-format) for date formatting.

### Common Date Formats

| Format | Example | Description |
|--------|---------|-------------|
| `%Y-%m-%d` | 2025-03-15 | ISO date |
| `%b %Y` | Mar 2025 | Abbreviated month + year |
| `%B %d, %Y` | March 15, 2025 | Full date |
| `%m/%d` | 03/15 | Month/day |
| `%b` | Mar | Abbreviated month |

---

## Chart Height Defaults

| Chart Type | Default Height |
|------------|----------------|
| metric | 120px |
| bar, line, area, scatter, pie, doughnut | 400px |
| table | 300px |

Override with `visualize.style.height`:

```yaml
visualize:
  style:
    height: 500
```
