# Datasource Reference

This document provides a reference for all supported datasource types, their authentication modes, and connection configurations.

**Last Updated:** January 2026

---

## Quick Reference Table

| Datasource | Auth Modes | Default Auth | Default Port |
|------------|-----------|--------------|--------------|
| BigQuery | Google OAuth (Kyomi), Enterprise OAuth, Service Account | Google OAuth (Kyomi) | N/A (API) |
| Snowflake | Password, Snowflake OAuth | Password | 443 |
| Azure Synapse | SQL Auth, Service Principal, Microsoft OAuth, Enterprise OAuth | SQL Auth | 1433 |
| PostgreSQL | Password | Password | 5432 |
| ClickHouse | Password | Password | 8443 |
| Databricks | Personal Access Token, Databricks OAuth | Personal Access Token | 443 |
| SQL Server | Password | Password | 1433 |
| MySQL | Password | Password | 3306 |
| Redshift | Password | Password | 5439 |
| DuckDB | None (local file) | None | N/A |

---

## BigQuery

### Authentication Modes

#### 1. Google OAuth (Kyomi) - Default
- **Mode ID:** `kyomi_oauth`
- **Description:** Sign in with your Google account using Kyomi's OAuth app
- **How it works:** Users authenticate via Google OAuth popup. Tokens stored in user profile and shared across all BigQuery datasources.
- **Best for:** Individual users, small teams

#### 2. Enterprise OAuth
- **Mode ID:** `enterprise_oauth`
- **Description:** Use your organization's Google OAuth configuration
- **How it works:** Workspace admin configures OAuth client ID/secret. Each user authenticates and gets their own tokens stored per-datasource.
- **Required admin config:** `oauth_client_id`, `oauth_client_secret`
- **Best for:** Organizations with their own Google Cloud projects

#### 3. Service Account
- **Mode ID:** `service_account`
- **Description:** Use a service account for server-side authentication
- **How it works:** Admin uploads service account JSON. All workspace users share these credentials.
- **Required admin config:** `service_account_json`
- **Best for:** Automated workflows, server-side access

### Connection Configuration

```yaml
# Required
project_id: "your-gcp-project"          # Default GCP project for queries

# Optional - Catalog Indexing
catalog_projects: ["project1", "project2"]  # Projects to index (empty = all)
include_public_datasets: false              # Include public datasets in catalog

# Optional - Enterprise OAuth
auth_mode: "enterprise_oauth"
oauth_client_id: "your-client-id"
oauth_client_secret: "your-secret"        # Sensitive - masked in API

# Optional - Service Account
auth_mode: "service_account"
service_account_json: "{...}"             # Sensitive - masked in API
```

---

## Snowflake

### Authentication Modes

#### 1. Password - Default
- **Mode ID:** `password`
- **Description:** Authenticate with database username and password
- **Supports shared credentials:** Yes (admin can configure for all users)

#### 2. Snowflake OAuth
- **Mode ID:** `oauth`
- **Description:** Authenticate with your Snowflake account via OAuth
- **How it works:** Users authenticate via Snowflake OAuth popup. Per-user tokens stored per-datasource.

### Connection Configuration

```yaml
# Required
account: "your-account"                   # Snowflake account identifier (e.g., xy12345.us-east-1)
warehouse: "COMPUTE_WH"                   # Default warehouse
database: "MY_DATABASE"                   # Default database

# Optional
role: "MY_ROLE"                           # Snowflake role to use
schema: "PUBLIC"                          # Default schema

# Optional - Catalog Indexing
catalog_schemas: ["SCHEMA1", "SCHEMA2"]   # Schemas to index (empty = all)
```

---

## Azure Synapse

### Authentication Modes

#### 1. SQL Authentication - Default
- **Mode ID:** `sql`
- **Description:** Authenticate with database username and password
- **Supports shared credentials:** Yes

#### 2. Service Principal
- **Mode ID:** `service_principal`
- **Description:** Authenticate using an Azure AD service principal
- **Required credentials:** `tenant_id`, `client_id`, `client_secret`
- **Note:** Requires ODBC Driver 18 for Azure AD token authentication

#### 3. Microsoft OAuth (Kyomi)
- **Mode ID:** `oauth`
- **Description:** Sign in with your Microsoft/Azure AD account
- **How it works:** Users authenticate via Microsoft OAuth popup using Kyomi's multi-tenant app

#### 4. Microsoft OAuth (Enterprise)
- **Mode ID:** `enterprise_oauth`
- **Description:** Use your organization's Azure AD OAuth configuration
- **How it works:** Admin configures Azure AD app credentials. Each user authenticates with their own tokens.
- **Required admin config:** `oauth_client_id`, `oauth_client_secret`, `oauth_tenant_id`

### Connection Configuration

```yaml
# Required
server: "your-workspace.sql.azuresynapse.net"   # Synapse SQL endpoint
database: "your_database"                        # Database name
port: 1433                                       # Default: 1433

# Optional - Enterprise OAuth
auth_mode: "enterprise_oauth"
oauth_client_id: "your-azure-app-id"
oauth_client_secret: "your-secret"               # Sensitive
oauth_tenant_id: "your-tenant-id"

# Optional - Catalog Indexing
catalog_schemas: ["dbo", "analytics"]            # Schemas to index (empty = all)
```

---

## PostgreSQL

### Authentication Modes

#### 1. Password - Default
- **Mode ID:** `password`
- **Description:** Authenticate with database username and password
- **Supports shared credentials:** Yes

### Connection Configuration

```yaml
# Required
host: "localhost"                         # Database host
port: 5432                                # Default: 5432
database: "mydb"                          # Database name

# Optional - SSL
ssl_mode: "require"                       # disable, allow, prefer, require, verify-ca, verify-full

# Optional - SSH Tunnel
use_ssh_tunnel: true
ssh_host: "bastion.example.com"
ssh_port: 22
ssh_username: "tunnel_user"
# SSH auth via key pair (generated in UI) or password

# Optional - Catalog Indexing
catalog_schemas: ["public", "analytics"]  # Schemas to index (empty = all)
```

---

## ClickHouse

### Authentication Modes

#### 1. Password - Default
- **Mode ID:** `password`
- **Description:** Authenticate with database username and password
- **Supports shared credentials:** Yes

### Connection Configuration

```yaml
# Required
host: "localhost"                         # ClickHouse host
port: 8443                                # HTTPS port (8123 for HTTP)
database: "default"                       # Default database

# Optional
secure: true                              # Use HTTPS (default: true for port 8443)
verify_ssl: true                          # Verify SSL certificate

# Optional - Catalog Indexing
catalog_databases: ["db1", "db2"]         # Databases to index (empty = all)
```

---

## Databricks

### Authentication Modes

#### 1. Personal Access Token - Default
- **Mode ID:** `token`
- **Description:** Use a Databricks personal access token for authentication
- **How to get:** Databricks workspace > User Settings > Access Tokens

#### 2. Databricks OAuth
- **Mode ID:** `oauth`
- **Description:** Authenticate with your Databricks account via OAuth
- **Note:** OAuth support varies by Databricks deployment type

### Connection Configuration

```yaml
# Required
server_hostname: "your-workspace.cloud.databricks.com"  # Workspace URL
http_path: "/sql/1.0/warehouses/abc123"                 # SQL warehouse path

# Optional
catalog: "main"                           # Unity Catalog name
schema: "default"                         # Default schema

# Optional - Catalog Indexing
catalog_schemas: ["schema1", "schema2"]   # Schemas to index (empty = all)
```

---

## SQL Server

### Authentication Modes

#### 1. Password - Default
- **Mode ID:** `password`
- **Description:** Authenticate with database username and password
- **Supports shared credentials:** Yes

### Connection Configuration

```yaml
# Required
host: "localhost"                         # SQL Server host
port: 1433                                # Default: 1433
database: "master"                        # Database name

# Optional
encrypt: true                             # Use encrypted connection
trust_server_certificate: false           # Trust self-signed certs

# Optional - Catalog Indexing
catalog_schemas: ["dbo", "sales"]         # Schemas to index (empty = all)
```

---

## MySQL

### Authentication Modes

#### 1. Password - Default
- **Mode ID:** `password`
- **Description:** Authenticate with database username and password
- **Supports shared credentials:** Yes

### Connection Configuration

```yaml
# Required
host: "localhost"                         # MySQL host
port: 3306                                # Default: 3306
database: "mydb"                          # Database name

# Optional
ssl_mode: "REQUIRED"                      # DISABLED, PREFERRED, REQUIRED

# Optional - Catalog Indexing
catalog_databases: ["db1", "db2"]         # Databases to index (empty = all)
```

---

## Redshift

### Authentication Modes

#### 1. Password - Default
- **Mode ID:** `password`
- **Description:** Authenticate with database username and password
- **Supports shared credentials:** Yes

### Connection Configuration

```yaml
# Required
host: "your-cluster.region.redshift.amazonaws.com"  # Cluster endpoint
port: 5439                                          # Default: 5439
database: "dev"                                     # Database name

# Optional
ssl_mode: "require"                       # SSL mode

# Optional - Catalog Indexing
catalog_schemas: ["public", "analytics"]  # Schemas to index (empty = all)
```

---

## DuckDB

### Authentication Modes

#### 1. No Authentication - Default
- **Mode ID:** `none`
- **Description:** No authentication required (local file-based database)

### Connection Configuration

```yaml
# Required
database_path: "/path/to/database.duckdb"  # Path to DuckDB file

# Optional - Catalog Indexing
catalog_schemas: ["main"]                  # Schemas to index
```

---

## Shared Credentials

For datasources that support shared credentials (PostgreSQL, MySQL, ClickHouse, SQL Server, Synapse SQL, Redshift), workspace admins can configure credentials that all users share:

```yaml
# In connection_config
shared_credentials: true
shared_username: "readonly_user"
shared_password: "secret"                 # Sensitive - masked in API
```

When shared credentials are enabled:
- Users don't need to provide their own credentials
- Users can enable/disable the datasource via their preferences
- All queries run with the shared credentials

---

## Catalog Indexing

Each datasource supports catalog indexing for schema discovery. The indexed schemas/tables appear in the AI assistant's context for query generation.

### Configuration Keys by Type

| Datasource | Config Key | Container Type |
|------------|-----------|----------------|
| BigQuery | `catalog_projects` | Project |
| Snowflake | `catalog_schemas` | Schema |
| Synapse | `catalog_schemas` | Schema |
| PostgreSQL | `catalog_schemas` | Schema |
| ClickHouse | `catalog_databases` | Database |
| Databricks | `catalog_schemas` | Schema |
| SQL Server | `catalog_schemas` | Schema |
| MySQL | `catalog_databases` | Database |
| Redshift | `catalog_schemas` | Schema |
| DuckDB | `catalog_schemas` | Schema |

### Empty vs. Specified

- **Empty list (`[]`)**: Index ALL available schemas/databases
- **Specified list**: Index ONLY the listed schemas/databases

---

## Security Notes

### Sensitive Fields

The following fields are automatically masked in API responses:

**Connection Config (workspace-level):**
- `oauth_client_secret` - OAuth client secrets
- `service_account_json` - Service account credentials
- `shared_password` - Shared credential passwords

**User Credentials (per-user):**
- `password` - Database passwords
- `client_secret` - Service principal secrets
- `oauth_access_token` / `oauth_refresh_token` - OAuth tokens
- `private_key` / `passphrase` - Key pair credentials
- `access_token` - API tokens

### OAuth Token Storage

- **Global OAuth** (BigQuery kyomi_oauth): Tokens in `User.oauth_data`
- **Per-datasource OAuth** (Enterprise, Snowflake, etc.): Tokens in `UserDatasourceCredential`
