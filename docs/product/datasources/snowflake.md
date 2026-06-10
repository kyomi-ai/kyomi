# Connecting to Snowflake

Kyomi supports two authentication methods for Snowflake. Choose the one that best fits your organization's security requirements.

## Which authentication method should I use?

| Method | Best For | Setup Complexity |
|--------|----------|------------------|
| **Username/Password** | Quick setup, individual access | Easy |
| **OAuth** | Enterprise SSO, per-user audit trails | Advanced |

---

## Connection Details

Before configuring authentication, you'll need your Snowflake account identifier.

### Account Identifier

Your account identifier is found in your Snowflake URL:
- **Format**: `<account_identifier>` (e.g., `xy12345.us-east-1` or `myorg-myaccount`)
- **Where to find it**: Look at your Snowflake URL - `https://xy12345.us-east-1.snowflakecomputing.com`
- The account identifier is the part before `.snowflakecomputing.com`

### Optional Settings

After connecting, you can configure:
- **Warehouse**: Compute warehouse for running queries
- **Default Database**: Database to use when not specified in queries
- **Default Schema**: Schema to use (usually `PUBLIC`)
- **Role**: Snowflake role for access control

---

## Method 1: Username/Password

The simplest way to connect using your Snowflake username and password.

### Prerequisites

- Snowflake user account with appropriate permissions
- Password authentication enabled for your account

### Setup Steps

1. In the datasource modal, select **Snowflake** as the datasource type
2. Enter your **Account Identifier** (e.g., `xy12345.us-east-1`)
3. Click **Connect** to test the connection
4. Select your **Warehouse** from the dropdown
5. Optionally select a **Default Database** and **Schema**
6. Enter your Snowflake **Username** and **Password**
7. Click **Save**

### Required Permissions

Your Snowflake user needs:
- `USAGE` privilege on the warehouse
- `USAGE` privilege on databases and schemas you want to query
- `SELECT` privilege on tables you want to read

---

## Method 2: OAuth (Enterprise)

Configure OAuth for enterprise SSO integration. Users authenticate with their corporate credentials.

### Prerequisites

- Snowflake account with OAuth configured
- OAuth Security Integration created in Snowflake
- Access to create OAuth clients

### Step 1: Create Security Integration

In Snowflake, create an OAuth integration:

```sql
CREATE OR REPLACE SECURITY INTEGRATION kyomi_oauth
  TYPE = OAUTH
  ENABLED = TRUE
  OAUTH_CLIENT = CUSTOM
  OAUTH_CLIENT_TYPE = 'CONFIDENTIAL'
  OAUTH_REDIRECT_URI = 'https://app.kyomi.ai/auth/oauth/snowflake/callback'
  OAUTH_ISSUE_REFRESH_TOKENS = TRUE
  OAUTH_REFRESH_TOKEN_VALIDITY = 86400;
```

### Step 2: Get Client Credentials

```sql
-- Get the client ID and secret
SELECT SYSTEM$SHOW_OAUTH_CLIENT_SECRETS('KYOMI_OAUTH');
```

### Step 3: Configure in Kyomi

1. In the datasource modal, select **Snowflake**
2. Enter your **Account Identifier**
3. Click **Connect**
4. Select your **Warehouse**, **Database**, and **Schema**
5. Choose **OAuth** as the authentication method
6. As an admin, enter the **OAuth Client ID** and **Client Secret**
7. Click **Save**
8. Each user clicks **Connect with Snowflake** to authenticate

---

## Catalog Indexing

Configure which databases Kyomi should index for AI-assisted queries.

### Databases to Index

Select the databases you want Kyomi to index:
- Tables and columns from these databases will appear in the catalog
- The AI will use this information to help write queries
- Leave empty to index all accessible databases

---

## Troubleshooting

### "Invalid account identifier" error

- Ensure you're using the correct format (e.g., `xy12345.us-east-1`)
- Check if your organization uses a custom account name

### "Warehouse not found" error

- Verify the warehouse exists and is running
- Check that your user has `USAGE` privilege on the warehouse

### "Authentication failed" error

- For password auth: Verify username and password are correct
- For OAuth: Check that the security integration is enabled and credentials are correct

### Can't see expected databases/schemas

- Verify your user/role has `USAGE` privilege on the databases
- Check role hierarchy if using a specific role

---

## Additional Resources

- [Snowflake Account Identifiers](https://docs.snowflake.com/en/user-guide/admin-account-identifier)
- [OAuth for Custom Clients](https://docs.snowflake.com/en/user-guide/oauth-custom)

---

[← Back to Datasources](/docs/datasources/) | [Back to Docs](/docs/)
