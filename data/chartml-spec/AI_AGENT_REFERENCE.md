# ChartML Quick Reference for AI Agents

**Version:** 1.0

---

## ⚠️ CRITICAL: Output Format is YAML, NOT JSON

**ChartML uses YAML syntax within markdown code blocks.** The JSON schema is for validation only.

**ALWAYS output YAML like this:**

```chartml
type: chart
version: 1
title: "Revenue by Region"
data:
  provider: bigquery
  query: |
    SELECT
      region,
      SUM(revenue) as revenue
    FROM `project.dataset.sales`
    GROUP BY region
visualize:
  type: bar
  columns: region
  rows: revenue
```

**NEVER output JSON** like `{"type": "chart", ...}`. Always use YAML syntax.

---

## Default Chart Heights

When the user asks to change height (e.g., "make it shorter", "make it taller"), these are the baselines:

| Chart Type | Default Height |
|------------|----------------|
| metric | 120px |
| bar, line, area, scatter, pie, doughnut | 400px |
| table | 300px |

To change height, set `visualize.style.height` (in pixels).

---

## Basic Chart Structure

Every chart follows the **Data → Transform → Visualize** pipeline:

```chartml
type: chart
version: 1
title: "Chart Title"                    # Optional

data: source_name                       # Reference named source
# OR BigQuery query:
data:
  provider: bigquery
  query: "SELECT..."                    # For BigQuery
  cache:
    ttl: 6h                             # Cache duration: 30s, 5m, 6h, 24h, 7d
    autoRefresh: true                   # Optional - auto-refresh when TTL expires

transform:                              # Optional - data transformation pipeline
  sql: |                                # Stage 1: SQL preprocessing (joins, CTEs, window functions)
    SELECT ... FROM {name}              # {name} matches keys in data:
  aggregate:                            # Stage 2: Declarative aggregation
    dimensions: [...]
    measures: [...]
  forecast:                             # Stage 3: Time series forecasting
    timestamp: date
    value: revenue
    horizon: 6

visualize:                              # Required - how to render
  type: bar | line | area | scatter | pie | doughnut | table | metric
  columns: field_name                   # Categories (X-axis)
  rows: field_name                      # Values (Y-axis)
  style:
    height: 400

layout:                                 # Optional - grid positioning
  colSpan: 6                            # 1-12 columns (default: 12)
```

---

## ⚠️ columns vs rows - NEVER Mix These Up!

**Most common mistake**: Reversing `columns` and `rows`

- **`columns:`** = Categories (region, month, product name, etc.) → X-axis
- **`rows:`** = Numbers (revenue, count, score, etc.) → Y-axis

```chartml
# ✅ CORRECT
visualize:
  type: bar
  columns: region      # Category on X-axis
  rows: revenue        # Value on Y-axis

# ❌ WRONG - DO NOT DO THIS
visualize:
  type: bar
  columns: revenue     # ❌ revenue is a value, not a category
  rows: region         # ❌ region is a category, not a value
```

---

## Chart Types

### 1. Bar Chart (with grouped mode, formatting, and data labels)

```chartml
type: chart
version: 1
title: "Revenue by Region and Product"
data:
  provider: bigquery
  query: |
    SELECT
      region,
      product,
      SUM(revenue) as revenue
    FROM `project.dataset.sales`
    WHERE sale_date >= DATE_SUB(CURRENT_DATE(), INTERVAL 30 DAY)
    GROUP BY region, product
  cache:
    ttl: 6h
visualize:
  type: bar
  mode: grouped                         # grouped | stacked | normalized
  orientation: vertical                 # vertical | horizontal
  columns: region
  rows:
    field: revenue
    dataLabels:                         # Show values on bars
      show: true
      position: top                     # top | center | bottom
      format: "$,.0f"
  marks:
    color: product                      # Group by product
  axes:
    rows:
      label: "Revenue ($)"
      format: "$,.0f"
      min: 0                            # Override auto-scaling
      max: 50000
  style:
    height: 400
    grid:
      y: true
      color: "#e0e0e0"
      opacity: 0.5
```

### 2. Line Chart (with multiple series and annotations)

```chartml
type: chart
version: 1
title: "Monthly Revenue Trend with Goal"
data:
  provider: bigquery
  query: |
    SELECT
      FORMAT_DATE('%b', sale_date) as month,
      SUM(CASE WHEN region = 'US' THEN revenue ELSE 0 END) as us,
      SUM(CASE WHEN region = 'EU' THEN revenue ELSE 0 END) as eu
    FROM `project.dataset.sales`
    WHERE sale_date >= DATE_SUB(CURRENT_DATE(), INTERVAL 3 MONTH)
    GROUP BY month, EXTRACT(MONTH FROM sale_date)
    ORDER BY EXTRACT(MONTH FROM sale_date)
  cache:
    ttl: 1h
visualize:
  type: line
  columns: month
  rows:
    - field: us
      label: "US Revenue"
      dataLabels:
        show: true
        format: "$,.0f"
    - field: eu
      label: "EU Revenue"
  axes:
    rows:
      label: "Revenue ($)"
      format: "$,.0f"
  annotations:                          # Reference lines and bands
    - type: line
      axis: left
      value: 150000
      orientation: horizontal
      label: "Goal"
      labelPosition: end                # start | center | end
      color: "#34a853"
      strokeWidth: 2
      dashArray: "5,5"
  style:
    height: 400
    showDots: true
    strokeWidth: 2
```

### 3. Area Chart (with stacked mode)

```chartml
type: chart
version: 1
title: "Revenue Composition Over Time"
data:
  provider: bigquery
  query: |
    SELECT
      FORMAT_DATE('%Y-W%V', sale_date) as week,
      SUM(CASE WHEN category = 'Hardware' THEN revenue ELSE 0 END) as hardware,
      SUM(CASE WHEN category = 'Software' THEN revenue ELSE 0 END) as software,
      SUM(CASE WHEN category = 'Services' THEN revenue ELSE 0 END) as services
    FROM `project.dataset.sales`
    WHERE sale_date >= DATE_SUB(CURRENT_DATE(), INTERVAL 8 WEEK)
    GROUP BY week, sale_date
    ORDER BY sale_date
  cache:
    ttl: 12h
visualize:
  type: area
  mode: stacked                         # stacked | normalized
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

### 4. Dual-Axis Chart (bar + line combo)

```chartml
type: chart
version: 1
title: "Revenue and Customer Growth"
data:
  provider: bigquery
  query: |
    SELECT
      FORMAT_DATE('%b', sale_date) as month,
      SUM(revenue) as revenue,
      COUNT(DISTINCT customer_id) as customers
    FROM `project.dataset.sales`
    WHERE sale_date >= DATE_SUB(CURRENT_DATE(), INTERVAL 3 MONTH)
    GROUP BY month, EXTRACT(MONTH FROM sale_date)
    ORDER BY EXTRACT(MONTH FROM sale_date)
  cache:
    ttl: 6h
visualize:
  type: bar
  columns: month
  rows:
    - field: revenue
      mark: bar                         # bar | line | area
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

### 5. Scatter Plot (with size and color encoding)

```chartml
type: chart
version: 1
title: "Revenue Efficiency Analysis"
data:
  provider: bigquery
  query: |
    SELECT
      product_name as product,
      SUM(revenue) as revenue,
      SUM(profit) as profit,
      SUM(units_sold) as units,
      MAX(category) as category
    FROM `project.dataset.product_performance`
    WHERE sale_date >= DATE_SUB(CURRENT_DATE(), INTERVAL 30 DAY)
    GROUP BY product_name
  cache:
    ttl: 12h
visualize:
  type: scatter
  columns: revenue                      # X-axis
  rows: profit                          # Y-axis
  marks:
    color: category                     # Color by category
    size: units                         # Size by units
  style:
    height: 400
```

### 6. Pie/Doughnut Chart

```chartml
type: chart
version: 1
title: "Market Share by Category"
data:
  provider: bigquery
  query: |
    SELECT
      category,
      SUM(revenue) as revenue
    FROM `project.dataset.sales`
    WHERE sale_date >= DATE_SUB(CURRENT_DATE(), INTERVAL 90 DAY)
    GROUP BY category
  cache:
    ttl: 24h
visualize:
  type: pie                             # pie | doughnut
  columns: category                     # Slice labels
  rows: revenue                         # Slice sizes
  style:
    height: 400
```

### 7. Table (with formatted columns)

```chartml
type: chart
version: 1
title: "Product Performance"
data:
  provider: bigquery
  query: |
    SELECT
      product_name as product,
      SUM(revenue) as revenue,
      SUM(units_sold) as units,
      AVG(growth_rate) as growth
    FROM `project.dataset.product_metrics`
    WHERE sale_date >= DATE_SUB(CURRENT_DATE(), INTERVAL 30 DAY)
    GROUP BY product_name
    ORDER BY revenue DESC
    LIMIT 20
  cache:
    ttl: 6h
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
    - field: units
      label: "Units Sold"
      format: ",.0f"
      width: 100
    - field: growth
      label: "YoY Growth"
      format: ".1%"
      width: 120
  style:
    height: 300
```

### 8. Metric Card (KPI with trend comparison)

**Default height: 120px** (much shorter than other charts which default to 400px)

```chartml
type: chart
version: 1
title: "Total Revenue"                  # Optional - label above card
layout:
  colSpan: 3                            # Use 3 columns in grid
data:
  provider: bigquery
  query: |
    SELECT
      SUM(CASE WHEN sale_date >= DATE_SUB(CURRENT_DATE(), INTERVAL 30 DAY)
        THEN revenue ELSE 0 END) as current,
      SUM(CASE WHEN sale_date >= DATE_SUB(CURRENT_DATE(), INTERVAL 60 DAY)
        AND sale_date < DATE_SUB(CURRENT_DATE(), INTERVAL 30 DAY)
        THEN revenue ELSE 0 END) as previous
    FROM `project.dataset.sales`
  cache:
    ttl: 1h
visualize:
  type: metric
  value: current
  label: "Revenue"                      # Optional - label inside card
  format: "$,.0f"
  compareWith: previous                 # Show trend vs previous
  invertTrend: false                    # true = red for increase, green for decrease
```

---

## Data Layer

### Option 1: Single Source Query (with cache) - PREFERRED

```chartml
data:
  provider: bigquery
  query: |
    SELECT
      region,
      product,
      SUM(revenue) as revenue,
      COUNT(DISTINCT customer_id) as customers
    FROM `project.dataset.sales`
    WHERE EXTRACT(YEAR FROM sale_date) = EXTRACT(YEAR FROM CURRENT_DATE())
    GROUP BY region, product
  cache:
    ttl: 6h                             # 30s, 5m, 6h, 24h, 7d
```

### Option 2: Named Data Sources (Required for Multi-Source Joins)

Use named data sources when combining data from multiple sources, or when using `{name}` placeholders in transform SQL. Keys become table names in transform SQL. **Note: `cache:` goes INSIDE each named source.**

```chartml
type: chart
version: 1
title: "Actuals vs Forecast"
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
transform:
  sql: |
    SELECT
      a.month,
      a.revenue as actual,
      f.predicted_revenue as forecast,
      f.upper_bound,
      f.lower_bound
    FROM {actuals} a
    JOIN {forecasts} f USING (month)
    ORDER BY a.month
visualize:
  type: line
  columns: month
  rows:
    - field: actual
      label: "Actual Revenue"
    - field: forecast
      label: "Forecast"
      lineStyle: dashed
  axes:
    rows:
      format: "$,.0f"
```

**Rules:**
- Source names cannot be reserved words: `datasource`, `provider`, `rows`, `url`
- Use `{name}` placeholders in transform SQL to reference each source
- Each source is fetched independently, then available as DuckDB tables

### Option 3: Reference Named Source

First, define a reusable source:

```chartml
type: source
version: 1
name: sales_data
provider: bigquery
query: |
  SELECT region, product, revenue, sale_date
  FROM `project.dataset.sales`
  WHERE EXTRACT(YEAR FROM sale_date) = EXTRACT(YEAR FROM CURRENT_DATE())
cache:
  ttl: 12h
```

Then reference it in charts:

```chartml
type: chart
version: 1
title: "Revenue by Region"
data: sales_data                        # Reference by name
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
```

---

## Transform Pipeline

The `transform:` block contains three optional stages that run in a fixed order: **sql → aggregate → forecast**. Each stage operates on whatever the previous stage produced, or directly on the source data if it's the first stage. At least one stage must be present; otherwise omit `transform:` entirely.

Both unnamed (single source) and named data sources work with transform:

```yaml
# ✅ Simple — single source with transform (automatically normalized)
data:
  datasource: my-datasource
  query: "SELECT ..."
transform:
  forecast:
    timestamp: date
    value: revenue
    horizon: 6

# ✅ Named — required for multi-source joins or {name} placeholders in sql stage
data:
  sales:
    datasource: my-datasource
    query: "SELECT ..."
    cache:
      ttl: 6h                   # cache goes INSIDE the named source
transform:
  sql: |
    SELECT * FROM {sales} WHERE revenue > 0
```

**Stages:**

```yaml
transform:
  sql: ...           # Stage 1: SQL preprocessing (joins, CTEs, window functions)
  aggregate: ...     # Stage 2: Declarative aggregation (dimensions, measures, filters)
  forecast: ...      # Stage 3: Time series forecasting
```

### Stage 1: SQL (`transform.sql`)

SQL preprocessing in DuckDB. Use for multi-source joins, CTEs, window functions, CASE WHEN, and anything beyond standard aggregation.

- Use `{name}` placeholders matching keys in `data:` to reference source tables
- Can be a string (single statement) or array (multi-statement — last statement's result is used)

#### Single-Source SQL

```chartml
data:
  transactions:
    datasource: "production-postgres"
    query: "SELECT region, revenue, customer_id FROM sales.transactions"

transform:
  sql: |
    SELECT
      region,
      SUM(revenue) as total_revenue,
      COUNT(DISTINCT customer_id) as customers
    FROM {transactions}
    GROUP BY region
    ORDER BY total_revenue DESC
```

#### Multi-Source Join

Use `{name}` placeholders matching keys in `data:`:

```chartml
data:
  sales:
    datasource: "production-postgres"
    query: "SELECT region, revenue FROM sales.transactions"
  targets:
    datasource: "analytics-clickhouse"
    query: "SELECT region, target FROM planning.regional_targets"

transform:
  sql: |
    SELECT
      s.region,
      SUM(s.revenue) as revenue,
      t.target,
      SUM(s.revenue) - t.target as variance
    FROM {sales} s
    JOIN {targets} t USING (region)
    GROUP BY s.region, t.target
    ORDER BY variance DESC
```

#### Multi-Statement SQL

Use an array for multi-statement SQL. The last statement's result is used for visualization:

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

#### Parameter References in SQL

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
    LIMIT $top_n
```

### Stage 2: Aggregate (`transform.aggregate`)

Declarative aggregation without writing SQL. **Always prefer this over the SQL stage for standard group-by aggregation on a single source.**

The aggregate stage operates on a single table. For multiple sources, use the SQL stage first to join them.

**Properties:**

- **`dimensions`** — Group-by columns. String shorthand (`"region"`) or object (`{ column, name, type }`). Temporal types for date truncation: `day`, `week`, `month`, `quarter`, `year`.
- **`measures`** (required) — Aggregations. `{ column, aggregation, name }` for direct aggregation, `{ expression, name }` for calculated measures. 16 aggregation functions: `sum`, `avg`, `count`, `countDistinct`, `min`, `max`, `median`, `stddev`, `variance`, `percentile25`, `percentile50`, `percentile75`, `percentile90`, `percentile95`, `percentile99`.
- **`filters`** — `{ combinator, rules }`. Rules referencing dimensions go to WHERE (pre-aggregation); rules referencing measures go to HAVING (post-aggregation). Operators: `=`, `!=`, `<`, `>`, `<=`, `>=`, `contains`, `startsWith`, `endsWith`, `isNull`, `isNotNull`, `in`, `notIn`, `between`.
- **`sort`** — Array of `{ field, direction }` where direction is `asc` or `desc`.
- **`limit`** — Maximum rows returned.

#### Aggregate-Only Example

```chartml
type: chart
version: 1
title: "Revenue by Region"
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
```

#### Aggregate with Temporal Dimensions, Filters, and Calculated Measures

```chartml
data:
  sales:
    datasource: "production-postgres"
    query: "SELECT sale_date, region, revenue, units FROM sales.transactions"

transform:
  aggregate:
    dimensions:
      - column: sale_date
        type: month
        name: month
      - region
    measures:
      - column: revenue
        aggregation: sum
        name: total_revenue
      - column: units
        aggregation: count
        name: order_count
      - expression: total_revenue / order_count
        name: avg_order_value
    filters:
      combinator: and
      rules:
        - field: region
          operator: "!="
          value: "Test"
        - field: total_revenue
          operator: ">="
          value: 10000
    sort:
      - field: month
        direction: asc
    limit: 100
```

### Stage 3: Forecast (`transform.forecast`)

Time series forecasting with prediction intervals. **Always prefer this for standard forecasting over manual SQL with the `forecast()` function.**

**Schema:**

```yaml
transform:
  forecast:
    timestamp: date              # Required — the time column name
    value: revenue               # Required — the value column to forecast
    horizon: 6                   # Optional — periods ahead (default: 3)
    confidence_level: 0.95       # Optional — interval width (default: 0.95)
    model: auto                  # Optional — auto|ets|linear|exponential|logistic (default: auto)
    group_by:                    # Optional — for per-group forecasts
      - region
```

**Output columns** (stable names — use these in `visualize:`):

| Column | Type | Description |
|--------|------|-------------|
| `forecast` | DOUBLE | Point forecast value (NULL for historical rows) |
| `lower_bound` | DOUBLE | Lower confidence interval (NULL for historical rows) |
| `upper_bound` | DOUBLE | Upper confidence interval (NULL for historical rows) |
| `is_forecast` | BOOLEAN | TRUE for forecast rows, FALSE for historical |

The original `timestamp` and `value` columns are preserved. Historical rows have the original values with NULL forecast columns. Forecast rows have the forecast values with NULL for the original value column.

**Anchor row:** The forecast prepends the last historical data point so the forecast line connects to the actuals line with no visual gap.

**Models:**
- `auto` (default, recommended) — selects best model via cross-validation
- `ets` — exponential smoothing (good for seasonal data)
- `linear` — linear regression
- `exponential` — exponential growth curves (requires all values > 0)
- `logistic` — S-curve / saturation patterns

**Standard forecast visualization pattern:**

```yaml
visualize:
  type: line
  columns: date
  rows:
    - field: revenue
      label: "Actual Revenue"
    - field: forecast
      lineStyle: dashed
      label: "Forecast"
    - mark: range
      upper: upper_bound
      lower: lower_bound
      label: "Confidence Interval"
```

### When to Use Aggregate vs SQL

| Scenario | Stage to use |
|----------|-------------|
| Single source, standard GROUP BY | `transform.aggregate` — always prefer |
| Multi-source joins | `transform.sql` |
| Window functions, CTEs, CASE WHEN | `transform.sql` |
| Join then aggregate | `transform.sql` + `transform.aggregate` |
| Simple forecast on time series | `transform.forecast` |
| Aggregate then forecast | `transform.aggregate` + `transform.forecast` |
| Full pipeline (join, aggregate, forecast) | `transform.sql` + `transform.aggregate` + `transform.forecast` |

### Complete Transform Examples

#### Simple Aggregation

```chartml
data:
  sales:
    datasource: "production-postgres"
    query: "SELECT region, revenue FROM sales.transactions"
transform:
  aggregate:
    dimensions: [region]
    measures:
      - column: revenue
        aggregation: sum
        name: total_revenue
    sort:
      - field: total_revenue
        direction: desc
visualize:
  type: bar
  columns: region
  rows: total_revenue
```

#### SQL-Only

```chartml
data:
  sales:
    datasource: "production-postgres"
    query: "SELECT region, revenue, cost FROM sales.transactions WHERE sale_date >= '2025-01-01'"
transform:
  sql: |
    SELECT
      region,
      SUM(revenue) as total_revenue,
      SUM(cost) as total_cost,
      SUM(revenue) - SUM(cost) as profit,
      (SUM(revenue) - SUM(cost)) / SUM(revenue) as profit_margin
    FROM {sales}
    GROUP BY region
    HAVING profit_margin >= 0.30
    ORDER BY profit_margin DESC
visualize:
  type: bar
  columns: region
  rows:
    field: profit_margin
    format: ".1%"
  axes:
    rows:
      label: "Profit Margin"
      format: ".1%"
```

#### Multi-Source Join

```chartml
data:
  orders:
    datasource: "production-postgres"
    query: "SELECT * FROM orders"
  customers:
    datasource: "production-postgres"
    query: "SELECT * FROM customers"
transform:
  sql: >
    SELECT o.order_date, o.revenue, c.region
    FROM {orders} o
    JOIN {customers} c ON o.customer_id = c.id
  aggregate:
    dimensions:
      - column: order_date
        type: month
        name: month
      - region
    measures:
      - column: revenue
        aggregation: sum
        name: total_revenue
    sort:
      - field: month
        direction: asc
visualize:
  type: line
  columns: month
  rows: total_revenue
```

#### Aggregation + Forecast

```chartml
data:
  sales:
    datasource: "production-postgres"
    query: "SELECT sale_date, revenue FROM sales.transactions"
transform:
  aggregate:
    dimensions:
      - column: sale_date
        type: month
        name: month
    measures:
      - column: revenue
        aggregation: sum
        name: revenue
    sort:
      - field: month
        direction: asc
  forecast:
    timestamp: month
    value: revenue
    horizon: 6
visualize:
  type: line
  columns: month
  rows:
    - field: revenue
      label: "Actual Revenue"
    - field: forecast
      lineStyle: dashed
      label: "Forecast"
    - mark: range
      upper: upper_bound
      lower: lower_bound
      label: "95% Confidence"
```

#### Forecast Only (Most Common)

When your query already returns a time series at the right granularity, just add the forecast stage — no aggregate needed:

```chartml
data:
  revenue:
    datasource: "production-postgres"
    query: |
      SELECT sale_date::date as date, SUM(revenue) as revenue
      FROM sales.transactions
      WHERE sale_date >= '2024-01-01'
      GROUP BY 1 ORDER BY 1
transform:
  forecast:
    timestamp: date
    value: revenue
    horizon: 6
visualize:
  type: line
  columns: date
  rows:
    - field: revenue
      label: "Actual Revenue"
    - field: forecast
      lineStyle: dashed
      label: "Forecast"
    - mark: range
      upper: upper_bound
      lower: lower_bound
      label: "95% Confidence"
```

---

## QuackStats: Statistical Functions for DuckDB

**For standard forecasting, prefer the declarative `transform.forecast` stage (see above).** Use the QuackStats SQL functions below for advanced workflows only — custom UNION logic, combining multiple forecast outputs, integrating with complex SQL pipelines, etc.

QuackStats is a DuckDB extension loaded automatically in ChartML's transform pipeline. It provides time series forecasting and seasonality detection as SQL table functions.

### `forecast()` Table Function

Generates time series forecasts with prediction intervals.

**Syntax:**
```sql
SELECT * FROM forecast(
  'table_name',
  timestamp = 'date_column',
  value = 'value_column',
  group_by = ['col1', 'col2'],
  horizon = 6,
  confidence_level = 0.95,
  model = 'auto'
)
```

**Parameters:**

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `table_name` | VARCHAR (positional) | *required* | DuckDB table or CTE to forecast |
| `timestamp` | VARCHAR | `"timestamp"` | Name of the DATE column |
| `value` | VARCHAR | `"value"` | Name of the numeric column to forecast |
| `group_by` | VARCHAR[] | *none* | Produce independent forecasts per group |
| `horizon` | INTEGER | `3` | Number of future steps to predict |
| `confidence_level` | DOUBLE | `0.95` | Width of prediction interval (0.0–1.0) |
| `model` | VARCHAR | `"auto"` | `"auto"`, `"ets"`, `"linear"`, `"exponential"`, or `"logistic"` |

**Output columns:**

| Column | Type | Description |
|--------|------|-------------|
| `forecast_timestamp` | DATE | Future date for this prediction |
| `forecast` | DOUBLE | Point forecast value |
| `lower_bound` | DOUBLE | Lower prediction interval bound |
| `upper_bound` | DOUBLE | Upper prediction interval bound |

**Key behaviors:**
- **Anchor row**: The first output row is the last historical observation (forecast = actual value, bounds = actual value). This ensures the forecast line connects seamlessly to the actuals line with no visual gap.
- **Model `auto`**: Uses cross-validation to select the best model. Holds out 20% of data, evaluates all candidate models (seasonal ETS, ETS, exponential, logistic, linear), and selects the one with lowest MSE. Falls back to heuristic selection if insufficient data for CV.
- **Model `exponential`**: Fits y = a * exp(b * x) via log-transform + OLS. Best for data with consistent percentage growth. Requires all values > 0.
- **Model `logistic`**: Fits y = L / (1 + exp(-k * (x - x0))) via Levenberg-Marquardt. Best for S-curve / saturation patterns where growth approaches a capacity limit.
- **Minimum data**: 4 data points per series (groups with fewer are silently skipped).
- **`group_by`**: When provided, each group is forecast independently. Group columns appear as leading output columns.

### `detect_seasonality()` Table Function

Detects repeating periodic patterns in time series data.

**Syntax:**
```sql
SELECT * FROM detect_seasonality(
  'table_name',
  timestamp = 'date_column',
  value = 'value_column',
  group_by = ['col1']
)
```

**Parameters:** Same as `forecast()` except no `horizon`, `confidence_level`, or `model` parameters.

**Output columns:**

| Column | Type | Description |
|--------|------|-------------|
| `period` | INTEGER | Detected period length (e.g., 7 for weekly in daily data) |
| `strength` | DOUBLE | Strength of the seasonal component (0.0–1.0) |

**Key behaviors:**
- Returns multiple rows if multiple periods are detected, sorted by descending strength.
- Returns no rows if no seasonality is found.
- **Minimum data**: 8 data points per series.
- Strength > 0.3 generally indicates meaningful seasonality. Strength > 0.7 is strong.

### Usage in ChartML — Forecast Example

#### Declarative Approach (Preferred)

Use `transform.forecast` for standard forecasting. The pipeline handles all the SQL generation automatically:

```chartml
type: chart
version: 1
title: "Revenue: Actuals vs Forecast"
data:
  revenue:
    datasource: "production-postgres"
    query: |
      SELECT sale_date::date as date, SUM(revenue) as revenue
      FROM sales.transactions
      WHERE sale_date >= '2024-01-01'
      GROUP BY 1 ORDER BY 1
transform:
  forecast:
    timestamp: date
    value: revenue
    horizon: 6
visualize:
  type: line
  columns: date
  rows:
    - field: revenue
      label: "Actual Revenue"
    - field: forecast
      lineStyle: dashed
      label: "Forecast"
    - mark: range
      upper: upper_bound
      lower: lower_bound
      label: "95% Confidence"
```

#### Manual SQL Approach (Advanced)

Use `transform.sql` with the `forecast()` function directly when you need custom UNION logic, multiple forecast outputs, or other advanced workflows:

```chartml
type: chart
version: 1
title: "Revenue: Actuals vs Forecast"

data:
  revenue:
    datasource: "production-postgres"
    query: |
      SELECT sale_date::date as date, SUM(revenue) as revenue
      FROM sales.transactions
      WHERE sale_date >= '2024-01-01'
      GROUP BY 1 ORDER BY 1

transform:
  sql:
    - |
      CREATE OR REPLACE TABLE actuals AS
      SELECT date, revenue FROM {revenue}
    - |
      CREATE OR REPLACE TABLE predictions AS
      SELECT * FROM forecast(
        'actuals',
        timestamp = 'date',
        value = 'revenue',
        horizon = 6,
        confidence_level = 0.95
      )
    - |
      SELECT
        date as month,
        revenue as actual,
        NULL as forecast,
        NULL as lower_95,
        NULL as upper_95
      FROM actuals
      UNION ALL
      SELECT
        forecast_timestamp as month,
        NULL as actual,
        forecast,
        lower_bound as lower_95,
        upper_bound as upper_95
      FROM predictions
      ORDER BY month

visualize:
  type: line
  columns: month
  rows:
    - field: actual
      label: "Actual Revenue"
      color: "#4285f4"
    - field: forecast
      label: "Forecast"
      lineStyle: dashed
      color: "#34a853"
    - mark: range
      upper: upper_95
      lower: lower_95
      label: "95% Confidence"
      color: "#34a853"
      opacity: 0.15
  axes:
    rows:
      label: "Revenue ($)"
      format: "$,.0f"
```

### Usage in ChartML — Seasonality Detection Example

```chartml
type: chart
version: 1
title: "Detected Seasonal Patterns"

data:
  traffic:
    datasource: "production-postgres"
    query: |
      SELECT visit_date::date as date, COUNT(*) as visits
      FROM analytics.page_views
      WHERE visit_date >= '2024-01-01'
      GROUP BY 1 ORDER BY 1

transform:
  sql:
    - |
      CREATE OR REPLACE TABLE daily_traffic AS
      SELECT date, visits FROM {traffic}
    - |
      SELECT * FROM detect_seasonality(
        'daily_traffic',
        timestamp = 'date',
        value = 'visits'
      )

visualize:
  type: table
  columns:
    - field: period
      label: "Period (days)"
    - field: strength
      label: "Strength"
      format: ".2f"
```

---

## Number and Date Formatting

ChartML uses [d3-format](https://d3js.org/d3-format) for numbers and [d3-time-format](https://d3js.org/d3-time-format) for dates.

### Common Formats

| Format | Example Output | Description |
|--------|---------------|-------------|
| `$,.0f` | $1,234 | Currency, thousands separator, no decimals |
| `$,.2f` | $1,234.56 | Currency with 2 decimals |
| `,.0f` | 1,234 | Thousands separator, no decimals |
| `.1%` | 12.3% | Percentage with 1 decimal |
| `.0%` | 12% | Percentage, no decimals |
| `~s` | 1.2K, 3.4M, 5.6B | SI-prefix (K, M, B, T) |
| `.2e` | 1.23e+4 | Scientific notation |
| `+,.0f` | +1,234 | Always show sign |

### Where to Use Formats

```chartml
visualize:
  type: bar
  rows:
    field: revenue
    dataLabels:
      format: "$,.0f"                   # Format data labels
  axes:
    rows:
      format: "$,.0f"                   # Format axis ticks
```

---

## Annotations (Reference Lines & Bands)

### Reference Line (horizontal goal)

```chartml
visualize:
  type: bar
  columns: month
  rows: revenue
  annotations:
    - type: line
      axis: left                        # left | right | x
      value: 150000
      orientation: horizontal
      label: "Monthly Goal"
      labelPosition: end                # start | center | end
      color: "#34a853"
      strokeWidth: 2
      dashArray: "5,5"                  # Dashed line
      opacity: 1.0
```

### Reference Band (target range)

```chartml
visualize:
  type: line
  columns: month
  rows: revenue
  annotations:
    - type: band
      axis: left
      from: 140000                      # Start of range
      to: 160000                        # End of range
      orientation: horizontal
      label: "Target Range"
      color: "#34a853"
      opacity: 0.15
      strokeColor: "#34a853"
      strokeWidth: 1
```

### Event Marker (vertical line)

```chartml
visualize:
  type: line
  columns: date
  rows: sales
  annotations:
    - type: line
      axis: x
      value: "2025-03-15"               # Date value for vertical line
      orientation: vertical
      label: "Product Launch"
      labelPosition: start
      color: "#4285f4"
      strokeWidth: 2
```

---

## Grid Layout

Charts use a responsive 12-column grid system:

```chartml
# Create a 2x2 grid of charts
- type: chart
  version: 1
  title: "Chart 1"
  layout:
    colSpan: 6                          # Half width
  data:
    provider: bigquery
    query: |
      SELECT region, SUM(revenue) as revenue
      FROM `project.dataset.sales`
      GROUP BY region
    cache:
      ttl: 6h
  visualize:
    type: bar
    columns: region
    rows: revenue

- type: chart
  version: 1
  title: "Chart 2"
  layout:
    colSpan: 6                          # Half width
  data:
    provider: bigquery
    query: |
      SELECT
        FORMAT_DATE('%b', sale_date) as month,
        SUM(revenue) as sales
      FROM `project.dataset.sales`
      WHERE sale_date >= DATE_SUB(CURRENT_DATE(), INTERVAL 6 MONTH)
      GROUP BY month, EXTRACT(MONTH FROM sale_date)
      ORDER BY EXTRACT(MONTH FROM sale_date)
    cache:
      ttl: 6h
  visualize:
    type: line
    columns: month
    rows: sales
```

**Column spans**: 1-12 (default: 12 = full width)
- `colSpan: 12` = Full width
- `colSpan: 6` = Half width
- `colSpan: 4` = Third width
- `colSpan: 3` = Quarter width

### ⚠️ CRITICAL: Side-by-Side Charts MUST Be in the SAME Block

**To display charts side-by-side, they MUST be in the SAME `chartml` code block using YAML array syntax (starting with `-`).**

```chartml
# ✅ CORRECT - Charts side-by-side (same block, array syntax)
- type: chart
  version: 1
  title: "Chart A"
  layout:
    colSpan: 6
  data:
    provider: bigquery
    query: "SELECT region, SUM(revenue) as revenue FROM `project.dataset.sales` GROUP BY region"
  visualize:
    type: bar
    columns: region
    rows: revenue

- type: chart
  version: 1
  title: "Chart B"
  layout:
    colSpan: 6
  data:
    provider: bigquery
    query: "SELECT category, SUM(units) as units FROM `project.dataset.sales` GROUP BY category"
  visualize:
    type: bar
    columns: category
    rows: units
```

**WRONG - Separate blocks = charts stack vertically (NOT side-by-side):**

```chartml
# ❌ WRONG - This chart will be on its own row
type: chart
version: 1
title: "Chart A"
layout:
  colSpan: 6
...
```

```chartml
# ❌ WRONG - This chart will be on a SEPARATE row, not next to Chart A
type: chart
version: 1
title: "Chart B"
layout:
  colSpan: 6
...
```

**Rule of thumb:**
- **Same row** → Put all charts in ONE `chartml` block using YAML array syntax (`-`)
- **Different rows** → Use separate `chartml` blocks

---

## Reusable Styles (Optional)

Define reusable themes for consistent styling:

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

Reference in charts:

```chartml
type: chart
version: 1
style: corporate_theme                  # Reference by name
data:
  provider: bigquery
  query: "SELECT region, SUM(revenue) as revenue FROM `project.dataset.sales` GROUP BY region"
visualize:
  type: bar
  columns: region
  rows: revenue
```

Or override specific properties (deep merge):

```chartml
type: chart
version: 1
style: corporate_theme
data:
  provider: bigquery
  query: "SELECT region, SUM(revenue) as revenue FROM `project.dataset.sales` GROUP BY region"
visualize:
  type: bar
  columns: region
  rows: revenue
  style:
    height: 600                         # Override just height
    grid:
      color: "#ff0000"                  # Override just grid color
    # Other properties still from corporate_theme
```

---

## Common Patterns

### Pattern 1: Top N Products

```chartml
type: chart
version: 1
title: "Top 10 Products by Revenue"
data:
  provider: bigquery
  query: |
    SELECT
      product_name as product,
      SUM(revenue) as revenue
    FROM `project.dataset.sales`
    WHERE sale_date >= DATE_SUB(CURRENT_DATE(), INTERVAL 90 DAY)
    GROUP BY product_name
    ORDER BY revenue DESC
    LIMIT 10
  cache:
    ttl: 6h
visualize:
  type: bar
  orientation: horizontal               # Good for rankings
  columns: product
  rows: revenue
  axes:
    rows:
      format: "$,.0f"
```

### Pattern 2: Metric Cards Row (4 KPIs)

```chartml
- type: chart
  version: 1
  title: "Total Revenue"
  layout:
    colSpan: 3
  data:
    provider: bigquery
    query: |
      SELECT
        SUM(CASE WHEN sale_date >= DATE_SUB(CURRENT_DATE(), INTERVAL 30 DAY)
          THEN revenue ELSE 0 END) as current,
        SUM(CASE WHEN sale_date >= DATE_SUB(CURRENT_DATE(), INTERVAL 60 DAY)
          AND sale_date < DATE_SUB(CURRENT_DATE(), INTERVAL 30 DAY)
          THEN revenue ELSE 0 END) as previous
      FROM `project.dataset.sales`
    cache:
      ttl: 1h
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
    provider: bigquery
    query: |
      SELECT
        COUNT(DISTINCT CASE WHEN activity_date >= DATE_SUB(CURRENT_DATE(), INTERVAL 30 DAY)
          THEN user_id END) as current,
        COUNT(DISTINCT CASE WHEN activity_date >= DATE_SUB(CURRENT_DATE(), INTERVAL 60 DAY)
          AND activity_date < DATE_SUB(CURRENT_DATE(), INTERVAL 30 DAY)
          THEN user_id END) as previous
      FROM `project.dataset.user_activity`
    cache:
      ttl: 1h
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
    provider: bigquery
    query: |
      SELECT
        SAFE_DIVIDE(
          COUNT(DISTINCT CASE WHEN converted AND visit_date >= DATE_SUB(CURRENT_DATE(), INTERVAL 30 DAY)
            THEN visitor_id END),
          COUNT(DISTINCT CASE WHEN visit_date >= DATE_SUB(CURRENT_DATE(), INTERVAL 30 DAY)
            THEN visitor_id END)
        ) as current,
        SAFE_DIVIDE(
          COUNT(DISTINCT CASE WHEN converted AND visit_date >= DATE_SUB(CURRENT_DATE(), INTERVAL 60 DAY)
            AND visit_date < DATE_SUB(CURRENT_DATE(), INTERVAL 30 DAY)
            THEN visitor_id END),
          COUNT(DISTINCT CASE WHEN visit_date >= DATE_SUB(CURRENT_DATE(), INTERVAL 60 DAY)
            AND visit_date < DATE_SUB(CURRENT_DATE(), INTERVAL 30 DAY)
            THEN visitor_id END)
        ) as previous
      FROM `project.dataset.visits`
    cache:
      ttl: 1h
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
    provider: bigquery
    query: |
      SELECT
        AVG(CASE WHEN order_date >= DATE_SUB(CURRENT_DATE(), INTERVAL 30 DAY)
          THEN order_value END) as current,
        AVG(CASE WHEN order_date >= DATE_SUB(CURRENT_DATE(), INTERVAL 60 DAY)
          AND order_date < DATE_SUB(CURRENT_DATE(), INTERVAL 30 DAY)
          THEN order_value END) as previous
      FROM `project.dataset.orders`
    cache:
      ttl: 1h
  visualize:
    type: metric
    value: current
    format: "$,.2f"
    compareWith: previous
```

### Pattern 3: Grouped Bar Chart with Data Labels

```chartml
type: chart
version: 1
title: "Regional Revenue by Product"
data:
  provider: bigquery
  query: |
    SELECT
      region,
      SUM(CASE WHEN product = 'Widget A' THEN revenue ELSE 0 END) as widget_a,
      SUM(CASE WHEN product = 'Widget B' THEN revenue ELSE 0 END) as widget_b
    FROM `project.dataset.sales`
    WHERE sale_date >= DATE_SUB(CURRENT_DATE(), INTERVAL 30 DAY)
    GROUP BY region
  cache:
    ttl: 6h
visualize:
  type: bar
  mode: grouped
  columns: region
  rows:
    - field: widget_a
      label: "Widget A"
      dataLabels:
        show: true
        format: "$,.0f"
    - field: widget_b
      label: "Widget B"
      dataLabels:
        show: true
        format: "$,.0f"
  axes:
    rows:
      format: "$,.0f"
  style:
    height: 400
```

### Pattern 4: Stacked Area with Normalized Mode

```chartml
type: chart
version: 1
title: "Regional Market Share Over Time"
data:
  provider: bigquery
  query: |
    SELECT
      FORMAT_DATE('%Y-W%V', sale_date) as week,
      SUM(CASE WHEN region = 'North' THEN revenue ELSE 0 END) as north,
      SUM(CASE WHEN region = 'South' THEN revenue ELSE 0 END) as south,
      SUM(CASE WHEN region = 'East' THEN revenue ELSE 0 END) as east
    FROM `project.dataset.sales`
    WHERE sale_date >= DATE_SUB(CURRENT_DATE(), INTERVAL 8 WEEK)
    GROUP BY week, sale_date
    ORDER BY sale_date
  cache:
    ttl: 12h
visualize:
  type: area
  mode: normalized                      # Shows as 100% stacked
  columns: week
  rows:
    - field: north
      label: "North"
    - field: south
      label: "South"
    - field: east
      label: "East"
  axes:
    rows:
      label: "Market Share"
      format: ".0%"
  style:
    height: 300
```

---

## lineStyle (Line Dash Patterns)

Use `lineStyle` on rows objects to control line dash patterns. Values: `solid` (default), `dashed`, `dotted`.

```chartml
visualize:
  type: line
  columns: month
  rows:
    - field: actual
      label: "Actual"
      lineStyle: solid
    - field: forecast
      label: "Forecast"
      lineStyle: dashed
      color: "#888888"
    - field: budget
      label: "Budget"
      lineStyle: dotted
      color: "#cc0000"
```

Only applies to `mark: line` (or chart `type: line`). Ignored for bar, area, and dot marks.

---

## mark: range (Confidence Intervals & Bands)

The `range` mark renders a shaded area between upper and lower bounds. Uses `upper`/`lower` instead of `field`:

```chartml
visualize:
  type: line
  columns: date
  rows:
    - field: forecast
      label: "Forecast"
      mark: line
      lineStyle: dashed
      color: "#4285f4"
    - mark: range
      upper: upper_95
      lower: lower_95
      label: "95% CI"
      color: "#4285f4"
      opacity: 0.15
```

**Range mark properties:** `mark: range` (required), `upper` (required), `lower` (required), `label`, `color`, `opacity` (0-1, default: 0.15), `axis` (left|right).

---

## End-to-End: Forecast with Confidence Intervals

**Preferred approach:** Use `transform.forecast` to generate predictions declaratively. The pipeline handles all the intermediate SQL. See the **Transform Pipeline > Stage 3: Forecast** section above for the schema and visualization pattern.

**Advanced approach:** Use the `forecast()` QuackStats function in `transform.sql` for custom workflows. See the **QuackStats > Manual SQL Approach** section above for the full example with multi-statement SQL, UNION ALL, and confidence intervals.

---

## `forecast_data` Agent Tool

The `forecast_data` tool runs time series forecasting on the backend and returns forecast **numbers** the agent can reason about and write in prose. This is different from the ChartML `transform.forecast` stage, which is visual. Calling `forecast_data` does NOT add a forecast to any chart — if you want a visual forecast line on a chart, you must include `transform.forecast` in the ChartML spec.

### When to Use

**Default to `transform.forecast` in a chart whenever the user wants to see a forecast.** Only use the `forecast_data` tool when the user needs forecast numbers in prose without a visualization, or when the watch agent needs to make decisions based on forecast values.

- User asks a **question** about future values ("What will revenue be next quarter?") → `forecast_data` tool (returns numbers for prose)
- User wants to **see** a forecast ("predict visitors for the next 7 days", "show me a forecast") → `transform.forecast` in a ChartML chart
- Watch agent needs forecast-based alerting ("Alert me if projected revenue drops below $100k") → `forecast_data` tool

### Parameters

| Parameter | Type | Required | Default | Description |
|-----------|------|----------|---------|-------------|
| `datasource` | string | Yes | — | Datasource slug |
| `query` | string | Yes | — | SQL query returning timestamp + value columns |
| `timestamp` | string | Yes | — | Name of the timestamp column |
| `value` | string | Yes | — | Name of the value column to forecast |
| `horizon` | integer | No | 3 | Number of periods to forecast ahead |
| `confidence_level` | number | No | 0.95 | Confidence interval width (0.0-1.0) |
| `model` | string | No | "auto" | `auto`, `ets`, `linear`, `exponential`, `logistic` |
| `group_by` | string[] | No | — | Columns for per-group forecasts |

### Return Format

The tool returns a structured result with forecast numbers:

```json
{
    "model_used": "ets",
    "data_points": 24,
    "forecast": [
        {"timestamp": "2025-07-01", "forecast": 142000, "lower_bound": 128000, "upper_bound": 156000},
        {"timestamp": "2025-08-01", "forecast": 148000, "lower_bound": 130000, "upper_bound": 166000}
    ],
    "summary": "Forecasted 6 periods using ETS model with 95% confidence intervals"
}
```

### Relationship to ChartML Forecast

Same statistical concepts, different purpose:

- **`forecast_data` tool** — returns numbers for prose and analysis. Use when the agent needs to state projections in text or make decisions based on forecast values.
- **`transform.forecast`** — visual forecast on a chart. Use when the user wants to see a forecast chart.
- **Both together** — use `forecast_data` to get numbers for the analysis text, and `transform.forecast` in a ChartML chart for the visualization. Common pattern for a complete response.

---

## When to Use What

| User intent | What to use |
|-------------|-------------|
| "Show me revenue by month" | `transform.aggregate` |
| "Predict visitors for the next 7 days" | ChartML with `transform.forecast` — **always show forecasts in a chart** |
| "Forecast revenue for the next 6 months" | ChartML with `transform.forecast` (query returns time series → just forecast) |
| "Show me revenue by month with forecast" | ChartML with `transform.aggregate` + `transform.forecast` (raw data needs rollup first) |
| "What will revenue be next quarter?" | `forecast_data` tool (user wants a **number in prose**, not a chart) |
| "Show me a chart of projected revenue" | ChartML with `transform.forecast` |
| "Alert me if projected revenue drops below $100k" | Watch with `forecast_data` tool |
| "Join orders with customers by region" | `transform.sql` + `transform.aggregate` |
| "Running average of daily sales" | `transform.sql` (window functions) |

---

## Quick Tips

1. **Always use YAML syntax**, never JSON
2. **columns = categories, rows = values** - don't mix them up
3. **Cache queries** with `cache.ttl` to improve performance (6h or 24h recommended)
4. **Both single and named sources work with transform** - Single source: `data: { datasource: ..., query: ... }` is auto-normalized. Named sources required only for multi-source joins or `{name}` placeholders
5. **`cache:` goes INSIDE named sources** - Not as a sibling: `data: { my_source: { datasource: ..., query: ..., cache: { ttl: 6h } } }`
6. **Transform pipeline** runs stages in order: `sql → aggregate → forecast`. Use `transform.aggregate` for standard aggregation, `transform.sql` for joins/CTEs/window functions, `transform.forecast` for time series predictions
7. **Format numbers** with d3-format: `$,.0f`, `.1%`, `~s`
8. **Grid layout** with `colSpan` for multi-chart dashboards
9. **Data labels** make charts more readable: `dataLabels.show: true`
10. **Reference lines** highlight goals: `annotations.type: line`
11. **Dual-axis charts** for different scales: `rows[].axis: left|right`
12. **lineStyle** for visual distinction: `solid`, `dashed`, `dotted` (line marks only)
13. **mark: range** for confidence intervals: `upper`/`lower` bounds with shaded fill
14. **Use relative date filters** like `DATE_SUB(CURRENT_DATE(), INTERVAL 30 DAY)` for dynamic queries
15. **Forecasting**: Use `transform.forecast` for visual chart forecasts (preferred). Use QuackStats `forecast()` in `transform.sql` for advanced SQL workflows. Use the `forecast_data` agent tool to get forecast numbers for prose analysis
16. **`forecast_data` tool** for projections in text: when the user asks "what will revenue be next quarter?", use the tool to get numbers, then write the analysis in prose

---

## Complete Dashboard Example

```markdown
# Q1 Sales Dashboard

## Overview Metrics

```chartml
- type: chart
  version: 1
  title: "Total Revenue"
  layout:
    colSpan: 3
  data:
    provider: bigquery
    query: |
      SELECT
        SUM(CASE WHEN sale_date >= DATE_SUB(CURRENT_DATE(), INTERVAL 90 DAY)
          THEN revenue ELSE 0 END) as current,
        SUM(CASE WHEN sale_date >= DATE_SUB(CURRENT_DATE(), INTERVAL 180 DAY)
          AND sale_date < DATE_SUB(CURRENT_DATE(), INTERVAL 90 DAY)
          THEN revenue ELSE 0 END) as previous
      FROM `project.dataset.sales`
    cache:
      ttl: 6h
  visualize:
    type: metric
    value: current
    format: "$,.0f"
    compareWith: previous

- type: chart
  version: 1
  title: "Customers"
  layout:
    colSpan: 3
  data:
    provider: bigquery
    query: |
      SELECT
        COUNT(DISTINCT CASE WHEN sale_date >= DATE_SUB(CURRENT_DATE(), INTERVAL 90 DAY)
          THEN customer_id END) as current,
        COUNT(DISTINCT CASE WHEN sale_date >= DATE_SUB(CURRENT_DATE(), INTERVAL 180 DAY)
          AND sale_date < DATE_SUB(CURRENT_DATE(), INTERVAL 90 DAY)
          THEN customer_id END) as previous
      FROM `project.dataset.sales`
    cache:
      ttl: 6h
  visualize:
    type: metric
    value: current
    format: ",.0f"
    compareWith: previous
```

## Revenue Analysis

```chartml
type: chart
version: 1
title: "Monthly Revenue Trend with Goal"
data:
  provider: bigquery
  query: |
    SELECT
      FORMAT_DATE('%b', sale_date) as month,
      SUM(revenue) as revenue
    FROM `project.dataset.sales`
    WHERE sale_date >= DATE_SUB(CURRENT_DATE(), INTERVAL 3 MONTH)
    GROUP BY month, EXTRACT(MONTH FROM sale_date)
    ORDER BY EXTRACT(MONTH FROM sale_date)
  cache:
    ttl: 6h
visualize:
  type: bar
  columns: month
  rows:
    field: revenue
    dataLabels:
      show: true
      format: "$,.0f"
  axes:
    rows:
      label: "Revenue ($)"
      format: "$,.0f"
  annotations:
    - type: line
      axis: left
      value: 150000
      label: "Goal"
      color: "#34a853"
      dashArray: "5,5"
  style:
    height: 400
```

```chartml
type: chart
version: 1
title: "Revenue by Region"
layout:
  colSpan: 6
data:
  provider: bigquery
  query: |
    SELECT
      region,
      SUM(revenue) as revenue
    FROM `project.dataset.sales`
    WHERE sale_date >= DATE_SUB(CURRENT_DATE(), INTERVAL 90 DAY)
    GROUP BY region
  cache:
    ttl: 24h
visualize:
  type: pie
  columns: region
  rows: revenue
  style:
    height: 350
```
```

---

**End of Quick Reference**
