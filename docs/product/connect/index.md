---
layout: doc
title: "Kyomi Connect - Keep Credentials on Your Infrastructure"
description: "Deploy a lightweight agent inside your network. Database credentials never leave your infrastructure."
---

# Kyomi Connect

Kyomi Connect is a lightweight, [open-source](https://github.com/kyomi-ai/kyomi-connect) agent that runs inside your network and proxies database queries from Kyomi's cloud backend. Your database credentials stay on your infrastructure — they are never transmitted to or stored by Kyomi. From Kyomi's perspective, everything works exactly the same: the AI assistant, dashboards, watches, and SQL editor all function identically whether you use a direct connection or Connect.

::: tip Open Source — Apache 2.0
Kyomi Connect is fully open-source under the [Apache License 2.0](https://github.com/kyomi-ai/kyomi-connect/blob/main/LICENSE). You can read every line of code, verify the security model yourself, and contribute improvements. Source code: [github.com/kyomi-ai/kyomi-connect](https://github.com/kyomi-ai/kyomi-connect)
:::

## How It Works

```
Your Network                                          Kyomi Cloud
+-------------------------------------------+        +------------------+
|                                            |        |                  |
|  +----------+      +-----------------+     |        |                  |
|  | Database  |<---->| Kyomi Connect   |<---+--WSS-->|  Kyomi Backend   |
|  +----------+      +-----------------+     |        |                  |
|                                            |        |                  |
|  Credentials stay here                     |        |  No credentials  |
+-------------------------------------------+        +------------------+
```

Connect opens an outbound WebSocket connection to Kyomi's API. Kyomi sends SQL commands through the WebSocket; Connect executes them against your local database and returns the result rows. No inbound ports are required -- Connect only makes outbound connections.

## Connection Type Comparison

When adding a datasource, you choose one of three connection methods:

| | Direct | SSH Tunnel | Kyomi Connect |
|---|---|---|---|
| **Where credentials stored** | Kyomi (encrypted) | Kyomi (encrypted) | Your infrastructure only |
| **Infrastructure you run** | None | Bastion / jump host | Connect binary or container |
| **Network requirements** | Database publicly accessible or allowlisted | SSH access to bastion | Outbound HTTPS/WSS from Connect host |
| **Best for** | Cloud databases with IP allowlisting | Databases behind a bastion | Firewalled databases, regulatory compliance, zero-trust environments |

All three methods produce identical behavior from Kyomi's perspective -- the same AI assistant, dashboards, and SQL editor experience.

## When to Use Connect

- **Firewalled databases** -- Your database is not accessible from the public internet and you cannot allowlist Kyomi's IP addresses.
- **Regulatory compliance** -- Your security policy prohibits sharing database credentials with third-party SaaS vendors.
- **Zero-trust preference** -- You want to evaluate Kyomi without entrusting it with credentials. Connect lets you try the full product while keeping credentials on your side.

## Quick Start

1. **Create a datasource in Kyomi** -- Go to Settings > Datasources > Add Datasource. Select your database type and choose "Kyomi Connect" as the connection method.

2. **Copy your Connect token** -- After saving, Kyomi generates a one-time token. Copy it now -- it will not be shown again.

3. **Install and run Connect** -- On a machine that can reach your database:
   ```bash
   curl -fsSL https://raw.githubusercontent.com/kyomi-ai/kyomi-connect/main/scripts/install.sh | bash
   ```
   The installer downloads the binary from [GitHub Releases](https://github.com/kyomi-ai/kyomi-connect/releases), verifies the SHA256 checksum, and runs the interactive setup wizard. Paste your token when prompted, then enter your database credentials.

   Alternatively, install via Cargo:
   ```bash
   cargo install kyomi-connect
   ```

4. **Verify in Kyomi** -- The datasource status in the Kyomi UI updates to "Connected" in real time. You can now query your data, index the catalog, and use the AI assistant.

::: tip
For Docker, Kubernetes, and other deployment methods, see the [Installation guide](./installation).
:::

## Supported Databases

Kyomi Connect supports all password-based datasource types:

| Database | Default Port |
|---|---|
| PostgreSQL | 5432 |
| MySQL | 3306 |
| ClickHouse | 8123 |
| SQL Server | 1433 |
| Redshift | 5432 |
| Azure Synapse | 1433 |

## Not Needed For

**BigQuery**, **Snowflake**, and **Databricks** do not require Connect. These platforms use OAuth for authentication -- you authorize Kyomi through the provider's own consent flow, and the resulting tokens are scoped and revocable. There are no passwords to protect, so Connect adds no value.

## Next Steps

- [Installation](./installation) -- All five deployment methods (binary, Docker, Compose, Kubernetes, AWS ECS)
- [Configuration](./configuration) -- Environment variables, health checks, and the setup wizard
- [Security](./security) -- JWT authentication, data flow, and token rotation
- [Troubleshooting](./troubleshooting) -- Common issues and solutions
