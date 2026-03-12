---
layout: doc
title: "ChartML: Parameters"
description: "ChartML params component — interactive dashboard controls with date, select, multiselect, text, and number inputs."
---

# Parameters

Parameters create interactive controls that let users filter and customize dashboard data. ChartML supports five parameter types: date range, select, multiselect, text, and number.

---

## Params Component

```yaml
type: params
version: 1
name: dashboard_filters
params:
  - id: date_range
    type: daterange
    label: "Date Range"
    default:
      start: "2024-01-01"
      end: "2024-12-31"
  - id: selected_regions
    type: multiselect
    label: "Regions"
    options: ["US", "EU", "APAC", "LATAM"]
    default: ["US", "EU"]
```

**Properties:**

| Property | Required | Description |
|----------|----------|-------------|
| `name` | Yes (dashboard-level) | Unique block name for variable references |
| `params` | Yes | Array of parameter definitions |
| `params[].id` | Yes | Unique parameter identifier |
| `params[].type` | Yes | `daterange`, `select`, `multiselect`, `number`, `text` |
| `params[].label` | Yes | Display label shown in UI |
| `params[].default` | Yes | Initial value |
| `params[].options` | For select/multiselect | Available choices |
| `params[].placeholder` | No | Placeholder text for text inputs |
| `params[].layout.colSpan` | No | Grid width (1-12) |

---

## Parameter Types

### Date Range

Start and end date inputs:

```yaml
- id: date_range
  type: daterange
  label: "Date Range"
  default:
    start: "2024-01-01"
    end: "2024-12-31"
```

Referenced as: `$block_name.date_range.start` and `$block_name.date_range.end`

### Select

Single-choice dropdown:

```yaml
- id: product_category
  type: select
  label: "Category"
  options: ["All", "Electronics", "Clothing", "Home & Garden"]
  default: "All"
```

Referenced as: `$block_name.product_category`

### Multiselect

Multiple-choice checkbox group:

```yaml
- id: selected_regions
  type: multiselect
  label: "Regions"
  options: ["US", "EU", "APAC", "LATAM"]
  default: ["US", "EU"]
```

Referenced as: `$block_name.selected_regions`

### Number

Numeric input:

```yaml
- id: minimum_revenue
  type: number
  label: "Minimum Revenue ($)"
  default: 1000
```

Referenced as: `$block_name.minimum_revenue`

### Text

Text search input:

```yaml
- id: search_term
  type: text
  label: "Search Products"
  placeholder: "Enter product name..."
  default: ""
```

Referenced as: `$block_name.search_term`

---

## Parameter Grid Layout

Parameters use the same 12-column grid as charts. When `layout.colSpan` is not specified, it auto-calculates:

- **1 parameter**: 12 columns (full width)
- **2 parameters**: 6 columns each (half width)
- **3 parameters**: 4 columns each (third width)
- **4+ parameters**: 3 columns each (quarter width)

Override with:

```yaml
- id: long_search
  type: text
  label: "Search Everything"
  layout:
    colSpan: 8
  placeholder: "Enter search terms..."
  default: ""
```

---

## Dashboard-Level vs Chart-Level Parameters

### Dashboard-Level (Shared)

Defined in a standalone `type: params` block with a `name`. Shared across all charts in the dashboard:

```yaml
type: params
version: 1
name: dashboard_filters
params:
  - id: date_range
    type: daterange
    label: "Date Range"
    default:
      start: "2024-01-01"
      end: "2024-12-31"
```

### Chart-Level (Private)

Defined inside a chart's `params:` field (no `name`). Private to that chart:

```yaml
type: chart
version: 1
title: "Top Revenue Products"
params:
  - id: top_n
    type: number
    label: "Top N Products"
    default: 10
data:
  datasource: my-datasource
  query: SELECT product, revenue FROM sales
transform:
  sql: |
    SELECT product, SUM(revenue) as total
    FROM {data}
    GROUP BY product
    ORDER BY total DESC
    LIMIT $top_n
visualize:
  type: bar
  columns: product
  rows: total
```

---

## Variable Reference Syntax

### Named Params (with dot notation)

Reference dashboard-level params using `$blockname.param_id`:

```yaml
WHERE region IN ($dashboard_filters.selected_regions)
  AND sale_date BETWEEN '$dashboard_filters.date_range.start'
    AND '$dashboard_filters.date_range.end'
```

### Chart-Level Params (no dot)

Reference chart-level params using `$param_id`:

```yaml
LIMIT $top_n
```

### Resolution Logic

1. **Has dot** (e.g., `$dashboard_filters.region`) → looks up named params block
2. **No dot** (e.g., `$top_n`) → looks in current chart's inline params
3. Variables are resolved before chart execution
4. If not found: warning, variable kept as-is
