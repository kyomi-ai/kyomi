# ChartML Quick Reference for Dashboard Copilot

## Critical Rules

1. **Output YAML, not JSON** - Always use YAML syntax in ```chartml blocks
2. **columns = categories (X-axis), rows = values (Y-axis)** - Never mix these up
3. **Side-by-side charts must be in the SAME block** using YAML array syntax (`-`)
4. **When editing charts in an array, output the ENTIRE array** - Include all charts with the ````chartml``` fence

## Basic Structure

```chartml
type: chart
version: 1
title: "Chart Title"
data:
  datasource: production-postgres   # Use slug from list_datasources/search_catalog
  query: |
    SELECT category, SUM(value) as total
    FROM schema.table
    GROUP BY category
  cache:
    ttl: 6h                    # 30s | 5m | 6h | 24h | 7d
    autoRefresh: true          # Optional - auto-refresh when TTL expires
visualize:
  type: bar                    # bar | line | area | scatter | pie | doughnut | table | metric
  columns: category            # X-axis (categories)
  rows: total                  # Y-axis (values)
  style:
    height: 400                # Chart height in pixels (default: 400)
layout:
  colSpan: 6                   # 3=1/4 | 4=1/3 | 6=1/2 | 12=full width
```

## Chart Types

| Type | Use For | Default Height |
|------|---------|----------------|
| `bar` | Comparisons, rankings | 400px |
| `line` | Trends over time | 400px |
| `area` | Cumulative trends, composition | 400px |
| `scatter` | Correlations (x vs y) - NOTE: both columns AND rows are numeric | 400px |
| `pie` / `doughnut` | Part-to-whole | 400px |
| `table` | Detailed data display | 300px |
| `metric` | Single KPI value | 120px |

## Type-Specific Options

### Bar Chart
```yaml
visualize:
  type: bar
  orientation: vertical        # vertical | horizontal (use horizontal for rankings)
  mode: grouped                # grouped | stacked | normalized
```

**IMPORTANT - Horizontal Bar Charts:**
When using `orientation: horizontal`, **columns and rows stay the same**:
- `columns:` = categories (will appear on Y-axis in horizontal layout)
- `rows:` = values (will appear on X-axis in horizontal layout)

Example (top regions ranked by revenue):
```yaml
visualize:
  type: bar
  orientation: horizontal
  columns: region        # Categories on Y-axis
  rows: total_revenue    # Values on X-axis
```
Do NOT swap columns/rows for horizontal orientation - the orientation property handles the visual rotation.

### Multiple Series (Line/Area/Bar)

**Option A: Separate columns in data** (e.g., `revenue_us`, `revenue_eu`)
```yaml
visualize:
  type: line
  columns: month
  rows:
    - field: revenue_us
      label: "US"
    - field: revenue_eu
      label: "EU"
```

**Option B: Grouping dimension in data** (e.g., `region` column with values US/EU)
```yaml
visualize:
  type: bar
  mode: grouped              # grouped | stacked
  columns: month
  rows: revenue
  marks:
    color: region            # Creates separate series per region value
```

### Line/Area Chart
```yaml
visualize:
  type: line                   # or area
  mode: stacked                # stacked | normalized (area only)
  style:
    showDots: true
    strokeWidth: 2
```

**lineStyle** - Control line dash patterns per series:
```yaml
rows:
  - field: actual
    label: "Actual"
    lineStyle: solid           # solid (default) | dashed | dotted
  - field: forecast
    label: "Forecast"
    lineStyle: dashed
```

### Range Mark (Confidence Intervals)
```yaml
visualize:
  type: line
  columns: date
  rows:
    - field: forecast
      lineStyle: dashed
    - mark: range              # Shaded area between bounds
      upper: upper_bound
      lower: lower_bound
      color: "#4285f4"
      opacity: 0.15
      floor: 0                 # Optional: clamp rendered lower bound (e.g., 0 for non-negative metrics)
      # ceiling: 100           # Optional: clamp rendered upper bound (e.g., 100 for percentages)
```

### Metric Card
```yaml
visualize:
  type: metric
  value: current               # Field name for main value
  label: "Revenue"             # Optional label inside card
  format: "$,.0f"
  compareWith: previous        # Field name for comparison (shows trend arrow)
  invertTrend: false           # true = red for increase (e.g., costs)
```

### Table
```yaml
visualize:
  type: table
  columns:
    - field: name
      label: "Name"
      width: auto
    - field: revenue
      label: "Revenue"
      format: "$,.0f"
      width: 140
```

### Dual-Axis (Bar + Line)
```yaml
visualize:
  type: bar
  columns: month
  rows:
    - field: revenue
      mark: bar                # bar | line | area
      axis: left
    - field: customers
      mark: line
      axis: right
```

## Axes Configuration

```yaml
visualize:
  axes:
    rows:
      label: "Revenue ($)"
      format: "$,.0f"
      min: 0
      max: 100000
    right:
      label: "Count"
      format: ",.0f"
```

## Data Labels

```yaml
visualize:
  type: bar
  rows:
    field: revenue
    dataLabels:
      show: true
      position: top            # top | center | bottom
      format: "$,.0f"
```

## Annotations (Reference Lines)

```yaml
visualize:
  annotations:
    - type: line               # line | band
      axis: left
      value: 150000
      orientation: horizontal
      label: "Goal"
      color: "#34a853"
      dashArray: "5,5"         # Dashed line
```

## Number Formats (d3-format)

| Format | Output | Use For |
|--------|--------|---------|
| `$,.0f` | $1,234 | Currency, no decimals |
| `$,.2f` | $1,234.56 | Currency with decimals |
| `,.0f` | 1,234 | Numbers with thousands separator |
| `.1%` | 12.3% | Percentages |
| `.0%` | 12% | Percentages, no decimals |
| `~s` | 1.2K, 3.4M | SI-prefix (K, M, B) |

## Layout Examples

### Full Width Chart
```yaml
layout:
  colSpan: 12
```

### Two Charts Side-by-Side (SAME BLOCK!)
```chartml
- type: chart
  version: 1
  title: "Chart A"
  layout:
    colSpan: 6
  data: ...
  visualize: ...

- type: chart
  version: 1
  title: "Chart B"
  layout:
    colSpan: 6
  data: ...
  visualize: ...
```

### Four Metric Cards in a Row
```yaml
layout:
  colSpan: 3    # Each card takes 1/4 width
```

## Named Data Sources (Required for Multi-Source Joins)

Use named data sources when combining data from multiple datasources, or when using `{name}` placeholders in transform SQL:
```yaml
data:
  actuals:
    datasource: "production-postgres"
    query: "SELECT month, revenue FROM sales.monthly"
    cache:                       # ⚠️ cache goes INSIDE each named source
      ttl: 6h
  forecasts:
    datasource: "analytics-clickhouse"
    query: "SELECT month, predicted FROM forecasts.monthly"
transform:
  sql: |
    SELECT a.month, a.revenue, f.predicted
    FROM {actuals} a JOIN {forecasts} f USING (month)
```

Source names become table names in transform SQL via `{name}` placeholders.

## Transform Pipeline

Three optional stages, fixed order: `sql` → `aggregate` → `forecast`. Each stage operates on the previous stage's output (or source data). At least one stage required.

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

**SQL Stage** — DuckDB SQL with `{name}` placeholders matching keys in `data:`:
```yaml
transform:
  sql: |
    SELECT region, SUM(revenue) as total
    FROM {sales}
    GROUP BY region
    ORDER BY total DESC
```

**Aggregate Stage** — Declarative dimensions + measures:
```yaml
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
```

**Forecast Stage** — Time series forecasting (operates on output of previous stages):
```yaml
transform:
  aggregate:
    dimensions:
      - column: sale_date
        name: month
        type: month
    measures:
      - column: revenue
        aggregation: sum
        name: revenue
  forecast:
    timestamp: month
    value: revenue
    horizon: 6
    model: auto
```
Output columns from forecast: `forecast`, `lower_bound`, `upper_bound`, `is_forecast`.

**Multi-statement SQL** (array form, last statement returns result):
```yaml
transform:
  sql:
    - "CREATE OR REPLACE TABLE tmp AS SELECT ... FROM {sales}"
    - "SELECT ... FROM tmp"
```

## QuackStats: Forecasting & Seasonality in Transform SQL

For standard forecasting, prefer the declarative `transform.forecast` stage. Use QuackStats directly for advanced scenarios (custom models, detect_seasonality, per-group forecasts).

DuckDB table functions for time series analysis, available automatically in transform SQL.

### `forecast()` — Time Series Prediction
```sql
SELECT * FROM forecast('table_name', timestamp='col', value='col', horizon=6, confidence_level=0.95, model='auto')
```
- **Output**: `forecast_timestamp`, `forecast`, `lower_bound`, `upper_bound`
- **Models**: `auto` (CV-based best model selection), `ets`, `linear`, `exponential` (growth, requires values > 0), `logistic` (S-curve/saturation)
- **Anchor row**: First row = last historical point (connects forecast to actuals)
- **Min data**: 4 points. Use `group_by=['col']` for per-group forecasts.

### `detect_seasonality()` — Pattern Detection
```sql
SELECT * FROM detect_seasonality('table_name', timestamp='col', value='col')
```
- **Output**: `period` (INTEGER), `strength` (DOUBLE 0.0–1.0)
- **Min data**: 8 points. Strength > 0.3 = meaningful.

### Forecast Pattern

**Preferred: Forecast only** (when query already returns a time series):
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
      label: "Actual"
    - field: forecast
      lineStyle: dashed
      label: "Forecast"
    - mark: range
      upper: upper_bound
      lower: lower_bound
      label: "95% Confidence"
      floor: 0                   # Use floor: 0 for counts, revenue, etc. to avoid negative confidence bands
```

If the raw data needs rollup first (e.g., daily rows → monthly), add an `aggregate` stage before `forecast`.

**Advanced: QuackStats SQL for full control**
```chartml
data:
  revenue:
    datasource: "production-postgres"
    query: "SELECT date, SUM(revenue) as revenue FROM sales GROUP BY 1 ORDER BY 1"
transform:
  sql:
    - "CREATE OR REPLACE TABLE actuals AS SELECT date, revenue FROM {revenue}"
    - |
      CREATE OR REPLACE TABLE predictions AS
      SELECT * FROM forecast('actuals', timestamp='date', value='revenue', horizon=6)
    - |
      SELECT date as month, revenue as actual, NULL as forecast, NULL as lower_95, NULL as upper_95
      FROM actuals
      UNION ALL
      SELECT forecast_timestamp, NULL, forecast, lower_bound, upper_bound
      FROM predictions ORDER BY 1
visualize:
  type: line
  columns: month
  rows:
    - field: actual
      label: "Actual"
    - field: forecast
      lineStyle: dashed
      label: "Forecast"
    - mark: range
      upper: upper_95
      lower: lower_95
      label: "95% Confidence"
```

See **AI_AGENT_REFERENCE.md → QuackStats** section for full parameter details.

## Common Mistakes

1. **Wrong**: `columns: revenue, rows: region` - Revenue is a value, not a category!
2. **Wrong**: Separate ```chartml blocks for side-by-side charts - they'll stack vertically
3. **Wrong**: Using JSON syntax `{"type": "chart"}` - Always use YAML
4. **Wrong**: `chart:` instead of `visualize:` - The correct key is `visualize`
5. **Wrong**: `marks.color: "#d32f2f"` - Use `style.colors: ["#d32f2f"]` for static colors. `marks.color` expects a field name.
6. **Wrong**: Using a string reference (`data: my_source`) with `transform:` - Transform requires an inline data source definition with `datasource:` and `query:`
7. **Wrong**: Putting `cache:` as sibling of named source instead of inside it - `cache:` goes INSIDE each named source definition

## Need More Details?

Use the `get_chartml_spec` tool to look up advanced features:
- `data` - SQL queries, datasource configuration, cache settings
- `visualize` - Full chart type options
- `marks` - Color scales, conditional formatting
- `axes` - Axis configuration details
- `format` - Complete number/date formatting reference
- `layout` - Grid system details
- `transform` - Transform pipeline: SQL preprocessing, declarative aggregation, forecasting
