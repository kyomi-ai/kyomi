# Connecting to SQL Server

Connect Kyomi to Microsoft SQL Server or Azure SQL Database for AI-powered analytics.

::: info Want to keep credentials on your infrastructure?
Use [Kyomi Connect](/docs/connect/) — deploy a lightweight agent inside your network. Queries execute locally, only results are sent to Kyomi. Database credentials never leave your infrastructure.
:::

## Connection Details

| Field | Description | Default |
|-------|-------------|---------|
| **Host** | Server hostname or IP | Required |
| **Port** | SQL Server port | `1433` |
| **Encrypt** | Use TLS encryption | `true` |
| **Trust Server Certificate** | Trust self-signed certs | `false` |
| **Database** | Database to connect to | Required |
| **Default Schema** | Default schema for queries | `dbo` |

---

## Setup Steps

### Step 1: Configure Connection

1. In the datasource modal, select **SQL Server** as the datasource type
2. Enter your server **Host** (e.g., `sqlserver.example.com`)
3. Enter the **Port** (default: `1433`)
4. Configure encryption options:
   - **Encrypt**: Enable TLS encryption (recommended)
   - **Trust Server Certificate**: Enable if using self-signed certificates
5. Click **Connect** to test the connection

### Step 2: Select Database and Schema

1. Choose your **Database** from the dropdown
2. Select a **Default Schema** (usually `dbo`)

### Step 3: Configure Credentials

Enter your SQL Server **Username** and **Password**.

::: info Authentication Types
Kyomi uses SQL Server Authentication. Windows Authentication is not currently supported.
:::

::: info Shared vs Personal Credentials
**Shared Credentials**: Admin configures once, all workspace users share the same database access.

**Personal Credentials**: Each user provides their own username/password for individual access control.
:::

### Step 4: Configure Catalog

Select which schemas Kyomi should index:
- Tables and columns from these schemas will appear in the catalog
- The AI will use this information to help write queries
- Leave empty to index all accessible schemas

---

## SSH Tunnel (Optional)

For SQL Server behind a firewall, Kyomi supports SSH tunnel connections.

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
-- Create login at server level
CREATE LOGIN kyomi_user WITH PASSWORD = 'SecurePassword123!';

-- Create user in target database
USE your_database;
CREATE USER kyomi_user FOR LOGIN kyomi_user;

-- Grant read access to schema
GRANT SELECT ON SCHEMA::dbo TO kyomi_user;
GRANT SELECT ON SCHEMA::analytics TO kyomi_user;

-- For catalog indexing
GRANT VIEW DEFINITION ON SCHEMA::dbo TO kyomi_user;
```

For broader read access:

```sql
-- Add to db_datareader role (all tables)
ALTER ROLE db_datareader ADD MEMBER kyomi_user;
```

---

## Troubleshooting

### "Cannot connect to server" error

- Verify the host and port are correct
- Check that SQL Server is accepting remote connections
- Ensure TCP/IP protocol is enabled in SQL Server Configuration Manager
- Verify firewall allows connections on port 1433

### "Login failed for user" error

- Verify username and password are correct
- Check SQL Server Authentication is enabled (mixed mode)
- Verify the login exists at server level

### "Certificate chain was issued by an authority that is not trusted"

- Enable **Trust Server Certificate** for self-signed certificates
- Or install the server's certificate in your trust store

### "Cannot open database" error

- Verify the database exists
- Check the user has access to the database

---

## Azure SQL Database

For Azure SQL Database:

1. Get the server name from Azure portal (e.g., `myserver.database.windows.net`)
2. Use port `1433`
3. Enable **Encrypt** (required by Azure)
4. May need to **Trust Server Certificate** depending on your setup
5. Configure Azure firewall to allow Kyomi's IP addresses

### Connection String Format

Azure SQL uses this format:
```
Server=myserver.database.windows.net,1433
Database=mydb
User Id=myuser
Password=mypassword
Encrypt=True
```

---

## Named Instances

For SQL Server named instances:

- **Default instance**: Use just the hostname (e.g., `sqlserver.example.com`)
- **Named instance**: Use `hostname\instancename` format
- Port may differ from 1433 - check SQL Server Configuration Manager

---

## SQL Server Versions

Kyomi supports:
- SQL Server 2012 and later (for OFFSET-FETCH pagination)
- Azure SQL Database
- Azure SQL Managed Instance

---

## Additional Resources

- [SQL Server Documentation](https://docs.microsoft.com/en-us/sql/sql-server/)
- [Azure SQL Documentation](https://docs.microsoft.com/en-us/azure/azure-sql/)
- [SQL Server Security Best Practices](https://docs.microsoft.com/en-us/sql/relational-databases/security/)

---

[← Back to Datasources](/docs/datasources/) | [Back to Docs](/docs/)
