---
layout: doc
title: "ChartML Reference"
description: "Essential reference for creating ChartML visualizations — chart types, syntax, formatting, grid layout, and common patterns."
---

# ChartML Reference

ChartML is a declarative YAML-based language for creating data visualizations. All ChartML is written inside `` ```chartml `` markdown code blocks.

For full specification details, see the per-section pages linked below.

---

## Basic Structure

Every chart follows the **Data → Transform → Visualize** pipeline:

```yaml
type: chart
version: 1
title: "Chart Title"

data:
  datasource: my-datasource
  query: |
    SELECT category, value FROM table

visualize:
  type: bar
  columns: category      # X-axis / categories
  rows: value            # Y-axis / values
```

**Required fields:**
- `type: chart` and `version: 1` — identifies the component
- `data:` — where the data comes from ([full details](/docs/chartml/source))
- `visualize:` — how to render it ([full details](/docs/chartml/chart))

**Optional fields:**
- `title:` — displayed above the chart
- `transform:` — data processing pipeline ([full details](/docs/chartml/transform))
- `layout:` — grid positioning ([full details](/docs/chartml/grid))
- `params:` — interactive controls ([full details](/docs/chartml/params))
- `style:` — reference a named style theme ([full details](/docs/chartml/style))

---

## Critical Rule: columns vs rows

The most common mistake is reversing `columns` and `rows`:

- **`columns:`** = Categories (region, month, product name) → X-axis
- **`rows:`** = Numbers (revenue, count, score) → Y-axis

```yaml
# ✅ CORRECT
columns: region      # Category on X-axis
rows: revenue        # Value on Y-axis

# ❌ WRONG
columns: revenue     # revenue is a value, not a category
rows: region         # region is a category, not a value
```

---

## Chart Types

### Bar Chart

Best for comparing categories.

```yaml
type: chart
version: 1
title: "Revenue by Region"
data:
  datasource: my-datasource
  query: SELECT region, SUM(revenue) as revenue FROM sales GROUP BY region
visualize:
  type: bar
  columns: region
  rows: revenue
  marks:
    color: product           # Grouped bars by product
  axes:
    rows:
      label: "Revenue ($)"
      format: "$,.0f"
```

Supports `mode: grouped | stacked | normalized` and `orientation: vertical | horizontal`.

### Line Chart

Best for trends over time.

```yaml
type: chart
version: 1
title: "Daily Sales Trends"
data:
  datasource: my-datasource
  query: SELECT date, store, daily_sales FROM sales
visualize:
  type: line
  columns: date
  rows: daily_sales
  marks:
    color: store             # Separate line per store
  axes:
    rows:
      format: "$,.0f"
```

### Area Chart

Like line charts with filled area underneath. Supports `mode: stacked | normalized`.

```yaml
visualize:
  type: area
  mode: stacked
  columns: date
  rows:
    - field: hardware
      label: "Hardware"
    - field: software
      label: "Software"
```

### Pie / Doughnut Chart

For showing parts of a whole.

```yaml
type: chart
version: 1
title: "Sales by Category"
data:
  datasource: my-datasource
  query: SELECT category, SUM(amount) as amount FROM sales GROUP BY category
visualize:
  type: pie              # or doughnut
  columns: category      # Slice labels
  rows: amount           # Slice sizes
```

### Table

Best for many rows of data (20+). Supports sorting, pagination, and column formatting.

```yaml
type: chart
version: 1
title: "Top Products"
data:
  datasource: my-datasource
  query: SELECT product, region, revenue, units FROM sales ORDER BY revenue DESC
visualize:
  type: table
  columns:
    - field: product
      label: "Product Name"
    - field: revenue
      label: "Revenue"
      format: "$,.0f"
      align: right
    - field: units
      label: "Units"
      format: ",.0f"
      align: right
```

### Metric Card

Single key number with optional trend comparison. Default height: 120px.

```yaml
type: chart
version: 1
title: "Total Revenue"
data:
  datasource: my-datasource
  query: SELECT SUM(revenue) as total, SUM(prev_revenue) as prev FROM sales
visualize:
  type: metric
  value: total
  label: "Revenue"           # Shown inside card
  format: "$,.0f"
  compareWith: prev          # Show trend arrow
  invertTrend: false         # true = red for increase
```

### Scatter Plot

For relationships between two numeric variables.

```yaml
visualize:
  type: scatter
  columns: revenue           # X-axis
  rows: profit               # Y-axis
  marks:
    color: category          # Color by group
    size: units              # Size by value
```

---

## Number Formatting

ChartML uses [d3-format](https://d3js.org/d3-format) for numbers. Apply via `axes.rows.format`, `visualize.format`, or table `columns.format`.

| Format | Example | Description |
|--------|---------|-------------|
| `$,.0f` | $1,234 | Currency, no decimals |
| `$,.2f` | $1,234.56 | Currency, 2 decimals |
| `,.0f` | 1,234 | Thousands separator |
| `.1%` | 12.3% | Percentage, 1 decimal |
| `.0%` | 12% | Percentage, no decimals |
| `~s` | 1.2K, 3.4M | SI-prefix notation |
| `+,.0f` | +1,234 | Always show sign |

---

## Grid Layout

Charts use a responsive 12-column grid. Set `layout.colSpan` to control width:

| colSpan | Width |
|---------|-------|
| 12 | Full width (default) |
| 6 | Half width |
| 4 | Third width |
| 3 | Quarter width |

**Side-by-side charts must be in the same code block** using YAML array syntax:

```yaml
- type: chart
  version: 1
  title: "Revenue"
  layout:
    colSpan: 6
  data:
    datasource: my-datasource
    query: SELECT region, SUM(revenue) as revenue FROM sales GROUP BY region
  visualize:
    type: bar
    columns: region
    rows: revenue

- type: chart
  version: 1
  title: "Customers"
  layout:
    colSpan: 6
  data:
    datasource: my-datasource
    query: SELECT region, COUNT(*) as count FROM customers GROUP BY region
  visualize:
    type: pie
    columns: region
    rows: count
```

Separate `` ```chartml `` blocks stack vertically. For full grid documentation, see [Grid Layout](/docs/chartml/grid).

---

## Dual-Axis Charts

Combine different mark types and scales:

```yaml
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
```

---

## Common Mistakes

| Mistake | Fix |
|---------|-----|
| Reversed columns/rows | `columns` = categories, `rows` = values. Never reverse. |
| Field name mismatch | Use EXACT field names from your SQL SELECT aliases |
| Wrong chart type | 20+ rows → `table`. Single number → `metric`. |
| Title in wrong place | Put `title:` at chart level, not inside `style` or `visualize` |
| Separate blocks for side-by-side | Use YAML array in one block (start items with `-`) |

---

## Chart Type Decision Tree

1. Single number? → **metric**
2. Many rows of data (20+)? → **table**
3. Parts of a whole? → **pie** or **doughnut**
4. Trends over time? → **line** or **area**
5. Comparing categories? → **bar**
6. Relationship between two numbers? → **scatter**

---

## Full Specification

For complete details on each component:

- **[Data Sources](/docs/chartml/source)** — Query, inline, multi-source, cache configuration
- **[Parameters](/docs/chartml/params)** — Interactive controls (date, select, number, text)
- **[Chart & Visualize](/docs/chartml/chart)** — All chart types, axes, marks, annotations, line styles
- **[Transform Pipeline](/docs/chartml/transform)** — SQL, aggregate, and forecast stages
- **[Style & Formatting](/docs/chartml/style)** — Color palettes, themes, number/date formatting
- **[Config](/docs/chartml/config)** — Scope-level defaults
- **[Grid Layout](/docs/chartml/grid)** — 12-column grid, multi-chart dashboards
