# Connecting to PostgreSQL

Connect Kyomi to your PostgreSQL database for AI-powered analytics.

::: info Want to keep credentials on your infrastructure?
Use [Kyomi Connect](/docs/connect/) — deploy a lightweight agent inside your network. Queries execute locally, only results are sent to Kyomi. Database credentials never leave your infrastructure.
:::

## Connection Details

| Field | Description | Default |
|-------|-------------|---------|
| **Host** | Database server hostname or IP | Required |
| **Port** | PostgreSQL port number | `5432` |
| **SSL Mode** | Connection security level | `require` |
| **Database** | Database name to connect to | Required |
| **Default Schema** | Default schema for queries | `public` |

---

## Setup Steps

### Step 1: Configure Connection

1. In the datasource modal, select **PostgreSQL** as the datasource type
2. Enter your database **Host** (e.g., `db.example.com`)
3. Enter the **Port** (default: `5432`)
4. Select **SSL Mode**:
   - `Disable` - No encryption (not recommended)
   - `Require` - Encrypted connection (recommended)
   - `Verify-CA` - Verify server certificate against CA
   - `Verify-Full` - Verify certificate and hostname
5. Click **Connect** to test the connection

### Step 2: Select Database and Schema

1. Choose your **Database** from the dropdown
2. Optionally select a **Default Schema** (usually `public`)

### Step 3: Configure Credentials

Enter your PostgreSQL **Username** and **Password**.

::: info Shared vs Personal Credentials
**Shared Credentials**: Admin configures once, all workspace users share the same database access.

**Personal Credentials**: Each user provides their own username/password for individual access control and audit trails.
:::

### Step 4: Configure Catalog

Select which schemas Kyomi should index:
- Tables and columns from these schemas will appear in the catalog
- The AI will use this information to help write queries
- Leave empty to index all accessible schemas

---

## SSH Tunnel (Optional)

For databases behind a firewall or in a private network, Kyomi supports SSH tunnel connections.

### Prerequisites

- SSH access to a bastion/jump server that can reach your database
- SSH key or password authentication

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

Your PostgreSQL user needs the following privileges:

```sql
-- Connect to database
GRANT CONNECT ON DATABASE your_database TO kyomi_user;

-- Read schemas
GRANT USAGE ON SCHEMA public TO kyomi_user;

-- Read tables
GRANT SELECT ON ALL TABLES IN SCHEMA public TO kyomi_user;

-- For catalog indexing (optional but recommended)
GRANT SELECT ON information_schema.tables TO kyomi_user;
GRANT SELECT ON information_schema.columns TO kyomi_user;
```

For a read-only analytics user, this is typically sufficient:

```sql
CREATE USER kyomi_user WITH PASSWORD 'secure_password';
GRANT CONNECT ON DATABASE analytics TO kyomi_user;
GRANT USAGE ON SCHEMA public, analytics TO kyomi_user;
GRANT SELECT ON ALL TABLES IN SCHEMA public, analytics TO kyomi_user;

-- Grant on future tables
ALTER DEFAULT PRIVILEGES IN SCHEMA public, analytics
  GRANT SELECT ON TABLES TO kyomi_user;
```

---

## Troubleshooting

### "Connection refused" error

- Verify the host and port are correct
- Check that PostgreSQL is accepting remote connections (`listen_addresses` in `postgresql.conf`)
- Ensure firewall rules allow the connection

### "SSL connection required" error

- Your server requires SSL - set SSL Mode to `Require` or higher
- If using self-signed certificates, you may need `Verify-CA` with the CA certificate

### "Password authentication failed" error

- Verify username and password are correct
- Check `pg_hba.conf` allows your connection method and host

### "Permission denied" errors when querying

- Verify the user has `SELECT` privilege on the tables
- Check schema permissions (`USAGE` on schema)

### Can't see expected tables in catalog

- Ensure the user has `SELECT` privilege on `information_schema.tables`
- Verify the schemas are included in "Schemas to Index"

---

## Cloud PostgreSQL Services

### Amazon RDS

- Use the RDS endpoint as the host (e.g., `mydb.xxxx.region.rds.amazonaws.com`)
- Ensure the security group allows inbound connections from Kyomi's IP
- SSL is typically required - use `Require` SSL mode

### Google Cloud SQL

- Use the Cloud SQL public IP or private IP as the host
- Configure authorized networks to allow Kyomi's IP
- Enable SSL and use `Require` or `Verify-CA` mode

### Azure Database for PostgreSQL

- Use the server name as host (e.g., `myserver.postgres.database.azure.com`)
- Username format: `user@servername`
- SSL is enforced by default

### Heroku Postgres

- Find connection details in Heroku dashboard
- SSL is required - use `Require` mode
- Connection credentials may rotate - use Heroku's credential management

---

## Additional Resources

- [PostgreSQL Documentation](https://www.postgresql.org/docs/)
- [PostgreSQL Security Best Practices](https://www.postgresql.org/docs/current/auth-pg-hba-conf.html)

---

[← Back to Datasources](/docs/datasources/) | [Back to Docs](/docs/)
