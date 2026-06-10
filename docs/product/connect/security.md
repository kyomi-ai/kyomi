---
layout: doc
title: "Security - Kyomi Connect"
description: "How Kyomi Connect protects your database credentials with JWT authentication, encrypted transport, and zero credential transmission."
---

# Security

Kyomi Connect is designed around a single principle: **database credentials never leave your infrastructure**. This page explains how that works technically.

## Open Source

Kyomi Connect is fully open-source under the [Apache License 2.0](https://github.com/kyomi-ai/kyomi-connect/blob/main/LICENSE). The entire codebase is publicly available at [github.com/kyomi-ai/kyomi-connect](https://github.com/kyomi-ai/kyomi-connect).

This means you can:

- **Audit the code** — Read every line to verify our security claims. No black boxes.
- **Build from source** — Compile the binary yourself with `cargo install kyomi-connect` or clone the repo and build directly.
- **Verify releases** — Compare published binaries against the source code. CI/CD pipelines that build releases are also public.
- **Contribute fixes** — Found a vulnerability? Submit a patch or report it via our [security policy](https://github.com/kyomi-ai/kyomi-connect/blob/main/SECURITY.md).

Open-source doesn't just mean transparency — it means the security claims on this page are verifiable, not just words.

## Credential Isolation

When you use Connect, Kyomi stores only:

- The datasource name and type (e.g., "Production Analytics", PostgreSQL)
- A token identifier (`jti`) for authentication and revocation
- The connection type (`connect`)

Kyomi does **not** store and has **never received**:

- Database hostname or IP address
- Database port
- Database username
- Database password
- SSL certificates or keys

All of this information exists only on the machine running Connect, either in environment variables, a config file with `600` permissions, or a password file.

## JWT Authentication

Connect authenticates with Kyomi using a JWT (JSON Web Token) signed with **ES256 (ECDSA with P-256)** -- an asymmetric signing algorithm.

### Why Asymmetric Signing

With symmetric signing (like HMAC-SHA256), both sides need the same secret key. That means distributing the secret to every Connect instance, and anyone with the secret could forge tokens. ES256 solves this:

- **Kyomi holds the private key** -- only Kyomi can sign (create) tokens
- **Connect uses the public key** -- anyone can verify tokens, but no one can create them
- **Public key is freely available** at `https://api.kyomi.ai/.well-known/jwks.json`

### Token Verification Flow

When Connect starts, it verifies the token before doing anything else:

1. Decode the JWT header to find the issuer URL
2. Fetch the public key from `{issuer}/.well-known/jwks.json` (standard JWKS endpoint)
3. Verify the JWT signature using the ES256 public key
4. If verification fails, **refuse to start** with a clear error message

This prevents a critical attack: if someone intercepts and modifies the token (for example, changing the WebSocket URL to point to their own server to steal query results), the signature check fails and Connect will not connect.

### What the Token Contains

```json
{
  "jti": "unique-token-id",
  "dsid": "datasource-config-uuid",
  "wid": "workspace-uuid",
  "db": "postgres",
  "url": "wss://api.kyomi.ai/connect/v1",
  "iss": "https://api.kyomi.ai",
  "iat": 1740700000
}
```

| Field | Purpose |
|---|---|
| `jti` | Unique token ID, used for revocation |
| `dsid` | Identifies which datasource this token belongs to |
| `wid` | Workspace ID for multi-tenant routing |
| `db` | Database type (determines which driver Connect loads) |
| `url` | WebSocket endpoint Connect should connect to |
| `iss` | Issuer URL (used to locate the JWKS endpoint) |
| `iat` | Issued-at timestamp |

Connect tokens do not have an expiration time. Revocation is handled explicitly through the `jti` -- see [Token Rotation](#token-rotation) below.

## Data Flow

Here is exactly what travels over the wire during a typical query:

```
1. User asks Kyomi: "What were last month's sales?"

2. Kyomi's AI generates SQL:
   SELECT SUM(amount) FROM orders
   WHERE created_at >= '2026-02-01'

3. Kyomi sends to Connect via WebSocket:
   {"id":"req-1","op":"execute_query","params":{"sql":"SELECT ..."}}

4. Connect executes the SQL against your local database

5. Connect sends result rows back via WebSocket:
   {"id":"req-1","type":"result","result":{"rows":[[42500.00]],...}}

6. Kyomi's AI interprets the result and responds to the user
```

**What is transmitted**: SQL query text and result rows (column names, data types, values).

**What is never transmitted**: Database hostname, port, username, password, connection strings, SSL certificates.

::: info
Query results do pass through Kyomi's servers on their way to the AI model for analysis. Connect protects credentials, not data in transit. For most use cases this is acceptable -- you are asking Kyomi to analyze your data. If your regulatory environment requires that result data also stays on-premise, Connect alone is not sufficient.
:::

### Transport Encryption

The WebSocket connection between Connect and Kyomi uses **WSS (WebSocket Secure)** -- TLS-encrypted transport. All data in transit is encrypted.

## Token Rotation

### When to Rotate

Rotate your Connect token when:

- An employee with access to the token leaves the organization
- You suspect the token has been compromised
- Your security policy requires periodic credential rotation
- You are migrating Connect to a different machine

### How to Rotate

1. In Kyomi, go to **Settings > Datasources** and select the Connect datasource
2. Click **Rotate Token**
3. Kyomi generates a new token with a new `jti` and invalidates the old one immediately
4. Copy the new token
5. Update the token in your Connect deployment:
   - **Environment variable**: Update `KYOMI_TOKEN` and restart the container/service
   - **Config file**: Run `kyomi-connect setup` and paste the new token
   - **Kubernetes Secret**: Update the Secret and restart the pod
6. Connect will authenticate with the new token on its next connection

::: warning
The old token is invalidated instantly when you click "Rotate Token". Any running Connect instance using the old token will be disconnected on its next handshake attempt. Update the token and restart Connect promptly.
:::

### Disconnecting Entirely

To fully revoke access, click **Disconnect** on the datasource settings page. This deletes the `jti` entirely -- the token can never be used again, even if someone has a copy.

## Network Requirements

Connect needs exactly one outbound connection:

| Direction | Destination | Port | Protocol | Purpose |
|---|---|---|---|---|
| Outbound | `api.kyomi.ai` | 443 | WSS (WebSocket over TLS) | Command channel to Kyomi backend |

**No inbound ports are required.** Connect initiates the WebSocket connection outbound, and all communication flows over that single connection. This means Connect works behind firewalls, NATs, and corporate proxies that allow outbound HTTPS.

### Firewall Configuration

If your network uses an allowlist for outbound connections, allow:

- `api.kyomi.ai` on port `443` (HTTPS/WSS)

During setup (one-time), Connect also fetches the public key from `api.kyomi.ai/.well-known/jwks.json` over HTTPS -- same host and port as above.

### Proxy Support

If your network requires an HTTP proxy for outbound connections, set the standard environment variables:

```bash
HTTPS_PROXY=http://proxy.internal:8080
```

Connect uses standard Rust HTTP/WebSocket libraries that respect `HTTPS_PROXY` and `NO_PROXY` environment variables.

## Secret Handling

### In Memory

After startup, credentials exist only in memory. Connect reads environment variables and config files once during initialization, then holds the values in process memory. No credentials are written to disk by Connect at runtime.

### Config File Permissions

The setup wizard creates config files with `600` permissions (owner read/write only):

- `~/.config/kyomi-connect/config.toml` -- contains the token and database connection settings (password stored by reference, not value)
- `~/.config/kyomi-connect/.db-password` -- contains the database password

### Token in Container Environments

| Method | Security consideration |
|---|---|
| `KYOMI_TOKEN` env var | Visible in container inspect but not in logs. Standard for Docker/K8s. |
| `KYOMI_TOKEN_FILE` env var | Reads from a file. Preferred for Docker secrets and K8s secret volumes. |
| Kubernetes Secret | Mounted as a file or injected as an env var. Use `existingSecret` in the Helm chart. |
| AWS Secrets Manager | Injected by ECS at task startup. Never stored on disk. |

### What Connect Logs

Connect logs connection events and command execution summaries at `info` level. It **never** logs:

- Database passwords
- The full JWT token
- Query result data

At `debug` level, SQL query text is logged. Avoid `debug` in production if your queries contain sensitive data.
