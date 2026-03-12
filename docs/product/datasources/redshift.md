# Connecting to Amazon Redshift

Connect Kyomi to your Amazon Redshift data warehouse for AI-powered analytics.

::: info Want to keep credentials on your infrastructure?
Use [Kyomi Connect](/docs/connect/) — deploy a lightweight agent inside your network. Queries execute locally, only results are sent to Kyomi. Database credentials never leave your infrastructure.
:::

## Connection Details

| Field | Description | Default |
|-------|-------------|---------|
| **Host** | Redshift cluster endpoint | Required |
| **Port** | Redshift port | `5439` |
| **Database** | Database to connect to | Required |
| **Default Schema** | Default schema for queries | `public` |

---

## Setup Steps

### Step 1: Get Cluster Endpoint

1. Open the Amazon Redshift console
2. Select your cluster
3. Find the **Endpoint** in the cluster details (e.g., `mycluster.xxxx.us-east-1.redshift.amazonaws.com`)

### Step 2: Configure Connection

1. In the datasource modal, select **Amazon Redshift** as the datasource type
2. Enter your cluster **Host** (the endpoint without the port)
3. Enter the **Port** (default: `5439`)
4. Click **Connect** to test the connection

### Step 3: Select Database and Schema

1. Choose your **Database** from the dropdown (typically `dev` or your custom database)
2. Select a **Default Schema** (usually `public`)

### Step 4: Configure Credentials

Enter your Redshift **Username** and **Password**.

::: info Shared vs Personal Credentials
**Shared Credentials**: Admin configures once, all workspace users share the same database access.

**Personal Credentials**: Each user provides their own username/password for individual access control and audit trails.
:::

### Step 5: Configure Catalog

Select which schemas Kyomi should index:
- Tables and columns from these schemas will appear in the catalog
- The AI will use this information to help write queries
- Leave empty to index all accessible schemas

---

## SSH Tunnel (Optional)

For Redshift clusters in a private VPC, Kyomi supports SSH tunnel connections.

### Configuration

1. Enable **Use SSH Tunnel** in the connection settings
2. Enter SSH connection details for a bastion host in your VPC:
   - **SSH Host**: Bastion server hostname
   - **SSH Port**: SSH port (default: `22`)
   - **SSH Username**: Your SSH username
3. The database connection will be tunneled through SSH

---

## Required Permissions

Create a read-only user for Kyomi:

```sql
-- Create user
CREATE USER kyomi_user PASSWORD 'SecurePassword123!';

-- Grant usage on schemas
GRANT USAGE ON SCHEMA public TO kyomi_user;
GRANT USAGE ON SCHEMA analytics TO kyomi_user;

-- Grant read access to all tables in schemas
GRANT SELECT ON ALL TABLES IN SCHEMA public TO kyomi_user;
GRANT SELECT ON ALL TABLES IN SCHEMA analytics TO kyomi_user;

-- Grant on future tables
ALTER DEFAULT PRIVILEGES IN SCHEMA public
  GRANT SELECT ON TABLES TO kyomi_user;
ALTER DEFAULT PRIVILEGES IN SCHEMA analytics
  GRANT SELECT ON TABLES TO kyomi_user;
```

---

## Network Configuration

### VPC Security Group

Ensure your Redshift cluster's security group allows inbound connections:

1. Go to EC2 → Security Groups
2. Find the security group attached to your Redshift cluster
3. Add an inbound rule:
   - **Type**: Custom TCP
   - **Port**: 5439
   - **Source**: Kyomi's IP addresses (or your VPC CIDR for SSH tunnel)

### Publicly Accessible

For direct connections (no SSH tunnel):
1. Ensure your cluster is **Publicly Accessible**
2. Or use VPC peering / PrivateLink

---

## Troubleshooting

### "Connection refused" or "Connection timed out"

- Verify the endpoint and port are correct
- Check security group allows inbound connections on port 5439
- Verify the cluster is running (not paused)
- Check if cluster is publicly accessible (or use SSH tunnel)

### "Invalid username or password"

- Verify credentials are correct
- Note: Redshift usernames are case-sensitive

### "Permission denied" when querying

- Verify the user has `SELECT` privilege on the tables
- Check schema permissions (`USAGE` on schema)

### Can't see expected tables

- Verify the user has access to the schema
- Check "Schemas to Index" includes the desired schemas
- External schemas (Spectrum) may require additional permissions

---

## Redshift Serverless

For Redshift Serverless:

1. Find the **Workgroup endpoint** in the Redshift Serverless console
2. Use the endpoint as the **Host**
3. Port is still `5439`
4. Authentication works the same as provisioned clusters

---

## Redshift Spectrum

If you use Redshift Spectrum for querying S3 data:

- External schemas will appear in the catalog if indexed
- Ensure the user has access to the external schema
- Spectrum queries may take longer due to S3 access

```sql
-- Grant access to external schema
GRANT USAGE ON SCHEMA spectrum_schema TO kyomi_user;
GRANT SELECT ON ALL TABLES IN SCHEMA spectrum_schema TO kyomi_user;
```

---

## Performance Tips

- **Distribution keys**: Queries joining on distribution keys are faster
- **Sort keys**: Filtering on sort keys improves performance
- **VACUUM/ANALYZE**: Run regularly for optimal query planning
- **Concurrency scaling**: Enable for handling multiple concurrent queries

---

## Additional Resources

- [Amazon Redshift Documentation](https://docs.aws.amazon.com/redshift/)
- [Redshift Best Practices](https://docs.aws.amazon.com/redshift/latest/dg/best-practices.html)
- [Managing Database Security](https://docs.aws.amazon.com/redshift/latest/dg/r_Database_objects.html)

---

[← Back to Datasources](/docs/datasources/) | [Back to Docs](/docs/)
