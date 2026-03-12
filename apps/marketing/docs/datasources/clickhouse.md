# Connecting to ClickHouse

Connect Kyomi to your ClickHouse analytics database for AI-powered queries.

::: info Want to keep credentials on your infrastructure?
Use [Kyomi Connect](/docs/connect/) — deploy a lightweight agent inside your network. Queries execute locally, only results are sent to Kyomi. Database credentials never leave your infrastructure.
:::

## Connection Details

| Field | Description | Default |
|-------|-------------|---------|
| **Host** | ClickHouse server hostname or IP | Required |
| **Port** | HTTP interface port | `8123` |
| **Secure (HTTPS)** | Use encrypted connection | `false` |
| **Default Database** | Database to use for queries | Required |

---

## Setup Steps

### Step 1: Configure Connection

1. In the datasource modal, select **ClickHouse** as the datasource type
2. Enter your server **Host** (e.g., `clickhouse.example.com`)
3. Enter the **Port**:
   - `8123` for HTTP (default)
   - `8443` for HTTPS
4. Enable **Secure (HTTPS)** if using encrypted connections
5. Click **Connect** to test the connection

### Step 2: Select Database

Choose your **Default Database** from the dropdown.

### Step 3: Configure Credentials

Enter your ClickHouse **Username** and **Password**.

::: info Default User
ClickHouse ships with a `default` user that may have no password. For production, always create dedicated users with strong passwords.
:::

::: info Shared vs Personal Credentials
**Shared Credentials**: Admin configures once, all workspace users share the same database access.

**Personal Credentials**: Each user provides their own username/password for individual access control.
:::

### Step 4: Configure Catalog

Select which databases Kyomi should index:
- Tables and columns from these databases will appear in the catalog
- The AI will use this information to help write queries
- Leave empty to index all accessible databases

---

## SSH Tunnel (Optional)

For ClickHouse servers behind a firewall, Kyomi supports SSH tunnel connections.

### Configuration

1. Enable **Use SSH Tunnel** in the connection settings
2. Enter SSH connection details:
   - **SSH Host**: Bastion server hostname
   - **SSH Port**: SSH port (default: `22`)
   - **SSH Username**: Your SSH username
3. The database connection will be tunneled through SSH

---

## Required Permissions

Create a read-only user for Kyomi:

```sql
-- Create user with password
CREATE USER kyomi_user IDENTIFIED BY 'secure_password';

-- Grant read access to specific databases
GRANT SELECT ON analytics.* TO kyomi_user;
GRANT SELECT ON events.* TO kyomi_user;

-- For catalog indexing (system tables)
GRANT SELECT ON system.tables TO kyomi_user;
GRANT SELECT ON system.columns TO kyomi_user;
GRANT SELECT ON system.databases TO kyomi_user;
```

For broader read access:

```sql
-- Grant read access to all databases
GRANT SELECT ON *.* TO kyomi_user;
```

---

## Troubleshooting

### "Connection refused" error

- Verify the host and port are correct
- Check that the HTTP interface is enabled (`http_port` in config)
- Ensure firewall rules allow the connection
- For HTTPS, verify port `8443` and enable "Secure"

### "Authentication failed" error

- Verify username and password are correct
- Check the user exists: `SELECT * FROM system.users WHERE name = 'your_user'`

### "Code: 497. DB::Exception: default: Not enough privileges"

- The user lacks necessary permissions
- Grant `SELECT` privilege on the required databases/tables

### Slow catalog indexing

- ClickHouse can have many system tables
- Consider limiting "Databases to Index" to only needed databases

---

## ClickHouse Cloud

For ClickHouse Cloud:

1. Get your connection details from the ClickHouse Cloud console
2. Use the provided hostname (e.g., `xxx.clickhouse.cloud`)
3. Port is typically `8443` (HTTPS)
4. Enable **Secure (HTTPS)**
5. Use your ClickHouse Cloud credentials

---

## Performance Notes

ClickHouse is optimized for:
- Large analytical queries (OLAP)
- Columnar storage with high compression
- Fast aggregations over billions of rows

Tips for best performance:
- Use appropriate `ORDER BY` in your table definitions
- Leverage ClickHouse-specific functions (`toStartOfMonth`, `dictGet`, etc.)
- Avoid `SELECT *` on wide tables - select only needed columns

---

## Additional Resources

- [ClickHouse Documentation](https://clickhouse.com/docs)
- [ClickHouse SQL Reference](https://clickhouse.com/docs/en/sql-reference)
- [Access Rights Management](https://clickhouse.com/docs/en/operations/access-rights)

---

[← Back to Datasources](/docs/datasources/) | [Back to Docs](/docs/)
