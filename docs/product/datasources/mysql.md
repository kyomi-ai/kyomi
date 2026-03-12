# Connecting to MySQL

Connect Kyomi to your MySQL or MariaDB database for AI-powered analytics.

::: info Want to keep credentials on your infrastructure?
Use [Kyomi Connect](/docs/connect/) — deploy a lightweight agent inside your network. Queries execute locally, only results are sent to Kyomi. Database credentials never leave your infrastructure.
:::

## Connection Details

| Field | Description | Default |
|-------|-------------|---------|
| **Host** | Database server hostname or IP | Required |
| **Port** | MySQL port number | `3306` |
| **SSL Mode** | Connection security level | `require` |
| **Default Database** | Database to use for queries | Required |

---

## Setup Steps

### Step 1: Configure Connection

1. In the datasource modal, select **MySQL** as the datasource type
2. Enter your database **Host** (e.g., `mysql.example.com`)
3. Enter the **Port** (default: `3306`)
4. Select **SSL Mode**:
   - `Disable` - No encryption (not recommended)
   - `Require` - Encrypted connection (recommended)
   - `Verify-CA` - Verify server certificate against CA
   - `Verify-Full` - Verify certificate and hostname match
5. Click **Connect** to test the connection

### Step 2: Select Database

Choose your **Default Database** from the dropdown.

::: info MySQL vs PostgreSQL
MySQL uses "databases" where PostgreSQL uses "schemas". In MySQL, each database is a separate namespace containing tables.
:::

### Step 3: Configure Credentials

Enter your MySQL **Username** and **Password**.

::: info Shared vs Personal Credentials
**Shared Credentials**: Admin configures once, all workspace users share the same database access.

**Personal Credentials**: Each user provides their own username/password for individual access control and audit trails.
:::

### Step 4: Configure Catalog

Select which databases Kyomi should index:
- Tables and columns from these databases will appear in the catalog
- The AI will use this information to help write queries
- Leave empty to index all accessible databases

---

## SSH Tunnel (Optional)

For databases behind a firewall or in a private network, Kyomi supports SSH tunnel connections.

### Configuration

1. Enable **Use SSH Tunnel** in the connection settings
2. Enter SSH connection details:
   - **SSH Host**: Bastion server hostname
   - **SSH Port**: SSH port (default: `22`)
   - **SSH Username**: Your SSH username
   - **SSH Authentication**: Key or password
3. The database connection will be tunneled through SSH

---

## Required Permissions

Your MySQL user needs the following privileges:

```sql
-- Create a read-only analytics user
CREATE USER 'kyomi_user'@'%' IDENTIFIED BY 'secure_password';

-- Grant read access to specific databases
GRANT SELECT ON analytics.* TO 'kyomi_user'@'%';
GRANT SELECT ON sales.* TO 'kyomi_user'@'%';

-- For catalog indexing (shows table/column metadata)
GRANT SELECT ON information_schema.* TO 'kyomi_user'@'%';

-- Apply changes
FLUSH PRIVILEGES;
```

For broader read access:

```sql
-- Grant read access to all databases (careful!)
GRANT SELECT ON *.* TO 'kyomi_user'@'%';
```

---

## Troubleshooting

### "Can't connect to MySQL server" error

- Verify the host and port are correct
- Check that MySQL is accepting remote connections (`bind-address` in `my.cnf`)
- Ensure firewall rules allow the connection

### "Access denied for user" error

- Verify username and password are correct
- Check the user is allowed to connect from your host (`'user'@'%'` vs `'user'@'localhost'`)
- Verify the user has necessary privileges

### SSL connection issues

- If SSL is required, ensure SSL Mode is set to `Require` or higher
- For self-signed certificates, you may need to configure CA certificates

### Can't see expected tables in catalog

- Verify the user has `SELECT` privilege on `information_schema`
- Check that databases are included in "Databases to Index"

---

## Cloud MySQL Services

### Amazon RDS for MySQL

- Use the RDS endpoint as the host
- Ensure security group allows inbound connections from Kyomi's IP
- SSL is typically available - use `Require` mode for production

### Google Cloud SQL for MySQL

- Use the public or private IP as the host
- Configure authorized networks to allow Kyomi's IP
- SSL can be enforced at the instance level

### Azure Database for MySQL

- Use the server name as host (e.g., `myserver.mysql.database.azure.com`)
- Username format: `user@servername`
- SSL is enforced by default

### PlanetScale

- Use the provided host from PlanetScale dashboard
- Requires SSL - use `Require` mode
- Use branch-specific credentials for development vs production

---

## MariaDB Compatibility

Kyomi works with MariaDB as it's MySQL-compatible:
- Connection settings are identical
- SQL syntax is compatible for most queries
- Some advanced features may differ

---

## Additional Resources

- [MySQL Documentation](https://dev.mysql.com/doc/)
- [MySQL User Account Management](https://dev.mysql.com/doc/refman/8.0/en/access-control.html)

---

[← Back to Datasources](/docs/datasources/) | [Back to Docs](/docs/)
