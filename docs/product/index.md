---
layout: doc
title: "Documentation & Help"
description: "Comprehensive guides for Kyomi — the data intelligence platform. Setup, Connect, ChartML, datasources, and features."
---

# Documentation & Help

Welcome to Kyomi! Find guides for connecting your data, building dashboards, and getting the most out of the platform.

## Getting Started

- **[Quick Start](/docs/#quick-start)** — Connect your data, start chatting, build dashboards

## Kyomi Connect

Keep database credentials on your own infrastructure by running a lightweight agent inside your network.

- **[Overview](/docs/connect/)** — What Connect is, architecture, quick start
- **[Installation](/docs/connect/installation)** — Binary, Docker, Compose, Kubernetes, ECS
- **[Configuration](/docs/connect/configuration)** — Environment variables, health checks, setup wizard
- **[Security Model](/docs/connect/security)** — JWT auth, data flow, token rotation, network requirements
- **[Troubleshooting](/docs/connect/troubleshooting)** — Common issues and fixes

## Datasource Setup

- **[Overview](/docs/datasources/)** — Supported platforms, auth methods, shared vs personal credentials
- **[BigQuery](/docs/datasources/bigquery)** — OAuth, Service Account, Enterprise OAuth
- **[Snowflake](/docs/datasources/snowflake)** — Password, OAuth, Key Pair
- **[PostgreSQL](/docs/datasources/postgres)** — Password auth with SSH tunnel
- **[MySQL](/docs/datasources/mysql)** — Password auth with SSH tunnel
- **[ClickHouse](/docs/datasources/clickhouse)** — Password auth, HTTP/HTTPS
- **[SQL Server](/docs/datasources/sqlserver)** — SQL authentication with SSH tunnel
- **[Redshift](/docs/datasources/redshift)** — Password auth with SSH tunnel
- **[Databricks](/docs/datasources/databricks)** — Personal Access Token
- **[Azure Synapse](/docs/datasources/synapse)** — SQL authentication

## ChartML

Human-readable YAML format for defining data visualizations.

- **[ChartML Reference](/docs/chartml/)** — Essential reference: chart types, syntax, formatting, grid layout
- **[Data Sources](/docs/chartml/source)** — Query, inline data, multi-source charts, cache configuration
- **[Parameters](/docs/chartml/params)** — Interactive controls (date, select, multiselect, number, text)
- **[Chart & Visualize](/docs/chartml/chart)** — All chart types, axes, marks, annotations, line styles
- **[Transform Pipeline](/docs/chartml/transform)** — SQL, declarative aggregation, and forecasting stages
- **[Style & Formatting](/docs/chartml/style)** — Color palettes, themes, number/date formatting
- **[Config](/docs/chartml/config)** — Scope-level defaults
- **[Grid Layout](/docs/chartml/grid)** — 12-column grid, multi-chart dashboards

## Features

- **[Creating Charts](/docs/#creating-charts)** — AI, Chart Builder, or write ChartML directly
- **[Dashboard Parameters](/docs/#dashboard-parameters)** — Interactive filters with date, select, text inputs
- **[SQL Editor](/docs/#sql-editor)** — Syntax highlighting, dry run, history, keyboard shortcuts
- **[AI Agent Features](/docs/#ai-agent-features)** — Custom knowledge, auto-learnings, token usage
- **[PDF Export](/docs/#pdf-export)** — Export dashboards as professional PDF documents
- **[Kyomi Watch](/docs/watches)** — Proactive AI monitoring with scheduled alerts
- **[Website Analytics](/docs/analytics)** — Privacy-first analytics
- **[Slack Integration](/docs/slack)** — Data intelligence in Slack
- **[MCP Server](/docs/mcp)** — Connect AI coding agents to your data

## Advanced

- **[Client-Side Data Processing (DuckDB)](/docs/#client-side-data-processing-duckdb)** — Transform pipeline, SQL stage, declarative aggregation
- **[Forecasting](/docs/#forecasting)** — Time series forecasting with QuackStats and confidence intervals
- **[Multi-Source Charts](/docs/#multi-source-charts)** — Combine data from multiple databases in one chart
- **[Multi-Series & Dual-Axis Charts](/docs/#advanced-features)** — Multiple metrics, different scales, annotations

---

## Quick Start

### 1. Connect Your Data
Connect your data warehouses and databases through **Settings → Datasources**.

**Supported platforms:**
- Cloud: BigQuery, Snowflake, Redshift, Azure Synapse, Databricks
- Databases: PostgreSQL, MySQL, SQL Server, ClickHouse

Kyomi will automatically index your tables and columns to help the AI understand your data.

### 2. Start Chatting
Ask questions in natural language and the AI will query your data, create visualizations, and provide insights.

### 3. Build Dashboards
Save important charts and analyses to dashboards for quick access and sharing with your team.

---

## Creating Charts

Kyomi uses **ChartML** - a simple, human-readable format for defining data visualizations. You can create charts in three ways:

### 1. Ask the AI
Simply describe what you want to see:
- "Show me revenue by month as a line chart"
- "Create a bar chart of sales by region"
- "Make a table of top 10 customers"

The AI will write the ChartML for you.

### 2. Use the Chart Builder
Click the chart icon in the SQL editor or dashboard editor to open the visual chart builder with:
- **SQL Editor Tab**: Write or paste your query
- **Chart Config Tab**: Configure visualization with live preview
- **AI Copilot**: Get help refining your chart

### 3. Write ChartML Directly
For full control, write ChartML directly in your dashboards or use the Chart Config tab.

---

## ChartML Basics

ChartML uses simple YAML syntax to define visualizations:

````yaml
```chartml
data:
  datasource: "my-bigquery"  # Datasource slug from Settings → Datasources
  query: |
    SELECT month, revenue
    FROM `project.dataset.sales`

visualize:
  type: line
  columns: month
  rows: revenue
  style:
    title: "Monthly Revenue"
    height: 400
```
````

### Key Concepts

**`data:`** - Where your data comes from
- `datasource:` - Datasource slug (e.g., `"my-bigquery"`, `"production-postgres"`)
- `query:` - SQL query for your datasource
- `inline:` - Hardcoded data array

**`visualize:`** - How to display the data
- `type:` - Chart type (bar, line, area, scatter, pie, donut, metric, table)
- `columns:` - X-axis / categories (NOT `x:`)
- `rows:` - Y-axis / values (NOT `y:`)
- `style:` - Customization (title, colors, height, etc.)

**`transform:`** - Data transformation pipeline (optional)
- Three composable stages: `sql`, `aggregate`, `forecast`
- Runs in DuckDB for fast client-side processing

---

## Chart Types

### <InlineIcon icon="ChartBarIcon" /> Bar Chart
Best for comparing categories.

```yaml
visualize:
  type: bar
  columns: category
  rows: value
```

### <InlineIcon icon="ArrowTrendingUpIcon" /> Line Chart
Best for trends over time.

```yaml
visualize:
  type: line
  columns: date
  rows: value
  style:
    showDots: true
```

### <InlineIcon icon="ChartBarSquareIcon" /> Area Chart
Like line charts but with filled area underneath.

```yaml
visualize:
  type: area
  columns: date
  rows: value
```

### <InlineIcon icon="CircleStackIcon" /> Scatter Plot
For showing relationships between two numeric variables.

```yaml
visualize:
  type: scatter
  columns: x_value
  rows: y_value
```

### <InlineIcon icon="ChartPieIcon" /> Pie / Donut Chart
For showing proportions of a whole.

```yaml
visualize:
  type: donut
  columns: category
  rows: value
```

### <InlineIcon icon="TableCellsIcon" /> Table Chart
Interactive data tables with sorting and pagination.

```yaml
visualize:
  type: table
  columns:
    - { field: name, label: "Customer" }
    - { field: revenue, label: "Revenue" }
  style:
    pageSize: 25
    height: 600
```

**Table Features:**
- Click column headers to sort (ascending/descending)
- Ctrl+click for multi-column sorting
- Pagination with customizable page size (10, 25, 50, 100 rows)
- Scrollable with fixed headers

### <InlineIcon icon="HashtagIcon" /> Metric Card
Single key numbers with optional comparison.

```yaml
visualize:
  type: metric
  value: 1234
  label: "Total Revenue"
  comparison:
    value: 15
    label: "vs last month"
```

---

## Data Sources

### Cloud Data Warehouses

Query your connected cloud platforms:

```yaml
data:
  datasource: "my-snowflake"  # Use the datasource slug
  query: |
    SELECT
      DATE_TRUNC('month', order_date) as month,
      SUM(amount) as revenue
    FROM sales.orders
    WHERE order_date >= DATEADD(month, -12, CURRENT_DATE())
    GROUP BY month
    ORDER BY month
```

### Relational Databases

Query PostgreSQL, MySQL, and SQL Server:

```yaml
data:
  datasource: "production-postgres"
  query: |
    SELECT
      DATE_TRUNC('month', order_date) as month,
      SUM(amount) as revenue
    FROM orders
    WHERE order_date >= NOW() - INTERVAL '12 months'
    GROUP BY month
    ORDER BY month
```

### Inline Data

For static data or examples:

```yaml
data:
  inline:
    - { month: "Jan", revenue: 1000 }
    - { month: "Feb", revenue: 1200 }
    - { month: "Mar", revenue: 1100 }
```

### Multi-Datasource Dashboards

Combine data from multiple sources in one dashboard:

```yaml
# Chart 1: BigQuery data
data:
  datasource: "analytics-bq"
  query: SELECT ... FROM bigquery_table

---

# Chart 2: PostgreSQL data
data:
  datasource: "production-postgres"
  query: SELECT ... FROM postgres_table
```

---

## Client-Side Data Processing (DuckDB)

Kyomi includes a powerful **DuckDB middleware** that runs in your browser for fast data transformations:

### Transform Pipeline

The `transform:` block contains three optional, composable stages that run in order:

- **`sql:`** — SQL preprocessing (joins, CTEs, window functions)
- **`aggregate:`** — Declarative aggregation (dimensions, measures, filters)
- **`forecast:`** — Time series forecasting with confidence intervals

#### SQL Stage

Write SQL to group, filter, and transform your data client-side:

```yaml
transform:
  sql: |
    SELECT region, SUM(revenue) as total_revenue, AVG(quantity) as avg_quantity
    FROM {sales}
    GROUP BY region
    HAVING SUM(revenue) > 1000
    ORDER BY total_revenue DESC
```

Use `{sourceName}` placeholders to reference named data sources. You have full DuckDB SQL available — joins, window functions, CTEs, and more.

#### Declarative Aggregate Stage

Group and aggregate data without writing SQL:

```yaml
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
    sort:
      - field: month
        direction: asc
    limit: 100
```

### Why DuckDB?

- **Fast**: Aggregations run in milliseconds
- **No API calls**: Transform cached data instantly
- **Free**: Doesn't count against your datasource quota
- **Interactive**: Change filters/groupings without waiting

---

## Forecasting

Kyomi includes **QuackStats**, a DuckDB extension we built for time series forecasting and seasonality detection. You can use it declaratively via the `transform.forecast` stage, or call the SQL functions directly in the `transform.sql` stage.

### The `forecast()` Function

Generates time series forecasts with prediction intervals:

```sql
SELECT * FROM forecast(
  'table_name',
  timestamp = 'date_column',
  value = 'value_column',
  horizon = 6,
  confidence_level = 0.95,
  model = 'auto'
)
```

**Parameters:**

| Parameter | Default | Description |
|-----------|---------|-------------|
| `table_name` | *required* | DuckDB table or CTE to forecast |
| `timestamp` | `"timestamp"` | Name of the DATE column |
| `value` | `"value"` | Name of the numeric column to forecast |
| `horizon` | `3` | Number of future steps to predict |
| `confidence_level` | `0.95` | Width of prediction interval (0.0–1.0) |
| `model` | `"auto"` | Model type: `auto`, `linear`, `exponential`, `logistic`, `ets` |

**Output columns:** `forecast_timestamp`, `forecast`, `lower_bound`, `upper_bound`

**Model types:**
- **auto** (recommended) — Cross-validates all models and picks the best fit
- **linear** — Straight line projection
- **exponential** — Fits y = a * exp(b * x), best for consistent percentage growth
- **logistic** — S-curve / saturation patterns approaching a capacity limit
- **ets** — Exponential smoothing with trend and seasonality

### Forecast ChartML Example

Add a forecast to any time series chart with the declarative `forecast:` stage:

````yaml
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
    confidence_level: 0.95

visualize:
  type: line
  columns: date
  rows:
    - field: revenue
      label: "Actual Revenue"
    - field: forecast
      label: "Forecast"
      lineStyle: dashed
    - mark: range
      upper: upper_bound
      lower: lower_bound
      label: "95% Confidence"
      opacity: 0.15
  axes:
    left:
      format: "$,.0f"
```
````

The `forecast:` stage generates the forecast SQL automatically. The output includes `forecast`, `lower_bound`, `upper_bound`, and `is_forecast` columns alongside your original data.

You can also combine stages — for example, aggregate monthly data then forecast:

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

For advanced use cases, you can still call the `forecast()` SQL function directly in the `sql:` stage.

### Seasonality Detection

Detect repeating periodic patterns in time series data:

```sql
SELECT * FROM detect_seasonality(
  'table_name',
  timestamp = 'date_column',
  value = 'value_column'
)
```

Returns detected periods with strength scores.

Forecasting is available on **all tiers** — Free, Starter, Pro, and Team.

---

## Multi-Source Charts

Combine data from multiple databases in a single chart using named data sources.

### Named Data Sources

Instead of a single `datasource` + `query`, provide named keys under `data:`:

````yaml
```chartml
type: chart
version: 1
title: "Revenue vs Marketing Spend"

data:
  sales:
    datasource: "production-postgres"
    query: |
      SELECT month, SUM(revenue) as revenue
      FROM orders GROUP BY month
  marketing:
    datasource: "analytics-bigquery"
    query: |
      SELECT month, SUM(spend) as ad_spend
      FROM campaigns GROUP BY month

transform:
  sql: |
    SELECT s.month, s.revenue, m.ad_spend,
           s.revenue / m.ad_spend as roas
    FROM {sales} s
    JOIN {marketing} m USING (month)
    ORDER BY s.month

visualize:
  type: line
  columns: month
  rows: [revenue, ad_spend]
  axes:
    left:
      label: "Revenue ($)"
      format: "$,.0f"
```
````

Each named source runs against its own datasource. Use `{sourceName}` placeholders in the `transform.sql` stage to reference each source's results. The join happens locally in DuckDB.

---

## Line Styles & Confidence Bands

### Line Styles

Set `lineStyle` on any row to change its appearance:

```yaml
rows:
  - field: actual
    label: "Actual"
    lineStyle: solid    # default
  - field: forecast
    label: "Forecast"
    lineStyle: dashed
  - field: baseline
    label: "Baseline"
    lineStyle: dotted
```

Available styles: `solid` (default), `dashed`, `dotted`

### Confidence Bands (Range Marks)

Use `mark: range` to render a shaded area between upper and lower bounds:

```yaml
rows:
  - field: forecast
    label: "Forecast"
    lineStyle: dashed
  - mark: range
    upper: upper_bound
    lower: lower_bound
    label: "95% Confidence"
    color: "#34a853"
    opacity: 0.15
```

Range marks work on both left and right axes. Combined with `lineStyle: dashed`, they create professional forecast visualizations.

---

## PDF Export

Export any dashboard as a professional PDF document.

### How to Export

1. Open a dashboard in the **Dashboard Viewer**
2. Click the **Download PDF** button in the toolbar
3. The PDF is generated with all charts rendered at high resolution

### What's Included

- All chart types render correctly (line, bar, area, scatter, pie, metric, table)
- Forecast lines, confidence bands, and multi-source charts work in PDFs
- Professional A4 layout with page numbers
- Consistent formatting and clean page breaks

PDF export is available on **Pro and Team** plans.

---

## Dashboard Parameters

Make your dashboards interactive with parameters:

### Define Parameters

```yaml
params:
  - name: start_date
    type: date
    default: "2024-01-01"
  - name: region
    type: select
    options: [US, EU, APAC]
    default: US
```

### Use in Queries

Reference parameters with `$params`:

```yaml
data:
  query: |
    SELECT *
    FROM sales
    WHERE date >= '$params.start_date'
      AND region = '$params.region'
```

### Parameter Types

- `date` - Date picker
- `select` - Dropdown menu
- `multiselect` - Multi-select dropdown
- `text` - Text input
- `number` - Numeric input

---

## Styling & Customization

### Color Palettes

Kyomi includes beautiful color palettes:

```yaml
style:
  colorPalette: spectrum_pro  # or autumn_forest, horizon_suite
```

**Custom colors:**

```yaml
style:
  colors:
    - "#FF6B6B"
    - "#4ECDC4"
    - "#45B7D1"
```

### Chart Dimensions

```yaml
style:
  height: 400        # Chart height in pixels
  width: "100%"      # Full width (default)
```

### Axes & Labels

```yaml
style:
  title: "My Chart"
  xAxisLabel: "Date"
  yAxisLabel: "Revenue ($)"
  showLegend: true
  legendPosition: "right"  # or "top", "bottom", "left"
```

### Number Formatting

```yaml
style:
  format: "$,.2f"     # Currency: $1,234.56
  format: ",.0f"      # Thousands: 1,234
  format: ".1%"       # Percentage: 45.2%
```

---

## Advanced Features

### Multi-Series Charts

Multiple metrics on one chart:

```yaml
visualize:
  type: line
  columns: month
  rows: [revenue, cost, profit]
  style:
    title: "Financial Overview"
```

### Dual-Axis Charts

Different scales for different metrics:

```yaml
visualize:
  type: line
  columns: month
  rows: revenue
  secondaryAxis:
    rows: conversion_rate
    format: ".1%"
```

### Annotations

Add reference lines and markers:

```yaml
style:
  annotations:
    - type: line
      value: 1000
      label: "Target"
      color: "#FF0000"
```

---

## Datasource Connection

### Setting Up

1. Go to **Settings → Datasources**
2. Click **Add Datasource**
3. Select your platform type
4. Enter connection details
5. Click **Test & Discover** to verify

### Detailed Setup Guides

For step-by-step instructions, see the [Datasource Setup Guides](/docs/datasources/):

- **[Google BigQuery](/docs/datasources/bigquery)** - OAuth, Service Account, or Enterprise OAuth
- **[Snowflake](/docs/datasources/snowflake)** - Password, OAuth, or Key Pair
- **[PostgreSQL](/docs/datasources/postgres)** - Password auth with SSH tunnel support
- **[MySQL](/docs/datasources/mysql)** - Password auth with SSH tunnel support
- **[ClickHouse](/docs/datasources/clickhouse)** - Password auth, HTTP/HTTPS
- **[SQL Server](/docs/datasources/sqlserver)** - SQL authentication with SSH tunnel
- **[Amazon Redshift](/docs/datasources/redshift)** - Password auth with SSH tunnel
- **[Databricks](/docs/datasources/databricks)** - Personal Access Token
- **[Azure Synapse](/docs/datasources/synapse)** - SQL authentication

### Authentication Methods

| Platform | Auth Options |
|----------|--------------|
| BigQuery | [OAuth, Service Account, Enterprise OAuth](/docs/datasources/bigquery) |
| Snowflake | [Password, OAuth, Key Pair](/docs/datasources/snowflake) |
| Databricks | [Personal Access Token](/docs/datasources/databricks) |
| PostgreSQL | [Password](/docs/datasources/postgres) |
| MySQL | [Password](/docs/datasources/mysql) |
| Redshift | [Password](/docs/datasources/redshift) |
| SQL Server | [Password](/docs/datasources/sqlserver) |
| ClickHouse | [Password](/docs/datasources/clickhouse) |
| Azure Synapse | [SQL Auth](/docs/datasources/synapse) |

### Shared vs Personal Credentials

**Shared Credentials (Admin-configured):**
- All workspace users share the same database credentials
- Good for read-only analytics connections
- Admin configures once, everyone uses

**Personal Credentials (User-configured):**
- Each user provides their own database credentials
- Queries run with individual user permissions
- Better for audit trails and access control

### SSH Tunnels (On-Premise Databases)

For databases behind firewalls:
1. Enable "Use SSH Tunnel" in datasource settings
2. Enter SSH host, port, username
3. Add SSH public key or password
4. Kyomi connects securely through the tunnel

### Billing

You pay your cloud provider directly for data processing:
- **BigQuery**: Bytes processed per query (Google billing)
- **Snowflake**: Compute credits (Snowflake billing)
- **Redshift/Databricks**: Cluster time (AWS/Databricks billing)

Kyomi charges a flat per-user subscription (AI included)—your data platform costs are separate.

---

## AI Agent Features

### Custom Knowledge

Train the AI on your business context:

**Workspace Knowledge** (Settings → Knowledge → Workspace)
- Shared across all team members
- Define metrics, terminology, business rules
- Admin-only editing

**User Knowledge** (Settings → Knowledge → User)
- Private to you
- Personal preferences, common queries
- Your own reference notes

**Auto-Learnings** (Settings → Knowledge → Learnings)
- AI automatically learns from conversations
- Discovered tables and schemas
- Query patterns and preferences

### Token Usage

Monitor AI usage in **Settings → Usage**:
- See percentage of monthly budget used
- Breakdown by feature (Chat, SQL Copilot, Chart Copilot)
- Upgrade tier for more AI budget

---

## SQL Editor

### Features

- **Syntax highlighting**: SQL syntax with autocomplete
- **Dry run validation**: Check queries before running
- **Query history**: Auto-saved queries with search
- **Star favorites**: Prevent auto-deletion
- **Performance stats**: Execution time and bytes processed
- **Pagination**: Navigate large result sets
- **Create charts**: Turn results into visualizations

### Keyboard Shortcuts

- **Ctrl/Cmd + Enter**: Run query
- **Ctrl/Cmd + S**: Save to history
- **Ctrl/Cmd + /**: Toggle comment

---

## Tips & Tricks

### Performance

1. **Cache everything**: Kyomi caches query results in DuckDB - refresh only when needed
2. **Use aggregations**: Transform data client-side instead of requerying
3. **Limit large queries**: Use `LIMIT` for exploration, refine before full run
4. **Arrow streaming**: Enable in Settings for 20-100x faster data transfer (Pro+)

### Chart Design

1. **Keep it simple**: One insight per chart
2. **Choose the right type**: Line for trends, bar for comparisons, table for details
3. **Add context**: Use titles, labels, and annotations
4. **Test interactivity**: Verify sorting and pagination on tables

### Working with AI

1. **Be specific**: "Line chart of daily revenue for last 30 days" beats "show sales"
2. **Iterate**: Refine charts by asking follow-up questions
3. **Add context**: Put important business rules in Workspace Knowledge
4. **Use copilots**: SQL and Chart copilots are faster for quick edits

---

## ChartML Reference

For the complete ChartML specification with all options and examples, visit:

**[chartml.org →](https://chartml.org)**

The ChartML docs include:
- Complete property reference
- Advanced examples
- JSON schema for validation
- Language specification

---

## Need More Help?

- **In-app AI**: Ask questions in chat - the AI knows how to help
- **Community**: Join our Discord community (coming soon)
- **Support**: Email support@kyomi.ai
- **Updates**: Follow [@kyomi_analyst](https://x.com/kyomi_analyst) for product updates

---

*Last updated: March 2026*
