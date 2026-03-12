# Datasource Setup Guides

Detailed guides for connecting your data sources to Kyomi.

## Cloud Data Warehouses

| Datasource | Auth Methods | Key Features |
|------------|--------------|--------------|
| [Google BigQuery](./bigquery) | OAuth, Service Account, Enterprise OAuth | Google Cloud, petabyte-scale analytics |
| [Snowflake](./snowflake) | Password, OAuth, Key Pair | Multi-cloud, data sharing |
| [Amazon Redshift](./redshift) | Password | AWS native, Spectrum for S3 |
| [Databricks](./databricks) | Personal Access Token | Unity Catalog, Delta Lake |
| [Azure Synapse](./synapse) | SQL Auth | Dedicated & serverless pools |

## Relational Databases

| Datasource | Default Port | Key Features |
|------------|--------------|--------------|
| [PostgreSQL](./postgres) | 5432 | Open source, extensive SQL support |
| [MySQL](./mysql) | 3306 | Widely used, MariaDB compatible |
| [SQL Server](./sqlserver) | 1433 | Microsoft, Azure SQL compatible |
| [ClickHouse](./clickhouse) | 8123 | Column-oriented, real-time analytics |

---

## Quick Comparison

### Authentication Methods

| Datasource | Password | OAuth | Token | Key Pair | Service Account |
|------------|:--------:|:-----:|:-----:|:--------:|:---------------:|
| BigQuery | - | ✓ | - | - | ✓ |
| Snowflake | ✓ | ✓ | - | ✓ | - |
| PostgreSQL | ✓ | - | - | - | - |
| MySQL | ✓ | - | - | - | - |
| ClickHouse | ✓ | - | - | - | - |
| SQL Server | ✓ | - | - | - | - |
| Redshift | ✓ | - | - | - | - |
| Databricks | - | - | ✓ | - | - |
| Synapse | ✓ | - | - | - | - |

### Features

| Datasource | SSH Tunnel | Shared Credentials | Catalog Discovery |
|------------|:----------:|:------------------:|:-----------------:|
| BigQuery | - | ✓ (Service Account) | ✓ |
| Snowflake | - | ✓ | ✓ |
| PostgreSQL | ✓ | ✓ | ✓ |
| MySQL | ✓ | ✓ | ✓ |
| ClickHouse | ✓ | ✓ | ✓ |
| SQL Server | ✓ | ✓ | ✓ |
| Redshift | ✓ | ✓ | ✓ |
| Databricks | - | ✓ | ✓ |
| Synapse | - | ✓ | ✓ |

---

## Common Setup Steps

All datasources follow a similar setup pattern:

1. **Add Datasource**: Go to Settings → Datasources → Add Datasource
2. **Select Type**: Choose your datasource type from the dropdown
3. **Configure Connection**: Enter host, port, and connection details
4. **Test Connection**: Click Connect to verify connectivity
5. **Set Credentials**: Enter authentication credentials
6. **Configure Catalog**: Select which databases/schemas to index
7. **Save**: Save the configuration

---

## Need Help?

- Check the specific datasource guide for detailed instructions
- See [Troubleshooting](/docs/#tips-tricks) for common issues
- Contact support@kyomi.ai for assistance

---

[← Back to Docs](/docs/)
