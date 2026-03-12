---
layout: doc
title: "ChartML: Grid Layout"
description: "ChartML 12-column grid system — colSpan, multi-chart dashboards, metric card rows, and layout patterns."
---

# Grid Layout

Charts use a responsive 12-column grid system for layout. Control width with `layout.colSpan` and create multi-chart dashboards using component arrays.

---

## Column Spans

```yaml
layout:
  colSpan: 6    # Half width
```

| colSpan | Width | Use For |
|---------|-------|---------|
| 12 | Full width (default) | Single charts, tables |
| 6 | Half width | Side-by-side charts |
| 4 | Third width | Three-column layouts |
| 3 | Quarter width | Metric card rows |

---

## Side-by-Side Charts

**Charts must be in the same `` ```chartml `` code block** using YAML array syntax (items start with `-`) to appear side-by-side:

```yaml
# ✅ CORRECT - Same block, array syntax → side-by-side
- type: chart
  version: 1
  title: "Revenue by Region"
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
  title: "Product Distribution"
  layout:
    colSpan: 6
  data:
    datasource: my-datasource
    query: SELECT product, COUNT(*) as count FROM sales GROUP BY product
  visualize:
    type: pie
    columns: product
    rows: count
```

Separate `` ```chartml `` blocks always stack vertically, even with `colSpan: 6`.

**Rule of thumb:**
- **Same row** → one code block, YAML array
- **Different rows** → separate code blocks

---

## Metric Card Row (4 KPIs)

A common dashboard pattern — four metric cards across the top:

```yaml
- type: chart
  version: 1
  title: "Total Revenue"
  layout:
    colSpan: 3
  data:
    datasource: my-datasource
    query: |
      SELECT SUM(revenue) as current, SUM(prev_revenue) as previous
      FROM sales_summary
  visualize:
    type: metric
    value: current
    format: "$,.0f"
    compareWith: previous

- type: chart
  version: 1
  title: "Active Users"
  layout:
    colSpan: 3
  data:
    datasource: my-datasource
    query: |
      SELECT COUNT(DISTINCT user_id) as current, COUNT(DISTINCT prev_user_id) as previous
      FROM user_activity
  visualize:
    type: metric
    value: current
    format: ",.0f"
    compareWith: previous

- type: chart
  version: 1
  title: "Conversion Rate"
  layout:
    colSpan: 3
  data:
    datasource: my-datasource
    query: |
      SELECT conversion_rate as current, prev_conversion_rate as previous
      FROM metrics_summary
  visualize:
    type: metric
    value: current
    format: ".1%"
    compareWith: previous

- type: chart
  version: 1
  title: "Avg Order Value"
  layout:
    colSpan: 3
  data:
    datasource: my-datasource
    query: |
      SELECT avg_order as current, prev_avg_order as previous
      FROM order_metrics
  visualize:
    type: metric
    value: current
    format: "$,.2f"
    compareWith: previous
```

---

## Component Arrays

A single `` ```chartml `` block can contain any mix of ChartML component types — sources, params, styles, configs, and charts:

```yaml
- type: style
  version: 1
  name: dashboard_theme
  colors: ["#4285f4", "#ea4335", "#fbbc04", "#34a853"]

- type: source
  version: 1
  name: sales_data
  datasource: "production-postgres"
  query: |
    SELECT region, product, revenue, sale_date
    FROM sales.transactions
  cache:
    ttl: 6h

- type: chart
  version: 1
  title: "Revenue by Region"
  style: dashboard_theme
  layout:
    colSpan: 6
  data: sales_data
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

- type: chart
  version: 1
  title: "Revenue by Product"
  style: dashboard_theme
  layout:
    colSpan: 6
  data: sales_data
  transform:
    aggregate:
      dimensions: [product]
      measures:
        - column: revenue
          aggregation: sum
          name: total_revenue
      sort:
        - field: total_revenue
          direction: desc
      limit: 10
  visualize:
    type: bar
    orientation: horizontal
    columns: product
    rows: total_revenue
```

---

## Complete Dashboard Example

A full dashboard with params, source, config, and charts:

````markdown
# Sales Dashboard

```chartml
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

```chartml
type: source
version: 1
name: sales_data
datasource: "production-postgres"
query: |
  SELECT region, product, revenue, sale_date, customers
  FROM sales.transactions
cache:
  ttl: 6h
```

```chartml
- type: chart
  version: 1
  title: "Revenue by Region"
  layout:
    colSpan: 6
  data:
    sales: sales_data
  transform:
    sql: |
      SELECT region, SUM(revenue) as total_revenue
      FROM {sales}
      WHERE region IN ($dashboard_filters.selected_regions)
        AND sale_date BETWEEN '$dashboard_filters.date_range.start'
          AND '$dashboard_filters.date_range.end'
      GROUP BY region
  visualize:
    type: bar
    columns: region
    rows: total_revenue
    axes:
      rows:
        label: "Revenue ($)"
        format: "$,.0f"

- type: chart
  version: 1
  title: "Customer Count"
  layout:
    colSpan: 6
  data:
    sales: sales_data
  transform:
    sql: |
      SELECT region, COUNT(DISTINCT customers) as unique_customers
      FROM {sales}
      WHERE region IN ($dashboard_filters.selected_regions)
      GROUP BY region
  visualize:
    type: bar
    columns: region
    rows: unique_customers
```
````
