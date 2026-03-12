---
layout: doc
title: "ChartML: Chart & Visualize"
description: "ChartML chart component — all visualization types, axes, marks, annotations, line styles, range marks, and dual-axis charts."
---

# Chart & Visualize

The chart component is the core of ChartML. It combines data, optional transforms, and a visualize layer to render charts.

---

## Chart Structure

```yaml
type: chart
version: 1
title: "Chart Title"

params:                       # Optional - chart-level parameters
  - id: param_id
    type: number
    default: 10

data: source_name             # Reference or inline data source

transform:                    # Optional - data transformation
  sql: ...
  aggregate: ...
  forecast: ...

visualize:                    # Required - chart rendering
  type: bar
  columns: field_name
  rows: field_name

layout:                       # Optional - grid positioning
  colSpan: 6
```

---

## Visualize Layer

The `visualize:` block describes how to render data as visual marks.

**Core concept:**
- **`columns:`** = Categories / X-axis (what you're grouping by)
- **`rows:`** = Values / Y-axis (the measurements)
- **`marks:`** = Additional encoding (color, size, text)

```yaml
visualize:
  type: bar | line | area | scatter | pie | doughnut | table | metric
  mode: stacked | grouped | normalized    # Optional, for bar/area
  orientation: vertical | horizontal      # Optional, for bar

  columns: field_name
  rows: field_name

  marks:
    color: field_name       # Color encoding
    size: field_name        # Size encoding (scatter)
    text: field_name        # Text labels

  axes:
    columns:
      label: "Label"
    rows:
      label: "Label"
      format: "$,.0f"
      min: 0
      max: 100
    right:                  # Secondary axis (dual-axis charts)
      label: "Label"
      format: ",.0f"

  annotations: [...]        # Reference lines and bands

  style:
    height: 400
```

---

## Chart Types

### Bar Chart

```yaml
visualize:
  type: bar
  mode: grouped              # grouped | stacked | normalized
  orientation: vertical      # vertical | horizontal
  columns: region
  rows: revenue
  marks:
    color: product           # Group by product
  axes:
    rows:
      label: "Revenue ($)"
      format: "$,.0f"
  style:
    height: 400
```

### Line Chart

```yaml
visualize:
  type: line
  columns: month
  rows: revenue
  marks:
    color: region            # Separate line per region
  axes:
    rows:
      format: "$,.0f"
  style:
    height: 400
    showDots: true
    strokeWidth: 2
```

### Area Chart

```yaml
visualize:
  type: area
  mode: stacked              # stacked | normalized
  columns: week
  rows:
    - field: hardware
      label: "Hardware"
    - field: software
      label: "Software"
    - field: services
      label: "Services"
  style:
    height: 350
```

### Scatter Plot

```yaml
visualize:
  type: scatter
  columns: revenue           # X-axis
  rows: profit               # Y-axis
  marks:
    color: category          # Color by group
    size: units              # Size by value
  style:
    height: 400
```

### Pie / Doughnut Chart

```yaml
visualize:
  type: pie                  # or doughnut
  columns: category          # Slice labels
  rows: revenue              # Slice sizes
  style:
    height: 400
```

### Table

```yaml
visualize:
  type: table
  columns:
    - field: product
      label: "Product Name"
      width: auto
    - field: revenue
      label: "Revenue"
      format: "$,.0f"
      width: 140
      align: right
    - field: units
      label: "Units Sold"
      format: ",.0f"
      width: 100
      align: right
    - field: growth
      label: "YoY Growth"
      format: ".1%"
      width: 120
  style:
    height: 300
```

Table column properties: `field`, `label`, `format`, `width`, `align` (left/center/right).

### Metric Card

Default height: 120px (shorter than other chart types).

```yaml
visualize:
  type: metric
  value: current_value
  label: "Revenue"           # Label shown inside card
  format: "$,.0f"
  compareWith: previous_value  # Show trend arrow
  invertTrend: false           # true = red for increase, green for decrease
```

**Labeling:**
- `chart.title` → label shown **above** card
- `visualize.label` → label shown **inside** card

---

## Multi-Series Rows

When a chart has multiple Y-axis fields, use an array for `rows`:

```yaml
rows:
  - field: revenue
    label: "Revenue"
    mark: bar                # bar | line | area
    axis: left               # left | right
    color: "#4285f4"
    dataLabels:
      show: true
      position: top          # top | center | bottom
      format: "$,.0f"

  - field: customers
    mark: line
    axis: right
    color: "#34a853"
```

**Row properties:**

| Property | Description |
|----------|-------------|
| `field` | Column name from data |
| `label` | Display name in legend |
| `mark` | Render as `bar`, `line`, or `area` |
| `axis` | Which Y-axis: `left` or `right` |
| `color` | Hex or CSS color |
| `lineStyle` | `solid`, `dashed`, or `dotted` |
| `dataLabels` | Show values on marks |

---

## Dual-Axis Charts

Combine different mark types on separate Y-axes:

```yaml
type: chart
version: 1
title: "Revenue and Customer Growth"
data:
  datasource: my-datasource
  query: |
    SELECT month, SUM(revenue) as revenue, COUNT(DISTINCT customer_id) as customers
    FROM sales GROUP BY month ORDER BY month
visualize:
  type: bar
  columns: month
  rows:
    - field: revenue
      mark: bar
      axis: left
      color: "#4285f4"
    - field: customers
      mark: line
      axis: right
      color: "#34a853"
  axes:
    rows:
      label: "Revenue ($)"
      format: "$,.0f"
    right:
      label: "Customers"
      format: ",.0f"
  style:
    height: 350
```

---

## Line Styles

Set `lineStyle` on any row to control the dash pattern:

```yaml
rows:
  - field: actual
    label: "Actual"
    lineStyle: solid         # Default
  - field: forecast
    label: "Forecast"
    lineStyle: dashed        # Dashed line
  - field: baseline
    label: "Baseline"
    lineStyle: dotted        # Dotted line
```

Available: `solid` (default), `dashed`, `dotted`. Only meaningful for `mark: line`.

---

## Range Marks (Confidence Bands)

Render a shaded area between upper and lower bounds:

```yaml
rows:
  - field: forecast
    label: "Forecast"
    mark: line
    lineStyle: dashed
    color: "#4285f4"

  - mark: range
    upper: upper_bound
    lower: lower_bound
    label: "95% Confidence"
    color: "#4285f4"
    opacity: 0.15
```

**Range mark properties:**

| Property | Required | Description |
|----------|----------|-------------|
| `mark` | Yes | Must be `range` |
| `upper` | Yes | Field name for upper bound |
| `lower` | Yes | Field name for lower bound |
| `label` | No | Legend display name |
| `color` | No | Fill color |
| `opacity` | No | Fill opacity (0-1, default: 0.15) |
| `axis` | No | `left` or `right` |
| `floor` | No | Clamp lower bound minimum |
| `ceiling` | No | Clamp upper bound maximum |

---

## Annotations (Reference Lines & Bands)

Add visual markers for goals, targets, or events.

### Horizontal Reference Line

```yaml
annotations:
  - type: line
    axis: left
    value: 150000
    orientation: horizontal
    label: "Goal"
    labelPosition: end       # start | center | end
    color: "#34a853"
    strokeWidth: 2
    dashArray: "5,5"         # Dashed
```

### Reference Band

```yaml
annotations:
  - type: band
    axis: left
    from: 140000
    to: 160000
    orientation: horizontal
    label: "Target Range"
    color: "#34a853"
    opacity: 0.15
    strokeColor: "#34a853"
    strokeWidth: 1
```

### Vertical Event Marker

```yaml
annotations:
  - type: line
    axis: x
    value: "2025-03-15"
    orientation: vertical
    label: "Product Launch"
    labelPosition: start
    color: "#4285f4"
    strokeWidth: 2
```

---

## Axes Configuration

```yaml
axes:
  columns:                   # Category axis (X)
    label: "Month"
  rows:                      # Measure axis (Y-left)
    label: "Revenue ($)"
    format: "$,.0f"
    min: 0                   # Force axis minimum
    max: 100000              # Force axis maximum
  right:                     # Secondary axis (Y-right)
    label: "Customers"
    format: ",.0f"
```

---

## Default Chart Heights

| Chart Type | Default Height |
|------------|----------------|
| metric | 120px |
| bar, line, area, scatter, pie, doughnut | 400px |
| table | 300px |

Override with `visualize.style.height`.
