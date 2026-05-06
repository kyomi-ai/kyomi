# ChartML Quick Reference

**Purpose:** Syntax reference for creating ChartML visualizations. For full docs, see SPECIFICATION.md.

---

## ⚠️ CRITICAL: Markdown Code Block Required

**ALL ChartML must be wrapped in a ```chartml``` markdown code block to render correctly:**

```chartml
type: chart
version: 1
# ... your chart spec
```

**Without the code block fence, the chart will NOT render!**

---

## Basic Structure

```chartml
type: chart
version: 1
title: "Chart Title"       # Optional - shown above chart

data:
  datasource: my-datasource
  query: |
    SELECT category, value FROM table

visualize:
  type: bar | line | area | pie | table | metric
  columns: field_name      # X-axis / categories
  rows: field_name         # Y-axis / values
```

**Data Source:** Use the `datasource` slug from `list_datasources` or `search_catalog` results.

---

## Common Chart Types

### 1. Bar Chart (Vertical)
```chartml
type: chart
version: 1
title: "Revenue by Region"

data:
  datasource: my-datasource
  query: SELECT region, revenue FROM sales

visualize:
  type: bar
  columns: region            # Categories on x-axis
  rows: revenue              # Values on y-axis
  axes:
    rows:
      label: "Revenue ($)"
      format: "$,.0f"
  style:
    height: 400
```

### 2. Bar Chart (Grouped/Colored)
```chartml
type: chart
version: 1
title: "Revenue by Month and Product"

data:
  datasource: my-datasource
  query: SELECT month, product, revenue FROM sales

visualize:
  type: bar
  mode: grouped              # or 'stacked'
  columns: month
  rows: revenue
  marks:
    color: product           # Separate bar per product
```

### 3. Line Chart
```chartml
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
      label: "Sales"
      format: "$,.0f"
  style:
    height: 400
```

### 4. Pie Chart
```chartml
type: chart
version: 1
title: "Sales by Category"

data:
  datasource: my-datasource
  query: |
    SELECT category, SUM(amount) as amount
    FROM sales GROUP BY category

visualize:
  type: pie
  columns: category          # Slice labels
  rows: amount               # Slice sizes
```

### 5. Table (Best for Many Rows)
```chartml
type: chart
version: 1
title: "Top Products"

data:
  datasource: my-datasource
  query: |
    SELECT product, region, revenue, units
    FROM sales
    ORDER BY revenue DESC

visualize:
  type: table
  columns:
    - field: product
      label: "Product Name"
    - field: region
      label: "Region"
    - field: revenue
      label: "Revenue"
      format: "$,.0f"
      align: right
    - field: units
      label: "Units"
      format: ",.0f"
      align: right
```

### 6. Metric Card (Single Number)
```chartml
type: chart
version: 1
title: "Total Revenue"       # Shown above card

data:
  datasource: my-datasource
  query: SELECT SUM(revenue) as total_revenue FROM sales

visualize:
  type: metric
  value: total_revenue
  label: "Revenue"           # Shown inside card
  format: "$,.0f"
  compareWith: previous_month  # Optional: show trend
```

---

## Formatting Numbers

Use the `format` option in axes or metric values:

```chartml
visualize:
  axes:
    rows:
      format: "$,.0f"        # Currency: $1,234
      # or ",.2f"           # Decimal: 1,234.56
      # or ".1%"            # Percent: 45.6%
```

**Common formats:**
- `$,.0f` → $1,234 (currency, no decimals)
- `,.2f` → 1,234.56 (comma separator, 2 decimals)
- `.1%` → 45.6% (percentage, 1 decimal)

---

## Multiple Charts in Grid

```chartml
# Array of charts - each takes half width (6 columns)
- type: chart
  version: 1
  title: "Revenue by Region"
  layout:
    colSpan: 6             # Half width (12-column grid)
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
    colSpan: 6             # Half width
  data:
    datasource: my-datasource
    query: SELECT product, COUNT(*) as count FROM sales GROUP BY product
  visualize:
    type: pie
    columns: product
    rows: count
```

**Grid sizes:** 1-12 columns (12 = full width, 6 = half, 4 = third, 3 = quarter)

---

## Styling Tips

### Chart Height
```chartml
visualize:
  style:
    height: 400            # Default: 400px
```

### Axis Labels
```chartml
visualize:
  axes:
    columns:
      label: "Month"
    rows:
      label: "Revenue ($)"
      format: "$,.0f"
      min: 0               # Force axis minimum
      max: 100000          # Force axis maximum
```

### Horizontal Bar Chart
```chartml
visualize:
  type: bar
  orientation: horizontal  # Flip to horizontal
  columns: category
  rows: value
```

### Mixed Chart with Dual Axes
```chartml
type: chart
version: 1
title: "Daily Volume with Rolling Average"

data:
  datasource: my-datasource
  query: |
    SELECT
      transaction_date,
      COUNT(*) as daily_volume,
      AVG(COUNT(*)) OVER (
        ORDER BY transaction_date
        ROWS BETWEEN 6 PRECEDING AND CURRENT ROW
      ) as rolling_7day_avg
    FROM transactions
    GROUP BY transaction_date
    ORDER BY transaction_date

visualize:
  type: bar
  columns: transaction_date
  rows:
    - field: daily_volume
      mark: bar
      axis: left
    - field: rolling_7day_avg
      mark: line
      axis: right
  axes:
    columns:
      label: "Date"
    rows:
      label: "Daily Volume"
      format: ",.0f"
    right:
      label: "7-Day Average"
      format: ",.1f"
  style:
    height: 400
```

---

## Common Patterns

### Aggregation in SQL
```sql
-- ✅ Aggregate BEFORE charting
SELECT
  region,
  SUM(revenue) as total_revenue
FROM sales
GROUP BY region
ORDER BY total_revenue DESC
```

### Time Series
```sql
-- ✅ Use DATE_TRUNC for time grouping
SELECT
  DATE_TRUNC(order_date, MONTH) as month,
  SUM(revenue) as monthly_revenue
FROM orders
GROUP BY month
ORDER BY month
```

---

## Common Mistakes

❌ **Reversed columns/rows** - columns = categories, rows = values (NEVER reverse!)
❌ **Field name mismatch** - Use EXACT field names from SQL SELECT
❌ **Wrong chart type** - 50+ rows? Use `table`. Single number? Use `metric`.
❌ **Title in wrong place** - Put `title:` at chart level, NOT inside `style` or `visualize`

---

## Quick Decision Tree

**What chart should I use?**

1. Showing a **single number**? → `metric`
2. Showing **many rows** of data (20+)? → `table`
3. Showing **parts of a whole**? → `pie`
4. Showing **trends over time**? → `line`
5. Comparing **categories**? → `bar`
6. Showing **two variables**? → `scatter`

---

## Practical Examples

### Revenue by Region (with formatting)
```chartml
type: chart
version: 1
title: "Revenue by Region"

data:
  datasource: my-datasource
  query: |
    SELECT region, SUM(revenue) as total_revenue
    FROM sales
    GROUP BY region
    ORDER BY total_revenue DESC

visualize:
  type: bar
  columns: region
  rows: total_revenue
  axes:
    rows:
      label: "Revenue ($)"
      format: "$,.0f"
  style:
    height: 400
```

### Monthly Trends (line chart)
```chartml
type: chart
version: 1
title: "Monthly Revenue by Product"

data:
  datasource: my-datasource
  query: |
    SELECT
      DATE_TRUNC(date, MONTH) as month,
      product,
      SUM(revenue) as revenue
    FROM sales
    GROUP BY month, product
    ORDER BY month

visualize:
  type: line
  columns: month
  rows: revenue
  marks:
    color: product
  axes:
    columns:
      label: "Month"
    rows:
      label: "Revenue"
      format: "$,.0f"
  style:
    height: 400
```

### Top Products Table
```chartml
type: chart
version: 1
title: "Top 50 Products"

data:
  datasource: my-datasource
  query: |
    SELECT product, SUM(revenue) as revenue, SUM(units) as units
    FROM sales
    GROUP BY product
    ORDER BY revenue DESC
    LIMIT 50

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
      label: "Units Sold"
      format: ",.0f"
      align: right
```

---

**Need full spec?** Call `get_chartml_spec()` tool or see SPECIFICATION.md for complete documentation.
