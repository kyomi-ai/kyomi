# Connecting to Databricks

Connect Kyomi to your Databricks SQL warehouse for AI-powered analytics.

## Connection Details

| Field | Description | Example |
|-------|-------------|---------|
| **Server Hostname** | Databricks workspace URL | `dbc-xxxxxxxx-xxxx.cloud.databricks.com` |
| **HTTP Path** | SQL warehouse path | `/sql/1.0/warehouses/xxxx` |
| **Catalog** | Unity Catalog or hive_metastore | `main` |
| **Default Schema** | Default schema for queries | `default` |

---

## Prerequisites

- Databricks workspace with SQL warehouse
- Authentication: Personal Access Token (PAT) **or** OAuth
- Appropriate permissions on catalogs and schemas

---

## Setup Steps

### Step 1: Get Connection Details

1. Open your Databricks workspace
2. Go to **SQL Warehouses**
3. Select your warehouse
4. Click **Connection Details** tab
5. Note the **Server hostname** and **HTTP path**

::: info SQL Warehouse Types
- **Serverless**: Instant startup, auto-scales, pay-per-query
- **Pro**: Fixed size, good for predictable workloads
- **Classic**: Legacy option, full control over compute
:::

### Step 2: Choose Authentication Method

Databricks supports two authentication methods:

#### Option A: Personal Access Token (PAT)

Best for quick setup and individual users.

1. In Databricks, click your username → **Settings**
2. Go to **Developer** → **Access tokens**
3. Click **Generate new token**
4. Give it a description (e.g., "Kyomi Analytics")
5. Set expiration (or no expiration for long-term use)
6. Copy the token immediately (you won't see it again)

#### Option B: OAuth (Recommended for Teams)

Best for organizations that want users to authenticate with their Databricks accounts.

**Admin Setup (one-time):**

1. Go to **Databricks Account Console** at [accounts.cloud.databricks.com](https://accounts.cloud.databricks.com)
2. Navigate to **Settings** → **App connections**
3. Click **Add connection**
4. Configure:
   - **Name**: `Kyomi Analytics`
   - **Redirect URI**: `https://your-kyomi-domain.com/auth/oauth/databricks/callback`
   - **Scopes**: Enable `all-apis`, `sql`, `offline_access`
5. Save and copy the **Client ID** and **Client Secret**

**User Setup:**

1. In Kyomi, select **OAuth** as authentication mode
2. Admin enters the Client ID and Client Secret (if not already configured)
3. Click **Connect with Databricks**
4. Sign in with your Databricks account in the popup
5. Authorize Kyomi to access your workspace

### Step 3: Configure Connection in Kyomi

1. In the datasource modal, select **Databricks** as the datasource type
2. Enter the **Server Hostname** (e.g., `dbc-xxxxxxxx-xxxx.cloud.databricks.com`)
3. Enter the **HTTP Path** (e.g., `/sql/1.0/warehouses/xxxx`)
4. Click **Connect** to test the connection

### Step 4: Select Catalog and Schema

1. Choose your **Catalog** from the dropdown:
   - `hive_metastore` - Legacy Hive metastore
   - `main` or custom - Unity Catalog
2. Select a **Default Schema** (e.g., `default`)

### Step 5: Enter Credentials

**For PAT authentication:**
1. Paste your **Personal Access Token** in the credentials section
2. Click **Save**

**For OAuth authentication:**
1. Your credentials are stored automatically after OAuth authorization
2. Click **Save** to complete setup

### Step 6: Configure Catalog Indexing

Select which catalogs Kyomi should index:
- Tables and columns from these catalogs will appear in the catalog
- The AI will use this information to help write queries
- Leave empty to index all accessible catalogs

---

## Unity Catalog Structure

Databricks Unity Catalog uses a three-level namespace:

```
Catalog → Schema → Table
   │         │        │
   │         │        └── Individual tables/views
   │         └── Grouping of tables (like database schemas)
   └── Top-level container (like a database)
```

**Examples:**
- `main.sales.orders` - Table `orders` in schema `sales` in catalog `main`
- `hive_metastore.default.users` - Legacy Hive table

---

## Required Permissions

Your Databricks user needs:

```sql
-- Unity Catalog permissions
GRANT USE CATALOG ON CATALOG main TO `user@example.com`;
GRANT USE SCHEMA ON SCHEMA main.sales TO `user@example.com`;
GRANT SELECT ON TABLE main.sales.* TO `user@example.com`;

-- Or broader access
GRANT USE CATALOG ON CATALOG main TO `user@example.com`;
GRANT USE SCHEMA ON ALL SCHEMAS IN CATALOG main TO `user@example.com`;
GRANT SELECT ON ALL TABLES IN CATALOG main TO `user@example.com`;
```

For legacy Hive metastore:
```sql
GRANT SELECT ON DATABASE default TO `user@example.com`;
```

---

## Troubleshooting

### "Invalid access token" error (PAT)

- Verify the token is correct and not expired
- Generate a new token if needed
- Ensure the token has appropriate permissions

### OAuth connection failed

- Verify the redirect URI matches exactly: `https://your-domain/auth/oauth/databricks/callback`
- Check that `offline_access` scope is enabled (required for refresh tokens)
- Ensure your Databricks user has access to the workspace
- Try disconnecting and reconnecting OAuth

### "SQL warehouse not found" error

- Verify the HTTP path is correct
- Check that the SQL warehouse exists and is running
- Ensure you have access to the warehouse

### "Catalog not found" error

- Verify the catalog name is correct
- Check you have `USE CATALOG` permission
- For `hive_metastore`, ensure it's enabled in your workspace

### Slow queries

- Check if the SQL warehouse is starting up (serverless can have cold start)
- Consider using a Pro warehouse for consistent performance
- Review query efficiency and table partitioning

### Can't see expected tables

- Verify permissions on the catalog/schema/table
- Check if using Unity Catalog vs legacy Hive metastore
- Ensure "Catalogs to Index" includes the desired catalogs

---

## Best Practices

### Token Management

- Use service principals for production (not personal tokens)
- Set appropriate token expiration
- Rotate tokens periodically
- Store tokens securely

### Performance

- Use Delta Lake tables for best performance
- Partition large tables appropriately
- Use photon-enabled warehouses when available
- Consider caching for frequently accessed data

### Cost Management

- Use serverless for variable workloads
- Set auto-stop for SQL warehouses
- Monitor query costs in the Databricks console

---

## Additional Resources

- [Databricks SQL Documentation](https://docs.databricks.com/sql/)
- [Unity Catalog](https://docs.databricks.com/data-governance/unity-catalog/)
- [Personal Access Tokens](https://docs.databricks.com/dev-tools/auth.html#personal-access-tokens)
- [SQL Warehouse Connection](https://docs.databricks.com/integrations/jdbc-odbc-bi.html)

---

[← Back to Datasources](/docs/datasources/) | [Back to Docs](/docs/)
