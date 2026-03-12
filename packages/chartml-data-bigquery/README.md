# @chartml/data-bigquery

BigQuery data source plugin for ChartML - execute SQL queries against Google BigQuery and visualize the results.

## Installation

```bash
npm install @chartml/data-bigquery @chartml/core
```

## Usage

### Basic Setup

```javascript
import { ChartML } from '@chartml/core';
import { createBigQueryDataSource } from '@chartml/data-bigquery';

// Create BigQuery data source
const bigquerySource = createBigQueryDataSource({
  projectId: 'my-gcp-project',
  getAccessToken: async () => {
    // Return a valid OAuth 2.0 access token
    return 'ya29.a0AfH6SMBx...';
  }
});

// Register with ChartML
const chartml = new ChartML();
chartml.registerDataSource('bigquery', bigquerySource);

// Use in ChartML spec
const spec = `
type: source
name: sales_data
provider: bigquery
query: |
  SELECT
    DATE_TRUNC(order_date, MONTH) as month,
    SUM(revenue) as total_revenue
  FROM \`my-project.dataset.sales\`
  WHERE order_date >= '2024-01-01'
  GROUP BY month
  ORDER BY month
`;

await chartml.render(spec, container);
```

### Advanced Configuration

```javascript
const bigquerySource = createBigQueryDataSource({
  projectId: 'my-gcp-project',
  getAccessToken: async () => getToken(),

  // Optional: BigQuery dataset location
  location: 'US',  // Default: 'US'

  // Optional: Maximum rows to return
  maxResults: 50000,  // Default: 10000

  // Optional: Use legacy SQL
  useLegacySQL: false  // Default: false (uses Standard SQL)
});
```

### ChartML Spec Options

```yaml
type: source
name: my_data
provider: bigquery

# Required: SQL query
query: SELECT * FROM `project.dataset.table` LIMIT 1000

# Optional: Override project ID for this query
projectId: other-project

# Optional: Override max results for this query
maxResults: 100000

# Optional: Query timeout in milliseconds
timeoutMs: 60000
```

## Complete Example

```yaml
# Define a reusable BigQuery data source
type: source
name: monthly_revenue
provider: bigquery
query: |
  SELECT
    FORMAT_DATE('%Y-%m', order_date) as month,
    product_category,
    SUM(revenue) as total_revenue,
    COUNT(DISTINCT customer_id) as unique_customers
  FROM `my-project.analytics.sales`
  WHERE order_date BETWEEN '2024-01-01' AND '2024-12-31'
  GROUP BY month, product_category
  ORDER BY month, total_revenue DESC

---

# Use the data source in a chart
type: chart
dataSource: monthly_revenue
visualize:
  type: bar
  columns: month
  rows: total_revenue
  style:
    title: "Monthly Revenue by Category"
    colors: ['#3B82F6', '#10B981', '#F59E0B']
```

## OAuth Token Management

The plugin requires a valid OAuth 2.0 access token with BigQuery permissions. Here's how to integrate with different auth systems:

### With Firebase Auth

```javascript
import { getAuth } from 'firebase/auth';

const bigquerySource = createBigQueryDataSource({
  projectId: 'my-project',
  getAccessToken: async () => {
    const auth = getAuth();
    const user = auth.currentUser;
    if (!user) throw new Error('Not authenticated');

    const token = await user.getIdToken();
    return token;
  }
});
```

### With Google Identity Services

```javascript
import { google } from 'googleapis';

const oauth2Client = new google.auth.OAuth2(
  CLIENT_ID,
  CLIENT_SECRET,
  REDIRECT_URI
);

const bigquerySource = createBigQueryDataSource({
  projectId: 'my-project',
  getAccessToken: async () => {
    const { token } = await oauth2Client.getAccessToken();
    return token;
  }
});
```

### With Custom Token Store

```javascript
const bigquerySource = createBigQueryDataSource({
  projectId: 'my-project',
  getAccessToken: async () => {
    // Retrieve from your token storage
    const token = await tokenStore.get('bigquery_access_token');

    // Check if expired and refresh if needed
    if (isTokenExpired(token)) {
      return await refreshToken();
    }

    return token;
  }
});
```

## Error Handling

The plugin provides detailed error messages:

```javascript
try {
  await chartml.render(spec, container);
} catch (error) {
  if (error.message.includes('BigQuery API error')) {
    // Handle BigQuery-specific errors
    console.error('Query failed:', error.originalError);
    console.error('SQL:', error.query);
  }
}
```

Common errors:
- `projectId is required` - Missing project ID in configuration
- `getAccessToken must be a function` - Invalid token provider
- `query field is required` - Missing SQL query in spec
- `BigQuery API error (401)` - Invalid or expired OAuth token
- `BigQuery API error (403)` - Insufficient permissions
- `BigQuery query timeout` - Query took longer than 30 seconds

## Performance Considerations

### Query Caching

BigQuery's query cache is automatically enabled. Identical queries will return cached results when possible.

### Result Limits

- Default: 10,000 rows
- Maximum: Configure via `maxResults` option
- For large datasets, consider:
  - Aggregating in SQL
  - Using `LIMIT` in queries
  - Implementing pagination

### Timeouts

- Default: 30 seconds
- Override: Set `timeoutMs` in spec
- Long queries will poll for results every 1 second (max 30 attempts)

## BigQuery Permissions Required

The OAuth token must have these BigQuery permissions:

- `bigquery.jobs.create` - Create query jobs
- `bigquery.jobs.get` - Retrieve query results
- `bigquery.tables.getData` - Read table data

Recommended IAM role: `roles/bigquery.user`

## Type Conversions

BigQuery types are automatically converted to JavaScript types:

| BigQuery Type | JavaScript Type |
|--------------|----------------|
| INTEGER, INT64 | Number |
| FLOAT, FLOAT64, NUMERIC | Number |
| BOOLEAN, BOOL | Boolean |
| STRING | String |
| TIMESTAMP | Date |
| DATE | String (YYYY-MM-DD) |

## Development

```bash
# Install dependencies
npm install

# Build
npm run build

# Watch mode
npm run dev
```

## License

MIT

## Related Packages

- `@chartml/core` - Core ChartML library
- `@chartml/aggregate-duckdb` - DuckDB aggregation middleware
- `@chartml/react` - React wrapper for ChartML
