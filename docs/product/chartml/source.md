---
layout: doc
title: "ChartML: Data Sources"
description: "ChartML source component — query, inline data, multi-source charts, cache configuration, and datasource slug usage."
---

# Data Sources

The `data:` block defines where chart data comes from. ChartML supports database queries, inline data, named reusable sources, and multi-source charts that combine data from different databases.

---

## Source Component

Define reusable data sources that multiple charts can reference:

```yaml
type: source
version: 1
name: quarterly_sales
datasource: "production-postgres"
query: |
  SELECT region, SUM(revenue) as revenue
  FROM sales.transactions
  GROUP BY region
cache:
  ttl: 12h
```

**Properties:**

| Property | Required | Description |
|----------|----------|-------------|
| `name` | Yes | Unique identifier for referencing this source |
| `datasource` | Preferred | Datasource slug from Settings → Datasources |
| `provider` | Alternative | Provider type (bigquery, postgres, clickhouse, etc.) |
| `query` | For SQL providers | SQL query string |
| `rows` | For inline | Array of data objects |
| `cache.ttl` | No | Cache duration (e.g., `30s`, `5m`, `6h`, `24h`, `7d`) |
| `cache.autoRefresh` | No | Auto-refresh when TTL expires (default: false) |

---

## Datasource Selection

### Named Slug (Preferred)

Use the slug configured in Settings → Datasources:

```yaml
data:
  datasource: "production-postgres"
  query: |
    SELECT region, SUM(revenue) as total
    FROM sales.transactions
    GROUP BY region
  cache:
    ttl: 6h
```

Slugs are lowercase alphanumeric with hyphens (e.g., `production-postgres`, `analytics-bq`).

### Provider Shorthand

Auto-resolves when your workspace has only one datasource of that type:

```yaml
data:
  provider: bigquery
  query: |
    SELECT region, SUM(revenue) as total
    FROM `project.dataset.sales`
    GROUP BY region
```

**Resolution logic:**
- 1 match → use it
- 0 matches → error "No {provider} datasource configured"
- 2+ matches → error "Multiple {provider} datasources — please specify datasource slug"

---

## Inline Data Source

For static data or examples:

```yaml
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

## Chart-Level Data

Charts can define data inline instead of referencing a named source:

### Option 1: Reference a Named Source

```yaml
data: quarterly_sales    # String reference to a named Source
```

### Option 2: Inline Query

```yaml
data:
  datasource: "production-postgres"
  query: |
    SELECT region, SUM(revenue) as total
    FROM sales.transactions
    GROUP BY region
  cache:
    ttl: 6h
```

### Option 3: Inline Data Rows

```yaml
data:
  provider: inline
  rows:
    - month: "Jan"
      sales: 1200
    - month: "Feb"
      sales: 1350
```

---

## Named Data Sources (Multi-Source Charts)

Combine data from multiple databases by providing named keys under `data:`. Each named source is fetched independently and made available as a DuckDB table in the `transform.sql` stage.

```yaml
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
- Use `{name}` placeholders in `transform.sql` to reference each source
- Multiple named sources **require** `transform.sql` to join or combine them
- `cache:` goes inside each named source, not at the top level

---

## Cache Configuration

Control how long query results are cached:

```yaml
cache:
  ttl: 6h               # Time-to-live
  autoRefresh: true      # Auto-refresh when TTL expires
```

**TTL format:** `<number><unit>` where unit is `s` (seconds), `m` (minutes), `h` (hours), `d` (days).

Examples: `30s`, `5m`, `6h`, `24h`, `1d`, `7d`

**Auto-refresh** requires the datasource admin to allow it (to prevent unexpected costs on pay-per-query sources).
