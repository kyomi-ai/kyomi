# ChartML v1.0 Specification

**Version:** 1.0
**Date:** 2025-10-25

**Related Documents:**
- **JSON Schema**: [`chartml_schema.json`](./chartml_schema.json) - Machine-readable validation rules
- **Examples**: [`EXAMPLES.md`](./EXAMPLES.md) - Real-world usage patterns
- **Overview**: [`README.md`](./README.md) - Directory index and guidelines

---

## Overview

ChartML is a declarative YAML-based language for creating data visualizations. All ChartML components use the `````chartml``` markdown code block and are distinguished by their `type:` field.

### Component Types

ChartML has five component types:

1. **Source** - Reusable data source definitions
2. **Params** - Dashboard parameter definitions (interactive controls)
3. **Style** - Visual theming and default styling
4. **Config** - Scope-level configuration and defaults
5. **Chart** - Visualization specifications

All components use:
- `````chartml``` markdown code block
- `type:` field to identify component type
- `version: 1` field for versioning

### Block Format

A `````chartml``` block can contain:
- **Single component**: One Source, Params, Style, Config, or Chart object
- **Component array**: An array of any ChartML components (useful for grid layouts)

```chartml
# Single component
type: chart
version: 1
title: "My Chart"
# ...
```

```chartml
# Component array
- type: style
  version: 1
  name: corporate_theme
  # ...

- type: source
  version: 1
  name: sales_data
  # ...

- type: chart
  version: 1
  title: "Chart 1"
  # ...

- type: chart
  version: 1
  title: "Chart 2"
  # ...
```

---

## Component 1: Source

Reusable data source definitions that can be referenced by multiple charts.

### Structure

```chartml
type: source
version: 1
name: source_name           # Required - unique identifier
datasource: "production-postgres"  # Preferred - user-friendly slug
provider: bigquery | clickhouse | postgres | mysql | snowflake | databricks | redshift | sqlserver | synapse | inline | http   # Provider type
query: "SELECT..."          # Required for provider: bigquery/postgres/etc.
rows: [...]                 # Required for provider: inline
url: "https://..."          # Required for provider: http
cache:                      # Optional - cache configuration
  ttl: 6h                   # Time-to-live: <number><unit> where unit is s/m/h/d
  autoRefresh: true         # Optional - enable automatic refresh based on TTL (default: false)
```

**Cache TTL Format:**
- Format: `<number><unit>`
- Units: `s` (seconds), `m` (minutes), `h` (hours), `d` (days)
- Examples: `"30s"`, `"5m"`, `"6h"`, `"24h"`, `"1d"`, `"7d"`

**Auto-Refresh:**
- `autoRefresh: true` - Dashboard will automatically refresh the chart when TTL expires
- `autoRefresh: false` (default) - Chart only refreshes on page load or manual refresh
- Auto-refresh requires datasource admin to allow it (to prevent unexpected costs on pay-per-query sources)
```

### Datasource Selection

Sources can reference datasources in two ways:

**1. Named datasource slug (preferred):**
```chartml
type: source
version: 1
name: quarterly_sales
datasource: "production-postgres"   # User-friendly slug (configured in Settings → Datasources)
query: |
  SELECT region, SUM(revenue) as revenue
  FROM sales.transactions
  GROUP BY region
cache:
  ttl: 12h
```

Slugs are lowercase alphanumeric with hyphens (e.g., `production-postgres`, `analytics-bq`, `staging-clickhouse`).
Users define slugs when creating datasources in Settings → Datasources.

**2. Provider shorthand (auto-resolves when unambiguous):**
```chartml
type: source
version: 1
name: quarterly_sales
provider: bigquery  # Auto-resolves to workspace's BigQuery datasource
query: |
  SELECT region, SUM(revenue) as revenue
  FROM sales.transactions
  GROUP BY region
cache:
  ttl: 12h
```

**Resolution Logic:**
- If `datasource` (slug) is provided, resolve it to the configured datasource
- If only `provider` is provided, auto-resolve to matching datasource:
  - If exactly 1 match: use it
  - If 0 matches: error "No {provider} datasource configured"
  - If 2+ matches: error "Multiple {provider} datasources - please specify datasource slug"

### Examples

**BigQuery Source:**
```chartml
type: source
version: 1
name: quarterly_sales
provider: bigquery
query: |
  SELECT
    region,
    product,
    DATE_TRUNC(sale_date, QUARTER) as quarter,
    SUM(revenue) as revenue,
    COUNT(DISTINCT customer_id) as customers
  FROM `project.dataset.sales`
  WHERE EXTRACT(YEAR FROM sale_date) = 2024
  GROUP BY region, product, quarter
cache:
  ttl: 12h
```

**Inline Source:**
```chartml
type: source
version: 1
name: sample_data
provider: inline
rows:
  - region: "US"
    revenue: 15000
    customers: 120
  - region: "EU"
    revenue: 12000
    customers: 95
  - region: "APAC"
    revenue: 8000
    customers: 67
```

---

## Component 2: Params

Dashboard parameter definitions that create interactive controls.

### Structure

```chartml
type: params
version: 1
name: block_name            # Required for dashboard-level params
params:
  - id: param_id              # Required - unique parameter identifier
    type: multiselect | select | daterange | number | text
    label: "Display Label"    # Required - shown in UI
    options: [...]            # Required for select/multiselect
    default: value            # Required - initial value
    placeholder: "text"       # Optional - for text inputs
    layout:                   # Optional - grid layout
      colSpan: 3              # Grid columns (1-12), defaults by type
```

### Grid Layout

Parameters use a 12-column grid system (same as charts). Each parameter can specify `layout.colSpan` to control width.

**Auto-Calculated Column Span (when `layout.colSpan` not specified):**
- **1 parameter**: 12 columns (full width)
- **2 parameters**: 6 columns each (half width)
- **3 parameters**: 4 columns each (third width)
- **4+ parameters**: 3 columns each (quarter width)

This ensures parameters automatically fill the available space intelligently based on how many you have.

**Custom Column Span Example:**
```chartml
- id: long_search
  type: text
  label: "Search Everything"
  layout:
    colSpan: 8            # Override default (4) to be wider
  placeholder: "Enter search terms..."
  default: ""
```

### Parameter Types

**1. Multiselect** - Checkbox group for multiple selections
```chartml
type: params
name: region_filter
params:
  - id: selected_regions
    type: multiselect
    label: "Regions"
    options: ["US", "EU", "APAC", "LATAM"]
    default: ["US", "EU"]
```
Referenced as: `$region_filter.selected_regions`

**2. Select** - Dropdown for single selection
```chartml
type: params
name: category_filter
params:
  - id: product_category
    type: select
    label: "Category"
    options: ["All", "Electronics", "Clothing", "Home & Garden"]
    default: "All"
```
Referenced as: `$category_filter.product_category`

**3. Date Range** - Start and end date inputs
```chartml
type: params
name: time_filter
params:
  - id: date_range
    type: daterange
    label: "Date Range"
    default:
      start: "2024-01-01"
      end: "2024-12-31"
```
Referenced as: `$time_filter.date_range.start` and `$time_filter.date_range.end`

**4. Number** - Numeric input
```chartml
type: params
name: revenue_filter
params:
  - id: minimum_revenue
    type: number
    label: "Minimum Revenue ($)"
    default: 1000
```
Referenced as: `$revenue_filter.minimum_revenue`

**5. Text** - Text search input
```chartml
type: params
name: search_filter
params:
  - id: search_term
    type: text
    label: "Search Products"
    placeholder: "Enter product name..."
    default: ""
```
Referenced as: `$search_filter.search_term`

### Dashboard-Level vs Chart-Level Parameters

**Dashboard-Level Named Params** (shared across charts):
```chartml
type: params
version: 1
name: dashboard_filters      # Required - unique block name
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
    options: ["US", "EU", "APAC"]
    default: ["US", "EU"]
```

**Chart-Level Inline Params** (private to chart):
```chartml
type: chart
version: 1
title: "Top Revenue Products"

params:  # Chart-specific parameters (no name field)
  - id: top_n
    type: number
    label: "Top N Products"
    default: 10

data: sales_data
transform:
  aggregate:
    measures:
      - column: revenue
        aggregation: sum
        name: total_revenue
    limit: "$top_n"  # Reference chart-level param (no prefix)
```

**Variable Reference Syntax:**
- **Named params**: `$blockname.param_id` (e.g., `$dashboard_filters.selected_regions`)
- **Chart-level params**: `$param_id` (e.g., `$top_n`)

**Resolution Logic:**
- Variable has dot (e.g., `$dashboard_filters.region`) → Look up named params block in registry
- Variable has no dot (e.g., `$top_n`) → Look in current chart's inline params array
- If not found: warning and keep variable as-is

---

## Component 3: Chart

Complete visualization specification using Data → Transform → Visualize pipeline.

### Structure

```chartml
type: chart
version: 1
title: "Chart Title"         # Optional

params:                       # Optional - chart-level parameters
  - id: param_id
    type: multiselect
    # ...

data: source_name             # Reference named Source (string)
# OR
data:                         # Inline Source definition (object)
  datasource: "slug"          # Preferred - use datasource slug from Settings
  provider: bigquery | clickhouse | postgres | mysql | snowflake | databricks | redshift | sqlserver | synapse | inline | http
  query: "SELECT..."          # For SQL providers (bigquery, clickhouse, postgres, etc.)
  rows: [...]                 # For provider: inline
  url: "https://..."          # For provider: http
  cache:                      # Optional
    ttl: 6h
# OR
data:                         # Named data sources (for multi-source charts)
  actuals:                    # Source name → becomes table in aggregate SQL
    datasource: "slug"
    query: "SELECT..."
  forecasts:
    datasource: "slug"
    query: "SELECT..."

transform:                    # Optional - data transformation pipeline
  sql: ...                    # Stage 1: SQL preprocessing
  aggregate: ...              # Stage 2: Declarative aggregation
  forecast: ...               # Stage 3: Time series forecasting

visualize:                    # Required - chart rendering
  type: bar | line | area | scatter | pie | doughnut | table | metric
  columns: field_name
  rows: field_name
  style:                      # Optional - visual styling
    height: 400
  # ... chart-specific options

layout:                       # Optional - grid layout
  colSpan: 12                 # Grid columns (1-12)
```

### Data Layer

Specifies the data source. The `data` attribute is always of type **Source** and can be:

**Option 1: Reference Source (string)**
```chartml
data: quarterly_sales         # References a named Source
```

**Option 2: Inline Data Rows (object)**
```chartml
data:
  provider: inline
  rows:
    - month: "Jan"
      sales: 1200
    - month: "Feb"
      sales: 1350
```

**Option 3: Named datasource slug (preferred for database queries)**
```chartml
data:
  datasource: "production-postgres"  # User-friendly slug
  query: |
    SELECT region, SUM(revenue) as total
    FROM sales.transactions
    GROUP BY region
  cache:
    ttl: 6h
```

Slugs are configured in Settings → Datasources (e.g., `production-postgres`, `analytics-bq`).

**Option 4: Provider shorthand (auto-resolves when unambiguous)**
```chartml
data:
  provider: bigquery  # Auto-resolves to workspace's BigQuery datasource
  query: |
    SELECT region, SUM(revenue) as total
    FROM `project.dataset.sales`
    GROUP BY region
  cache:
    ttl: 6h
```

Use this when your workspace has only one datasource of that type.

**Option 5: Named data sources (for multi-source charts)**

When a chart needs data from multiple sources (e.g., actuals from one database, forecasts from another), use named data sources. Keys are source names, values are source specs:

```chartml
data:
  actuals:
    datasource: "production-postgres"
    query: |
      SELECT month, revenue
      FROM sales.monthly_revenue
      WHERE year = 2025
  forecasts:
    datasource: "analytics-clickhouse"
    query: |
      SELECT month, predicted_revenue, upper_bound, lower_bound
      FROM forecasts.monthly_predictions
      WHERE year = 2025
```

Each named source is fetched independently and made available to the `transform` pipeline as DuckDB tables. Source names become table names in SQL (e.g., `SELECT * FROM actuals JOIN forecasts USING (month)`).

**Restrictions:**
- Source names cannot be reserved words: `datasource`, `provider`, `rows`, `url` (these would be ambiguous with single-source format)
- Named sources are referenced by name in `transform.sql` using `{name}` placeholders
- Each value follows the same format as single-source specs (datasource+query, provider+query, inline, or string reference)

### Transform Pipeline (Optional)

The transform pipeline processes data through up to three stages before visualization. Stages run in fixed order: **sql → aggregate → forecast**. Each stage operates on the output of the previous stage (or the source data if it is the first stage). At least one stage must be present when using `transform:`.

**Pipeline Structure:**
```chartml
transform:
  sql: ...           # Stage 1: SQL preprocessing (joins, CTEs, window functions)
  aggregate: ...     # Stage 2: Declarative aggregation (dimensions, measures, filters)
  forecast: ...      # Stage 3: Time series forecasting
```

**Key rules:**
- Three optional stages run in fixed order: sql → aggregate → forecast
- Each stage operates on the previous stage's output, or source data if first
- At least one stage must be present (schema `anyOf` constraint)
- `additionalProperties: false` — only these three stages are allowed
- All SQL (in the `sql` stage) runs in DuckDB, regardless of the upstream data source provider

---

#### Stage 1: SQL (`transform.sql`)

Raw SQL preprocessing using DuckDB syntax. Use for joins, CTEs, window functions, and any transformation expressible in SQL.

**Type:** `string` or `array of string`

**Key rules:**
- Uses DuckDB SQL syntax (e.g., `DATE_TRUNC('month', sale_date)` — date part comes **first**)
- `{name}` placeholders reference source tables (matching keys in `data:`)
- Multi-statement: array form; last statement's result is used as output
- Multiple named data sources **require** `transform.sql` to join or combine them

**Single named source example:**
```chartml
data:
  sales:
    datasource: "production-postgres"
    query: |
      SELECT region, product, revenue, sale_date
      FROM sales.transactions

transform:
  sql: |
    SELECT
      region,
      DATE_TRUNC('month', sale_date) as month,
      SUM(revenue) as total_revenue
    FROM {sales}
    GROUP BY region, month
    ORDER BY month
```

**Multi-source example (joining data from different datasources):**
```chartml
data:
  actuals:
    datasource: "production-postgres"
    query: |
      SELECT month, revenue FROM sales.monthly
  targets:
    datasource: "analytics-clickhouse"
    query: |
      SELECT month, target_revenue FROM planning.targets

transform:
  sql: |
    SELECT
      a.month,
      a.revenue,
      t.target_revenue,
      a.revenue - t.target_revenue as variance
    FROM {actuals} a
    JOIN {targets} t USING (month)
    ORDER BY a.month
```

**Multi-statement SQL:**

The `sql` property can be a string (single statement) or an array of strings (multi-statement). When using multiple statements, the last statement's result is used for visualization. This is useful for creating intermediate tables or loading DuckDB extensions:

```chartml
data:
  sales:
    datasource: "production-postgres"
    query: "SELECT sale_date, revenue FROM sales.transactions"

transform:
  sql:
    - "CREATE OR REPLACE TABLE monthly AS SELECT DATE_TRUNC('month', sale_date) as month, SUM(revenue) as revenue FROM {sales} GROUP BY 1"
    - "SELECT month, revenue, LAG(revenue) OVER (ORDER BY month) as prev_revenue FROM monthly ORDER BY month"
```

**Parameter references in SQL:**
```chartml
data:
  sales:
    datasource: "production-postgres"
    query: "SELECT region, revenue, sale_date FROM sales.transactions"

transform:
  sql: |
    SELECT region, SUM(revenue) as total_revenue
    FROM {sales}
    WHERE region IN ($dashboard_filters.selected_regions)
      AND sale_date BETWEEN '$time_filter.date_range.start' AND '$time_filter.date_range.end'
    GROUP BY region
    HAVING total_revenue >= $revenue_filter.minimum_revenue
    ORDER BY total_revenue DESC
    LIMIT 100
```

---

#### Stage 2: Declarative Aggregate (`transform.aggregate`)

Declarative aggregation that compiles to DuckDB SQL. Operates on a single table — if joining multiple sources, use the `sql` stage first.

**Required property:** `measures`

**Structure:**
```chartml
transform:
  aggregate:
    dimensions:       # Optional - grouping columns
      - field_name    # String shorthand
      - column: field_name
        name: alias   # Optional rename
        type: string | number | date  # Optional type hint
    measures:         # Required - aggregation expressions
      - column: field_name
        aggregation: sum | count | avg | min | max | countDistinct | median | stddev | variance | percentile25 | percentile50 | percentile75 | percentile90 | percentile95 | percentile99
        name: alias   # Optional rename
      - expression: "price * quantity"
        name: line_total  # Required for expression measures
    filters:          # Optional - row filtering
      combinator: and | or
      rules:
        - field: field_name
          operator: = | != | > | >= | < | <= | in | notIn | contains | startsWith | endsWith | between | isNull | isNotNull
          value: ...
    sort:             # Optional - result ordering
      - field: field_name
        direction: asc | desc
    limit: 100        # Optional - max rows
```

**Aggregation functions (16):** `sum`, `count`, `avg`, `min`, `max`, `countDistinct`, `median`, `stddev`, `variance`, `percentile25`, `percentile50`, `percentile75`, `percentile90`, `percentile95`, `percentile99`

**Filter operators (14):** `=`, `!=`, `>`, `>=`, `<`, `<=`, `in`, `notIn`, `contains`, `startsWith`, `endsWith`, `between`, `isNull`, `isNotNull`

**Filter auto-partitioning:** Filters are automatically partitioned into pre-aggregation (WHERE) or post-aggregation (HAVING) clauses based on whether the filtered field is a measure.

**Declarative aggregate example:**
```chartml
data:
  orders:
    datasource: "production-postgres"
    query: "SELECT region, product, revenue, order_date FROM sales.orders"

transform:
  aggregate:
    dimensions:
      - region
      - column: order_date
        name: month
        type: date
    measures:
      - column: revenue
        aggregation: sum
        name: total_revenue
      - column: revenue
        aggregation: avg
        name: avg_revenue
    filters:
      combinator: and
      rules:
        - field: region
          operator: in
          value: ["US", "EU"]
        - field: total_revenue
          operator: ">="
          value: 10000
    sort:
      - field: total_revenue
        direction: desc
    limit: 50

visualize:
  type: bar
  columns: region
  rows: total_revenue
```

---

#### Stage 3: Forecast (`transform.forecast`)

Time series forecasting that extends historical data with predicted values, confidence intervals, and model selection.

**Required properties:** `timestamp`, `value`

**Structure:**
```chartml
transform:
  forecast:
    timestamp: date_column       # Required - date/timestamp column
    value: numeric_column        # Required - numeric value column
    horizon: 6                   # Optional - periods to forecast (integer, min 1, default: 3)
    confidence_level: 0.95       # Optional - confidence interval width (0-1, default: 0.95)
    model: auto                  # Optional - "auto" | "ets" | "linear" | "exponential" | "logistic" (default: "auto")
    group_by:                    # Optional - per-group forecasts
      - category_column
```

**Output columns:**
- `forecast` — predicted value
- `lower_bound` — lower confidence bound
- `upper_bound` — upper confidence bound
- `is_forecast` — boolean flag (`true` for predicted rows, `false` for historical)

**Anchor row:** The first output row is the last historical observation (with `is_forecast: false`). This enables visual line connection between historical and forecast data.

**Simple forecast example:**
```chartml
data:
  revenue:
    datasource: "production-postgres"
    query: "SELECT month, total_revenue FROM sales.monthly_revenue ORDER BY month"

transform:
  forecast:
    timestamp: month
    value: total_revenue
    horizon: 6
    confidence_level: 0.95
    model: auto

visualize:
  type: line
  columns: month
  rows:
    - field: forecast
      label: "Revenue"
      mark: line
    - mark: range
      upper: upper_bound
      lower: lower_bound
      label: "95% Confidence"
      opacity: 0.15
```

**Forecast with group_by:**
```chartml
data:
  revenue:
    datasource: "production-postgres"
    query: "SELECT month, region, revenue FROM sales.monthly_by_region ORDER BY month"

transform:
  forecast:
    timestamp: month
    value: revenue
    horizon: 3
    group_by:
      - region

visualize:
  type: line
  columns: month
  rows: forecast
  marks:
    color: region
```

---

#### Combined Stages

Stages can be combined. Each stage feeds its output to the next.

**SQL + Forecast (preprocess then forecast):**
```chartml
data:
  sales:
    datasource: "production-postgres"
    query: "SELECT sale_date, revenue FROM sales.transactions"

transform:
  sql: |
    SELECT
      DATE_TRUNC('month', sale_date) as month,
      SUM(revenue) as total_revenue
    FROM {sales}
    GROUP BY 1
    ORDER BY 1
  forecast:
    timestamp: month
    value: total_revenue
    horizon: 6

visualize:
  type: line
  columns: month
  rows: forecast
```

**SQL + Aggregate (join then aggregate):**
```chartml
data:
  orders:
    datasource: "production-postgres"
    query: "SELECT order_id, customer_id, amount FROM sales.orders"
  customers:
    datasource: "production-postgres"
    query: "SELECT customer_id, region FROM sales.customers"

transform:
  sql: |
    SELECT o.order_id, o.amount, c.region
    FROM {orders} o
    JOIN {customers} c USING (customer_id)
  aggregate:
    dimensions:
      - region
    measures:
      - column: amount
        aggregation: sum
        name: total_amount
    sort:
      - field: total_amount
        direction: desc

visualize:
  type: bar
  columns: region
  rows: total_amount
```

**Validation rules:**
- Unnamed source (datasource/provider + query without a name key) **cannot** use `transform` — the query must return ready-to-visualize data
- Multiple named sources **require** `transform.sql` to join or combine them
- Single named source can optionally use any combination of transform stages
- The `aggregate` stage operates on a single table; for joins, use the `sql` stage first

### Visualize Layer

Describes how to render data as visual marks.

**Core Concept:**
- **Columns**: Categories / X-axis (the independent variable - what you're grouping or organizing by)
- **Rows**: Values / Y-axis (the dependent variable - the measurements or quantities)
- **Marks**: Additional encoding channels (color, size, text)

**⚠️ CRITICAL: columns vs rows - NEVER MIX THESE UP!**

The most common mistake is reversing `columns` and `rows`. Remember:
- **`columns:`** = Categories (region, month, product name, etc.)
- **`rows:`** = Numbers (revenue, count, score, etc.)

**Correct:**
```chartml
columns: region      # Categories on x-axis
rows: revenue        # Values on y-axis
```

**Wrong - DO NOT DO THIS:**
```chartml
columns: revenue     # ❌ WRONG - revenue is a value, not a category
rows: region         # ❌ WRONG - region is a category, not a value
```

**Basic Structure:**
```chartml
visualize:
  type: bar | line | area | scatter | pie | doughnut | table | metric
  mode: stacked | grouped | normalized  # Optional, for bar/area
  orientation: vertical | horizontal   # Optional, for bar charts

  columns: field_name
  rows: field_name

  marks:              # Optional
    color: field_name
    size: field_name
    text: field_name

  axes:               # Optional — use semantic keys (recommended)
    columns:            # Category axis (follows orientation)
      label: "Label"
    rows:               # Measure axis (follows orientation)
      label: "Label"
      format: "$,.0f"
      min: 0
      max: 100
    right:              # Secondary measure axis (dual-axis charts)
      label: "Label"
      format: ",.0f"

  annotations:        # Optional - reference lines, bands, markers
    - type: line | band
      axis: left | right | x
      value: number     # For line
      from: number      # For band start
      to: number        # For band end
      label: "text"
      color: "#color"

  style:              # Optional
    height: 400
```

**Chart Types:**

1. **Bar Chart**
```chartml
visualize:
  type: bar
  mode: grouped        # or stacked
  orientation: vertical
  columns: region
  rows: revenue
  marks:
    color: product     # Group by product
  axes:
    rows:
      label: "Revenue ($)"
      format: "$,.0f"
  style:
    height: 400
```

2. **Line Chart**
```chartml
visualize:
  type: line
  columns: month
  rows: revenue
  marks:
    color: region     # Separate line per region
  axes:
    rows:
      format: "$,.0f"
  style:
    height: 400
```

3. **Pie/Doughnut Chart**
```chartml
visualize:
  type: pie           # or doughnut
  columns: category   # Slice labels
  rows: revenue       # Slice sizes
  style:
    height: 400
```

4. **Table**
```chartml
visualize:
  type: table
  columns: [region, product, revenue, units]
  style:
    height: 400
```

5. **Metric Card**
```chartml
visualize:
  type: metric
  value: current_value
  label: "Revenue"             # Optional - label shown inside card
  format: "$,.0f"
  compareWith: previous_value  # Optional - show trend
  invertTrend: false           # Optional - invert trend colors (true = red for increase, green for decrease)
```

**Metric Labeling:**
- `chart.title` (optional) → Label shown **above** card (consistent with all chart types)
- `visualize.label` (optional) → Label shown **inside** card (metric-specific)
- If neither specified, only the formatted value is shown
```

6. **Dual-Axis Chart**
```chartml
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

### lineStyle (Line Dash Patterns)

The `lineStyle` property on rows objects controls the line dash pattern for line mark types. Valid values are `solid` (default), `dashed`, and `dotted`.

```chartml
visualize:
  type: line
  columns: month
  rows:
    - field: actual_revenue
      label: "Actual"
      lineStyle: solid           # Default — solid line

    - field: forecast_revenue
      label: "Forecast"
      lineStyle: dashed          # Dashed line — visually distinct from actuals
      color: "#888888"
```

`lineStyle` is only meaningful for `mark: line` (or chart `type: line`). It is ignored for bar, area, and dot marks.

### mark: range (Confidence Intervals and Bands)

The `range` mark type renders a shaded area between an upper and lower bound. It uses a different object structure than regular rows — instead of `field`, it requires `upper` and `lower` properties:

```chartml
visualize:
  type: line
  columns: month
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
- `mark: range` (required) — identifies this as a range mark
- `upper` (required) — field name for the upper bound
- `lower` (required) — field name for the lower bound
- `label` — display label shown in legend
- `color` — fill color (hex or CSS color)
- `opacity` — fill opacity (0-1, default: 0.15)
- `axis` — which Y-axis to use (`left` or `right`)
- `floor` — clamp the rendered lower bound to this minimum value (e.g., `floor: 0` for non-negative metrics). Does not affect the Y-axis domain.
- `ceiling` — clamp the rendered upper bound to this maximum value (e.g., `ceiling: 100` for percentages). Does not affect the Y-axis domain.

Range marks are typically used alongside line marks to show confidence intervals, prediction bands, or min/max ranges. The range mark inherits the same X-axis (columns) as other rows in the chart.

### Annotations (Reference Lines & Bands)

Add visual markers to highlight goals, targets, or significant values.

**Reference Line (horizontal or vertical):**
```chartml
annotations:
  - type: line
    axis: left              # Which axis to attach to
    value: 150000           # Y-value for horizontal line
    orientation: horizontal
    label: "Goal"
    labelPosition: end      # start | center | end
    color: "#34a853"
    strokeWidth: 2
    dashArray: "5,5"        # Dashed line
```

**Reference Band (range):**
```chartml
annotations:
  - type: band
    axis: left
    from: 140000            # Start of band
    to: 160000              # End of band
    orientation: horizontal
    label: "Target Range"
    color: "#34a853"
    opacity: 0.15
```

**Event Marker (vertical line):**
```chartml
annotations:
  - type: line
    axis: x
    value: "2025-03-15"     # X-value for vertical line
    orientation: vertical
    label: "Product Launch"
    color: "#4285f4"
```

See EXAMPLES.md for complete annotation examples with goals, targets, and event markers.

### Complete Chart Examples

**Example 1: Simple Bar Chart with Named Source**
```chartml
type: chart
version: 1
title: "Revenue by Region"

data:
  sales:
    datasource: "production-postgres"
    query: "SELECT region, revenue FROM sales.quarterly"

transform:
  sql: |
    SELECT region, SUM(revenue) as total_revenue
    FROM {sales}
    GROUP BY region

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

**Example 2: Chart with Chart-Level Inline Parameters**
```chartml
type: chart
version: 1
title: "Top N Products by Revenue"

params:
  - id: top_n
    type: number
    label: "Top N Products"
    default: 10

data:
  sales:
    datasource: "production-postgres"
    query: "SELECT product, revenue FROM sales.transactions"

transform:
  sql: |
    SELECT product, SUM(revenue) as total_revenue
    FROM {sales}
    GROUP BY product
    ORDER BY total_revenue DESC
    LIMIT $top_n

visualize:
  type: bar
  columns: product
  rows: total_revenue
  axes:
    rows:
      label: "Revenue ($)"
      format: "$,.0f"
  style:
    height: 400
```

**Example 3: Inline Data with Multiple Charts in Grid**
```chartml
- type: chart
  version: 1
  title: "Q1 Revenue"
  layout:
    colSpan: 6

  data:
    provider: inline
    rows:
      - region: "US"
        revenue: 25000
      - region: "EU"
        revenue: 18000

  visualize:
    type: bar
    columns: region
    rows: revenue
    style:
      height: 300

- type: chart
  version: 1
  title: "Q1 Customers"
  layout:
    colSpan: 6

  data:
    provider: inline
    rows:
      - region: "US"
        customers: 150
      - region: "EU"
        customers: 120

  visualize:
    type: bar
    columns: region
    rows: customers
    style:
      height: 300
```

---

## Component 4: Style

Reusable visual theming component that defines default appearance for charts.

### Mental Model

Styles are **default bundles** that cascade down the scope hierarchy (system → workspace → user → dashboard → chart), with more specific scopes overriding less specific ones. System defaults are defined in `system-defaults.chartml` and provide built-in themes.

### Structure

```chartml
type: style
version: 1
name: corporate_theme

# Color palette
colors: ["#4285f4", "#ea4335", "#fbbc04", "#34a853"]

# Grid defaults
grid:
  x: false
  y: true
  color: "#e0e0e0"
  opacity: 0.5
  dashArray: "2,2"

# Default height
height: 400

# Line chart defaults
showDots: false
strokeWidth: 2

# Typography (optional)
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

**Core Properties:**
- `colors` - Array of color values for multi-series charts (12-color system with automatic fallback)
- `grid` - Grid line configuration (x/y visibility, color, opacity, dashArray)
- `height` - Default chart height in pixels
- `showDots` - Show dots on line charts (boolean)
- `strokeWidth` - Line thickness (number)
- `legend` - Legend configuration (position, orientation)

**Typography (Optional):**
- `fonts.title` - Title font configuration (family, size, weight, color)
- `fonts.axis` - Axis label font configuration
- `fonts.dataLabel` - Data label font configuration

### Built-in Color Palettes

The system provides three professionally-designed 12-color palettes optimized for data visualization:

**1. autumn_forest** (Default)
- Improved BI palette with accessibility focus
- Colors: Ocean, Amber, Forest, Coral, Violet, Chartreuse, Burgundy, Indigo, Sienna, Teal, Sage, Mauve
- Best for: General purpose dashboards, mixed data types

**2. spectrum_pro**
- Inspired by Tableau with warmer tones
- Colors: Azure, Tangerine, Seafoam, Crimson, Orchid, Marigold, Steel, Jade, Burgundy, Periwinkle, Chartreuse, Slate Blue
- Best for: Professional presentations, warm color schemes

**3. horizon_suite**
- Inspired by Looker with deeper saturation
- Colors: Cobalt, Emerald, Sunset, Lavender, Gold, Teal, Berry, Moss, Peach, Indigo, Pine, Rose
- Best for: High-contrast displays, vibrant visualizations

### Automatic Fallback Colors

The system automatically handles charts with more than 12 series:

- **1-12 series**: Uses the base 12-color palette for maximum contrast
- **13-24 series**: Automatically generates 12 additional desaturated variants by:
  - Reducing saturation by 40%
  - Normalizing luminosity toward mid-range
  - Maintaining hue relationships for consistency
- **25+ series**: Cycles through the combined 24-color palette

**Recommendation**: For charts with >12 categories, consider:
- Filtering data to show top categories
- Using small multiples (separate charts)
- Grouping smaller categories into "Other"

### Usage in Charts

**Reference by name:**
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
  style:
    height: 600           # Override just height
    grid:
      color: "#ff0000"    # Override just grid color
    # Colors, fonts, other grid props still from corporate_theme
```

### Deep Merge Behavior

Chart inline styles are **deep merged** with referenced styles. This allows surgical overrides without losing inherited values.

**Example:**
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

---

## Component 5: Config

Scope-level configuration that sets defaults for all charts within that scope.

### Purpose

Sets scope-level defaults (system/workspace/user/dashboard) without repeating `style:` on every chart. System defaults are defined in `system-defaults.chartml`.

### Structure

```chartml
type: config
version: 1

# Reference named style
style: corporate_theme

# OR inline style definition
style:
  colors: ["#4285f4", "#ea4335"]
  grid:
    y: true
    color: "#e0e0e0"
  height: 400
```

### Usage

**Dashboard with config:**
```chartml
type: config
version: 1
style: corporate_theme  # All charts inherit this by default
```

**Charts automatically inherit:**
```chartml
type: chart
version: 1
title: "Revenue"
data: sales_data
visualize:
  type: bar
  # corporate_theme applied automatically from config
```

**Charts can override:**
```chartml
type: chart
version: 1
style: dark_theme  # Override config default
data: sales_data
visualize:
  type: line
```

### Resolution Order

When resolving a chart's style, the system cascades through six levels of specificity:

**1. System Config** (Base)
   - Defined in `system-defaults.chartml`
   - Provides `autumn_forest` as the default palette
   - All charts inherit these unless overridden

**2. Workspace Config** (Organization Defaults)
   - Set by workspace admins in Settings → Chart Styles
   - Applies to all users in the workspace
   - Accessed via: `/api/v1/workspaces/chartml-config`
   - Example:
     ```yaml
     type: config
     version: 1
     style: spectrum_pro  # All workspace charts use this palette
     ```

**3. User Config** (Personal Defaults)
   - Set by individual users in Settings → Chart Styles
   - Overrides workspace defaults for that user only
   - Accessed via: `/api/v1/users/chartml-config`
   - Example:
     ```yaml
     type: config
     version: 1
     style: horizon_suite  # Override workspace default
     ```

**4. Dashboard Config** (Dashboard Defaults)
   - Defined in dashboard markdown with `type: config` block
   - Applies to all charts in that dashboard
   - Overrides user and workspace defaults
   - Example:
     ```yaml
     type: config
     version: 1
     style: corporate_theme  # Dashboard-specific style
     ```

**5. Chart Style Reference** (Chart-Specific)
   - Defined with `style:` field on chart
   - References a named style by name
   - Overrides all config defaults
   - Example:
     ```yaml
     type: chart
     version: 1
     style: dark_theme  # This chart uses dark_theme
     data: sales_data
     visualize:
       type: bar
     ```

**6. Inline Style** (Surgical Overrides)
   - Defined with `visualize.style` in chart spec
   - Highest specificity - overrides everything
   - Use for one-off customizations
   - Example:
     ```yaml
     type: chart
     version: 1
     data: sales_data
     visualize:
       type: bar
       style:
         height: 600           # Override just height
         colors: ["#ff0000"]   # Override just colors
     ```

**Deep Merge Behavior**: Each level is deep-merged with the previous, so you only override what you specify. All other properties are inherited.

---

## Variable References

Charts can reference parameters using two syntaxes:

**Named Params Reference** (with dot notation):
```chartml
value: "$dashboard_filters.selected_regions"  # References named params block
```

**Chart-Level Params Reference** (no dot):
```chartml
value: "$top_n"  # References inline chart param
```

**Nested Property Access:**
```chartml
value: ["$time_filter.date_range.start", "$time_filter.date_range.end"]
```

**Resolution Logic:**
1. **Has dot** (e.g., `$blockname.param_id`) → Look up named params block in registry
2. **No dot** (e.g., `$param_id`) → Look in current chart's inline params array
3. Variables are resolved before chart execution
4. Resolved values flow through Data → Transform → Visualize pipeline

**Example:**
- `"$dashboard_filters.selected_regions"` → `["US", "EU"]` (from named block state)
- `"$top_n"` → `10` (from chart's inline params state)

---

## Number and Date Formatting

ChartML uses [d3-format](https://d3js.org/d3-format) for number formatting and [d3-time-format](https://d3js.org/d3-time-format) for date formatting.

### Common Number Formats

| Format | Example Output | Description |
|--------|---------------|-------------|
| `$,.0f` | $1,234 | Currency with thousands separator, no decimals |
| `$,.2f` | $1,234.56 | Currency with thousands separator, 2 decimals |
| `,.0f` | 1,234 | Thousands separator, no decimals |
| `.1%` | 12.3% | Percentage with 1 decimal place |
| `.0%` | 12% | Percentage with no decimals |
| `~s` | 1.2K, 3.4M, 5.6B | SI-prefix notation (K, M, B, T) |
| `.2e` | 1.23e+4 | Scientific notation |
| `+,.0f` | +1,234 | Always show sign |

### Usage in ChartML

Formats can be applied to:
- **Axis labels**: `axes.rows.format`
- **Data labels**: `rows.dataLabels.format`
- **Metric values**: `visualize.format`
- **Table columns**: `columns.format`

**Example:**
```chartml
visualize:
  type: bar
  columns: month
  rows:
    field: revenue
    dataLabels:
      show: true
      format: "$,.0f"    # Format data labels
  axes:
    rows:
      label: "Revenue ($)"
      format: "$,.0f"    # Format axis ticks
```

**Reference:** See [d3-format documentation](https://d3js.org/d3-format) for complete format specification.

---

## Grid Layout

Charts can be arranged in a responsive 12-column grid:

```chartml
layout:
  colSpan: 6  # Takes 6 columns (half width)
```

- Default: `colSpan: 12` (full width)
- Responsive: Automatically adjusts for mobile/tablet
- Multiple charts in one block create a grid automatically

---

## Complete Dashboard Example

```markdown
# Sales Dashboard

Dashboard-level named parameters (shared across all charts):

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

Reusable data source:

```chartml
type: source
version: 1
name: sales_data
provider: bigquery
query: |
  SELECT region, product, revenue, sale_date, customers
  FROM `project.dataset.sales`
  WHERE EXTRACT(YEAR FROM sale_date) = 2024
cache:
  ttl: 6h
```

Charts using source and named parameters:

```chartml
- type: chart
  version: 1
  title: "Revenue by Region"
  layout:
    colSpan: 6

  data:
    sales: sales_data       # Reference the Source by name

  transform:
    sql: |
      SELECT region, SUM(revenue) as total_revenue
      FROM {sales}
      WHERE region IN ($dashboard_filters.selected_regions)
        AND sale_date BETWEEN '$dashboard_filters.date_range.start' AND '$dashboard_filters.date_range.end'
      GROUP BY region

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
    style:
      height: 400
```
```

---

**End of Specification**
