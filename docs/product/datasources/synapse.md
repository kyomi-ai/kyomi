# Connecting to Azure Synapse Analytics

Connect Kyomi to Azure Synapse Analytics (dedicated or serverless SQL pools) for AI-powered analytics.

::: info Want to keep credentials on your infrastructure?
Use [Kyomi Connect](/docs/connect/) — deploy a lightweight agent inside your network. Queries execute locally, only results are sent to Kyomi. Database credentials never leave your infrastructure.
:::

## Connection Details

| Field | Description | Default |
|-------|-------------|---------|
| **Host** | Synapse workspace endpoint | Required |
| **Port** | SQL endpoint port | `1433` |
| **Database** | SQL pool or database name | Required |
| **Default Schema** | Default schema for queries | `dbo` |

---

## Prerequisites

- Azure Synapse Analytics workspace
- SQL pool (dedicated or serverless)
- User credentials with appropriate permissions

---

## Setup Steps

### Step 1: Get Workspace Endpoint

1. Open the Azure portal
2. Navigate to your Synapse workspace
3. Find the **SQL endpoint**:
   - Dedicated SQL pool: `workspacename.sql.azuresynapse.net`
   - Serverless SQL pool: `workspacename-ondemand.sql.azuresynapse.net`

### Step 2: Configure Connection in Kyomi

1. In the datasource modal, select **Azure Synapse** as the datasource type
2. Enter your **Host** (the SQL endpoint)
3. Enter the **Port** (default: `1433`)
4. Click **Connect** to test the connection

### Step 3: Select Database and Schema

1. Choose your **Database** from the dropdown:
   - For dedicated pools: Your SQL pool name
   - For serverless: `master` or a database you've created
2. Select a **Default Schema** (usually `dbo`)

### Step 4: Choose Authentication Method

Kyomi supports four authentication methods for Azure Synapse:

| Method | Best For | Setup |
|--------|----------|-------|
| **SQL Authentication** | Quick setup, dev/test | Username & password |
| **Service Principal** | Production, automated access | Azure AD app registration |
| **Microsoft Account** | Individual users | Sign in with Microsoft |
| **Enterprise OAuth** | Organizations with Azure AD | Your Azure AD app config |

Select your authentication method in the dropdown and enter the required credentials.

::: info Shared vs Personal Credentials
For SQL Authentication and Service Principal, admins can configure **Shared Credentials** that all workspace users share, or require **Personal Credentials** where each user provides their own.
:::

### Step 5: Configure Catalog

Select which schemas Kyomi should index:
- Tables and columns from these schemas will appear in the catalog
- The AI will use this information to help write queries
- Leave empty to index all accessible schemas

---

## Authentication Methods

### SQL Authentication

Standard username/password authentication. Best for quick setup and development.

**Required credentials:**
- **Username**: Your SQL login username
- **Password**: Your SQL login password

```sql
-- Create SQL login (in master database)
CREATE LOGIN kyomi_user WITH PASSWORD = 'SecurePassword123!';

-- Create user in your database
USE your_database;
CREATE USER kyomi_user FROM LOGIN kyomi_user;

-- Grant permissions
GRANT SELECT ON SCHEMA::dbo TO kyomi_user;
```

### Service Principal

Azure AD service principal authentication. Best for production and automated access.

**Required credentials:**
- **Tenant ID**: Your Azure AD tenant ID
- **Client ID**: The application (client) ID of your Azure AD app registration
- **Client Secret**: A client secret for the app registration

**Setup steps:**

1. Create an Azure AD App Registration in the Azure portal
2. Create a client secret for the app
3. Grant the app access to your Synapse workspace:
   ```sql
   -- In master database
   CREATE LOGIN [your-app-name] FROM EXTERNAL PROVIDER;

   -- In your database
   CREATE USER [your-app-name] FROM EXTERNAL PROVIDER;
   GRANT SELECT ON SCHEMA::dbo TO [your-app-name];
   ```

### Microsoft Account (OAuth)

Sign in with your personal or work Microsoft account. Uses Kyomi's multi-tenant Azure AD app.

**How it works:**
1. Click **Connect with Microsoft**
2. Sign in with your Microsoft account
3. Authorize Kyomi to access Azure Synapse on your behalf

::: tip Best for individual users
Each user signs in with their own Microsoft account, providing individual audit trails and access control.
:::

### Enterprise OAuth

Use your organization's Azure AD OAuth configuration. Best for enterprises with existing Azure AD apps.

**Required admin configuration:**
- **OAuth Client ID**: Your Azure AD application ID
- **OAuth Client Secret**: Your application's client secret
- **OAuth Tenant ID**: Your Azure AD tenant ID

**Setup steps:**

1. Create an Azure AD App Registration
2. Configure API permissions for Azure SQL Database
3. Add redirect URI: `https://app.kyomi.ai/auth/oauth/microsoft/callback`
4. Create a client secret
5. Configure the OAuth settings in Kyomi's datasource admin panel

::: info Enterprise Control
Enterprise OAuth gives your IT team full control over which users can access Synapse through Kyomi, using your existing Azure AD policies.
:::

---

## Dedicated vs Serverless SQL Pools

### Dedicated SQL Pools

- Pre-provisioned compute resources
- Best for large, predictable workloads
- Data stored in dedicated storage
- Pay for provisioned capacity

**Endpoint**: `workspacename.sql.azuresynapse.net`

### Serverless SQL Pools

- On-demand compute
- Query data in Azure Data Lake directly
- Pay per TB processed
- Great for exploration and ad-hoc queries

**Endpoint**: `workspacename-ondemand.sql.azuresynapse.net`

::: info Database Selection
For serverless pools, you may need to create a database first:
```sql
CREATE DATABASE analytics;
```
:::

---

## Required Permissions

### For Dedicated SQL Pools

```sql
-- Create user
CREATE USER kyomi_user FROM LOGIN kyomi_user;

-- Grant read access
ALTER ROLE db_datareader ADD MEMBER kyomi_user;

-- Or more granular permissions
GRANT SELECT ON SCHEMA::dbo TO kyomi_user;
GRANT SELECT ON SCHEMA::staging TO kyomi_user;
```

### For Serverless SQL Pools

```sql
-- Grant access to external data
GRANT REFERENCES ON DATABASE SCOPED CREDENTIAL::your_credential TO kyomi_user;

-- Grant access to schemas
GRANT SELECT ON SCHEMA::dbo TO kyomi_user;
```

---

## Troubleshooting

### "Cannot connect to server" error

- Verify the endpoint is correct
- Check Azure firewall allows Kyomi's IP addresses
- Ensure the SQL pool is running (dedicated pools can be paused)

### "Login failed" error

- Verify username and password
- For Azure AD: Check service principal credentials
- Ensure the login exists and user is mapped to the database

### "Database not found" error

- Verify the database/SQL pool name is correct
- For serverless: The database must be created first

### Slow query performance

- **Dedicated pools**: Check distribution and partitioning
- **Serverless pools**: Query performance depends on data format (Parquet is fastest)
- Review query patterns and consider materialized views

### "Permission denied" errors

- Verify user has appropriate role membership
- Check schema-level permissions
- For external data in serverless: Check credential permissions

---

## Firewall Configuration

### Azure Synapse Firewall

1. Go to your Synapse workspace in Azure portal
2. Navigate to **Networking** → **Firewalls**
3. Add Kyomi's IP addresses to the allow list
4. Or enable "Allow Azure services" if using Azure-hosted Kyomi

### Private Endpoints

For private connectivity:
1. Create a private endpoint for the SQL pool
2. Configure Kyomi to connect via the private endpoint
3. Use SSH tunnel through a VM in your Azure VNet

---

## Performance Tips

### Dedicated SQL Pools

- Use proper distribution (HASH, ROUND_ROBIN, REPLICATE)
- Partition large fact tables
- Use columnstore indexes for analytics
- Run `CREATE STATISTICS` for query optimization

### Serverless SQL Pools

- Use Parquet format for best performance
- Partition data by common filter columns
- Use wildcards for querying multiple files
- Consider creating views over external data

---

## Additional Resources

- [Azure Synapse Documentation](https://docs.microsoft.com/en-us/azure/synapse-analytics/)
- [Synapse SQL Best Practices](https://docs.microsoft.com/en-us/azure/synapse-analytics/sql/best-practices-dedicated-sql-pool)
- [Serverless SQL Pool](https://docs.microsoft.com/en-us/azure/synapse-analytics/sql/on-demand-workspace-overview)

---

[← Back to Datasources](/docs/datasources/) | [Back to Docs](/docs/)
