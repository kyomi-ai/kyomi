---
layout: doc
title: "ChartML: Transform Pipeline"
description: "ChartML transform pipeline — SQL preprocessing, declarative aggregation, time series forecasting, and QuackStats functions."
---

# Transform Pipeline

The `transform:` block processes data through up to three composable stages before visualization. Stages run in fixed order: **sql → aggregate → forecast**. Each stage operates on the output of the previous stage (or the source data if first).

```yaml
transform:
  sql: ...           # Stage 1: SQL preprocessing (joins, CTEs, window functions)
  aggregate: ...     # Stage 2: Declarative aggregation (dimensions, measures, filters)
  forecast: ...      # Stage 3: Time series forecasting
```

**Rules:**
- At least one stage must be present when using `transform:`
- Only these three stages are allowed
- All SQL runs in DuckDB, regardless of the upstream data source
- Unnamed sources (datasource + query without a name key) **cannot** use transform
- Multiple named sources **require** `transform.sql` to join or combine them

---

## Stage 1: SQL (`transform.sql`)

SQL preprocessing using DuckDB syntax. Use for joins, CTEs, window functions, CASE WHEN, and any transformation expressible in SQL.

**Type:** String (single statement) or array of strings (multi-statement).

### Single-Source SQL

```yaml
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

Use `{name}` placeholders matching keys in `data:` to reference source tables.

### Multi-Source Join

```yaml
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

### Multi-Statement SQL

Use an array for multi-statement SQL. The last statement's result is used:

```yaml
transform:
  sql:
    - |
      CREATE OR REPLACE TABLE monthly AS
      SELECT DATE_TRUNC('month', sale_date) as month, SUM(revenue) as revenue
      FROM {sales} GROUP BY 1
    - |
      SELECT month, revenue, LAG(revenue) OVER (ORDER BY month) as prev_revenue
      FROM monthly ORDER BY month
```

### Parameter References in SQL

```yaml
transform:
  sql: |
    SELECT region, SUM(revenue) as total_revenue
    FROM {sales}
    WHERE region IN ($dashboard_filters.selected_regions)
      AND sale_date BETWEEN '$time_filter.date_range.start'
        AND '$time_filter.date_range.end'
    GROUP BY region
    HAVING total_revenue >= $revenue_filter.minimum_revenue
    ORDER BY total_revenue DESC
    LIMIT $top_n
```

---

## Stage 2: Declarative Aggregate (`transform.aggregate`)

Declarative aggregation that compiles to DuckDB SQL. Prefer this over the SQL stage for standard group-by aggregation on a single source.

```yaml
transform:
  aggregate:
    dimensions:       # Optional - grouping columns
      - region
      - column: order_date
        name: month
        type: month     # Temporal: day, week, month, quarter, year
    measures:         # Required - aggregation expressions
      - column: revenue
        aggregation: sum
        name: total_revenue
      - column: revenue
        aggregation: avg
        name: avg_revenue
      - expression: "total_revenue / order_count"
        name: avg_order_value
    filters:          # Optional - row filtering
      combinator: and
      rules:
        - field: region
          operator: in
          value: ["US", "EU"]
        - field: total_revenue
          operator: ">="
          value: 10000
    sort:             # Optional - result ordering
      - field: total_revenue
        direction: desc
    limit: 50         # Optional - max rows
```

### Dimensions

Group-by columns. String shorthand or object form:

```yaml
dimensions:
  - region                    # String shorthand
  - column: order_date        # Object form
    name: month               # Rename output
    type: month               # Temporal truncation
```

Temporal types: `day`, `week`, `month`, `quarter`, `year`.

### Measures

**16 aggregation functions:** `sum`, `count`, `avg`, `min`, `max`, `countDistinct`, `median`, `stddev`, `variance`, `percentile25`, `percentile50`, `percentile75`, `percentile90`, `percentile95`, `percentile99`

Column aggregation:
```yaml
- column: revenue
  aggregation: sum
  name: total_revenue
```

Expression (calculated from other measures):
```yaml
- expression: "total_revenue / order_count"
  name: avg_order_value
```

### Filters

**14 operators:** `=`, `!=`, `>`, `>=`, `<`, `<=`, `in`, `notIn`, `contains`, `startsWith`, `endsWith`, `between`, `isNull`, `isNotNull`

Filters are automatically partitioned:
- Dimension fields → `WHERE` clause (pre-aggregation)
- Measure fields → `HAVING` clause (post-aggregation)

```yaml
filters:
  combinator: and       # and | or
  rules:
    - field: region
      operator: "!="
      value: "Test"
    - field: total_revenue
      operator: ">="
      value: 10000
```

---

## Stage 3: Forecast (`transform.forecast`)

Time series forecasting that extends historical data with predicted values and confidence intervals.

```yaml
transform:
  forecast:
    timestamp: date_column       # Required - date/timestamp column
    value: numeric_column        # Required - numeric value to forecast
    horizon: 6                   # Periods to forecast (default: 3)
    confidence_level: 0.95       # Interval width 0-1 (default: 0.95)
    model: auto                  # auto | ets | linear | exponential | logistic
    group_by:                    # Optional - per-group forecasts
      - region
```

### Output Columns

| Column | Type | Description |
|--------|------|-------------|
| `forecast` | DOUBLE | Point forecast (NULL for historical rows) |
| `lower_bound` | DOUBLE | Lower confidence bound (NULL for historical) |
| `upper_bound` | DOUBLE | Upper confidence bound (NULL for historical) |
| `is_forecast` | BOOLEAN | TRUE for forecast rows, FALSE for historical |

The original `timestamp` and `value` columns are preserved. The first output row is the last historical observation (anchor row) so the forecast line connects to the actuals line.

### Models

- **`auto`** (default, recommended) — cross-validates all models, picks best fit
- **`ets`** — exponential smoothing (good for seasonal data)
- **`linear`** — straight line projection
- **`exponential`** — fits y = a * exp(b * x), requires all values > 0
- **`logistic`** — S-curve / saturation patterns

Minimum 4 data points required per series.

### Forecast Visualization Pattern

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
      label: "95% Confidence"
      opacity: 0.15
```

### Forecast with group_by

```yaml
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

## Combined Stages

Stages can be combined — each feeds its output to the next.

### SQL + Forecast

```yaml
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
```

### SQL + Aggregate (Join then Aggregate)

```yaml
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
```

### Aggregate + Forecast

```yaml
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
```

### When to Use Aggregate vs SQL

| Scenario | Use |
|----------|-----|
| Single source, standard GROUP BY | `aggregate` (preferred) |
| Multi-source joins | `sql` |
| Window functions, CTEs, CASE WHEN | `sql` |
| Join then aggregate | `sql` + `aggregate` |
| Simple forecast | `forecast` |
| Aggregate then forecast | `aggregate` + `forecast` |
| Full pipeline | `sql` + `aggregate` + `forecast` |

---

## QuackStats: SQL Functions (Advanced)

For standard forecasting, prefer the declarative `transform.forecast` stage. Use QuackStats SQL functions in `transform.sql` for advanced workflows — custom UNION logic, combining multiple forecast outputs, or complex SQL pipelines.

QuackStats is a DuckDB extension loaded automatically in ChartML's transform pipeline.

### `forecast()` Table Function

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
| `table_name` | VARCHAR | *required* | DuckDB table or CTE |
| `timestamp` | VARCHAR | `"timestamp"` | Date column name |
| `value` | VARCHAR | `"value"` | Numeric column name |
| `group_by` | VARCHAR[] | *none* | Per-group forecasts |
| `horizon` | INTEGER | `3` | Future steps to predict |
| `confidence_level` | DOUBLE | `0.95` | Prediction interval width |
| `model` | VARCHAR | `"auto"` | Model selection |

**Output columns:** `forecast_timestamp`, `forecast`, `lower_bound`, `upper_bound`

**Key behaviors:**
- Anchor row: first output is the last historical observation (connects forecast to actuals)
- Minimum 4 data points per series
- `group_by`: each group forecast independently; group columns appear as leading output columns

### `detect_seasonality()` Table Function

```sql
SELECT * FROM detect_seasonality(
  'table_name',
  timestamp = 'date_column',
  value = 'value_column',
  group_by = ['col1']
)
```

**Output columns:** `period` (INTEGER), `strength` (DOUBLE, 0.0-1.0)

Returns multiple rows if multiple periods detected, sorted by descending strength. Strength > 0.3 = meaningful, > 0.7 = strong. Minimum 8 data points.
