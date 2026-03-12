---
layout: doc
title: Security
description: How we keep Kyomi and your data secure
---

# Security at Kyomi

Security isn't just a feature—it's foundational to everything we build. This page explains our security practices in detail.

---

## Our Security Philosophy

**Privacy by design:** We built Kyomi to minimize data warehouse data storage. We cache table/column metadata for search, but your actual data rows stay in your warehouse. Query results stream through our infrastructure but aren't stored (except limited 20-row samples in AI chat history). With Kyomi Connect, your database credentials never leave your infrastructure at all.

**Defense in depth:** Multiple layers of security protect your account, data, and infrastructure — whether you connect directly or via Kyomi Connect.

**Transparency:** We're open about our security practices so you can make informed decisions.

---

## Data Security

### Your Data Warehouse Data

**The most important point:** We minimize data warehouse data storage and use encryption when we do store it.

**What we cache:** Table and column metadata (names, types, descriptions) to enable intelligent search. This metadata is workspace-isolated.

**What we don't cache:** Your actual data rows. Query results are handled differently depending on the mode (see below).

Kyomi has three data access modes with different security implications:

#### Mode 1: AI Agent Queries (Server-Side, Limited)
- **20-row limit** - AI queries are restricted to maximum 20 rows
- **Server-side analysis** - Results sent to our backend for AI analysis
- **Encrypted storage** - 20-row samples stored encrypted at rest with AES-256-GCM (for chat history display)
- **Workspace-isolated** - Only you can access your chat data
- **User-deletable** - Delete chat history anytime

#### Mode 2: Standard Dashboard Access (Default, Most Secure)
- **Direct to browser** - Results stream directly from your data warehouse API to your browser
- **Bypasses our servers** - Data never touches our infrastructure
- **Client-side processing** - DuckDB WASM processes data in your browser
- **No storage** - Query results aren't stored by us

**This is the default mode** for dashboards and SQL editor.

#### Mode 3: Arrow Streaming (Opt-In, Faster)
- **Optional checkbox** - You choose to enable this for faster downloads
- **Through our server** - Data passes through backend (20-100x faster)
- **In-memory only** - Data never written to disk on our servers
- **No storage** - Data immediately discarded after streaming

**Result:** Even if our servers were compromised:
- **Mode 1 (AI Agent)**: Only encrypted 20-row samples exposed (not full datasets)
- **Mode 2 (Standard)**: Zero data warehouse data exposed (bypasses our servers)
- **Mode 3 (Arrow)**: Zero data warehouse data exposed (in-memory only, no persistence)

#### With Kyomi Connect

If you use Kyomi Connect instead of a direct connection, the data flow changes:

- **Database → Connect agent → Kyomi backend** — All queries pass through the agent on your infrastructure
- **Same storage rules apply** — Query results are not stored by the Connect agent or by Kyomi's backend (except 20-row AI samples)
- **Mode 2 note** — With Connect, query results pass through Kyomi's backend rather than streaming directly from your warehouse API to your browser. Results are still not persisted
- **Arrow Streaming not applicable** — Mode 3 (Arrow Streaming) is a BigQuery-specific optimization and is not available for Connect-based datasources

**If the Connect agent host were compromised:**
- The agent has access to your database (via locally stored credentials) — the same access you granted it
- Query results pass through the agent's memory briefly but are not cached or written to disk
- The agent's authentication token could be used to impersonate the agent to Kyomi's backend. Revoke tokens immediately from Settings if you suspect compromise

### Connection Security by Datasource Type

Different datasources use different authentication methods, all secured appropriately:

**OAuth-Based (Most Secure)**
- BigQuery: Google OAuth 2.0 with automatic token refresh
- Snowflake: OAuth 2.0 for enterprise deployments
- Databricks: OAuth 2.0 for supported deployments
- Azure Synapse: Microsoft OAuth 2.0 with automatic token refresh
- Slack: OAuth 2.0 with automatic token refresh

**Token-Based**
- Databricks: Personal Access Tokens (encrypted at rest)
- ClickHouse: Tokens or username/password

**Username/Password**
- PostgreSQL, MySQL, SQL Server, Redshift, Snowflake, Synapse
- Credentials encrypted with AES-256-GCM at rest
- SSH tunnel support for on-premise databases

**Key Pair Authentication**
- Snowflake: RSA key pair authentication supported
- Private keys encrypted at rest

**Kyomi Connect (On-Premise Agent)**
- Agent authenticates to Kyomi via ES256-signed JWT token
- Token is verified against Kyomi's public key (fetched once at startup via JWKS)
- Tokens are revocable — issuing a new token invalidates the previous one
- Database credentials are stored locally on the customer's machine, not on Kyomi's infrastructure
- All communication between Connect agent and Kyomi backend is over TLS-encrypted WebSocket

**All credentials for direct connections are:**
- Encrypted in transit (TLS 1.3)
- Encrypted at rest (AES-256-GCM)
- Workspace-isolated (multi-tenant)
- Deletable at any time

**Kyomi Connect credentials are:**
- Stored locally on your infrastructure (not on Kyomi's servers)
- Config files created with restricted file permissions (owner read/write only)
- For Kubernetes: injected from Kubernetes Secrets, never written to disk inside the container
- Your responsibility to secure and rotate

### Application Data

Data we do store (chat messages, dashboards, queries, catalog metadata) is protected with:

#### Encryption

- **In transit** - TLS 1.3 for all connections
- **At rest** - AES-256-GCM authenticated encryption for:
  - Chat message content (AI responses, summaries)
  - Chat message metadata (20-row data samples)
  - OAuth tokens (Google, Snowflake, Databricks, Microsoft access/refresh tokens)
- **Backups** - Encrypted backups in Google Cloud Storage

#### Catalog Metadata

We cache metadata about your data warehouse tables to enable intelligent search:

**What we store:**
- Table names, descriptions, and schema structure
- Column names, data types, and descriptions
- Modified timestamps for incremental indexing

**What we DON'T store:**
- Actual data rows from your tables
- Query results (except 20-row samples in AI chat history)

**How it's protected:**
- ✅ Stored in PostgreSQL database
- ✅ Workspace-isolated (only your team can access)
- ✅ Used only for search functionality
- ✅ Incremental updates minimize data transfer

**Why we cache it:** Enables semantic search across thousands of tables, helping you quickly find relevant datasets without querying BigQuery repeatedly.

#### Access Controls

- **Multi-tenant isolation** - Workspace data is strictly isolated
- **Role-based access** - Users only see what they're authorized to see
- **API authentication** - OAuth 2.0 with short-lived access tokens
- **Session management** - Secure HTTP-only cookies

#### Data Retention

- **Active data** - Kept while your account is active
- **Deleted data** - Permanently deleted within 30 days
- **Backups** - Retained for 30 days, then permanently deleted
- **Logs** - Security logs kept for 90 days

### Analytics Data (Kyomi Analytics — Separate from Data Warehouse Connections)

The security model above describes data warehouse connections where your data stays in your warehouse. Kyomi Analytics (built-in website tracking) is different — we collect and store anonymized event data on our infrastructure. Here's how we secure it:

- **Cookie-free** — No cookies set on your website visitors' browsers
- **IP hashing** — Visitor IPs are hashed with a daily rotating salt; raw IPs are never stored
- **No PII** — No personal data is collected or stored
- **Isolated storage** — Analytics events stored in ClickHouse, workspace-isolated
- **Encryption** — Data encrypted in transit (TLS 1.3) and at rest
- **Automatic cleanup** — Data automatically deleted after retention period (tier-dependent)
- **Domain validation** — Events are only accepted from domains you've explicitly configured
- **Separate infrastructure** — Analytics data is stored in a dedicated ClickHouse database, isolated from application data

### Kyomi Connect Agent Security

Kyomi Connect is an on-premise agent that runs on your infrastructure to bridge your database to Kyomi. Because it runs outside our infrastructure, its security model is different from the rest of Kyomi.

#### What the Agent Is

- An open-source Rust application licensed under [Apache 2.0](https://github.com/kyomi-ai/kyomi-connect) — the source code is publicly auditable by your security team
- Distributed as a single statically-linked binary (~10 MB) or a minimal scratch-based Docker container
- Runs as a long-lived process on your server, VM, or Kubernetes cluster
- Makes only outbound connections — no inbound ports or firewall rules required

#### Authentication

**Agent → Kyomi backend:**
- The agent authenticates using an ES256-signed JWT token issued by Kyomi
- The token contains the datasource ID, workspace ID, and database type — no database credentials
- On each connection, Kyomi's backend verifies the JWT signature, checks the token hasn't been revoked, and validates the datasource configuration
- Tokens can be revoked instantly from Settings — issuing a new token invalidates the old one

**Agent → Your database:**
- The agent connects to your database using credentials stored locally on your machine
- SSL/TLS is supported and configurable (prefer, require, verify-ca, verify-full)
- The agent tests the database connection on startup and fails fast if it cannot connect

#### Network Connections

The agent makes exactly three types of outbound connections:

| Connection | When | Protocol | Purpose |
|-----------|------|----------|---------|
| JWKS key fetch | Once at startup | HTTPS | Fetches Kyomi's public key to verify its own JWT token |
| WebSocket to Kyomi | Persistent | WSS (TLS) | Receives queries from Kyomi, sends back results |
| Database connection | Per query | Database-native (with optional TLS) | Executes SQL queries against your database |

No other network connections are made. No telemetry, no analytics endpoints, no crash reporting services, no DNS lookups beyond what's needed for the above.

#### Local Data Handling

- **No data cache** — The agent does not cache query results, schema data, or any database content on disk or in memory beyond what's needed to process the current request
- **No log files** — Logs are written to stdout only (structured JSON), suitable for collection by your existing log infrastructure (Docker, systemd, Kubernetes). No log files are written to disk by the agent
- **No local database** — The agent has no embedded database or persistent state beyond its configuration file

#### Credential Storage

- **Config file** (`~/.config/kyomi-connect/config.toml`) — Contains database host, port, name, user, SSL mode, and the Kyomi JWT token. Created with file permissions `0600` (owner read/write only)
- **Password file** (`~/.config/kyomi-connect/.db-password`) — Contains the database password in a separate file. Created with file permissions `0600`
- **Kubernetes** — Credentials are injected as environment variables from Kubernetes Secrets. The container uses a scratch base image with no shell, no package manager, and no writable filesystem
- **Systemd** — Credentials stored in `/etc/kyomi-connect/env` with restricted permissions, loaded as environment variables by the systemd unit

#### Updates

The Connect agent does not auto-update. You control when to update:
- **Binary installations** — Download the new version and replace the existing binary
- **Docker** — Pull the new image tag
- **Kubernetes/Helm** — Run `helm upgrade` with the new chart version

We recommend updating promptly when we announce security patches. We will clearly communicate security-relevant updates via email to workspace owners.

#### Compromise Scenarios

| Scenario | Impact | Mitigation |
|----------|--------|------------|
| Connect host compromised | Attacker could access your database using locally stored credentials, and impersonate the agent to Kyomi | Revoke the Connect token from Settings immediately. Rotate database credentials. Investigate the host |
| Kyomi backend compromised | Attacker could send arbitrary SQL to the agent for execution | The agent executes queries as whatever database user you configured — use a read-only database user with least-privilege permissions |
| JWT token stolen | Attacker could impersonate the agent from another machine | Revoke and reissue the token from Settings. The old token is invalidated immediately |
| Network interception | Attacker could observe query traffic between agent and Kyomi | All traffic is TLS-encrypted (WebSocket over TLS). Enable database SSL for the local connection as well |

---

## Authentication & Access

### Authentication Methods

Kyomi supports multiple secure authentication methods:

#### Email & Password
- Passwords hashed using bcrypt (industry-standard one-way hashing)
- We never store plaintext passwords
- Recommended: Use a strong, unique password

#### Google OAuth
- Industry-standard OAuth 2.0 flow
- No password to manage for Kyomi
- Leverages Google's security infrastructure

#### Passkeys (WebAuthn)
- Passwordless authentication using device biometrics or security keys
- Public key cryptography - private keys never leave your device
- Phishing-resistant by design
- Supported on modern browsers and devices

#### Two-Factor Authentication (2FA)
- TOTP-based 2FA compatible with Google Authenticator, Authy, etc.
- Adds an extra layer of security to password-based login
- 2FA secrets encrypted at rest

### Session Security

- **Short-lived tokens** - Access tokens expire automatically
- **Refresh tokens** - Securely stored and rotated
- **Automatic logout** - Sessions expire after period of inactivity
- **HTTP-only cookies** - Protected from XSS attacks

---

## Infrastructure Security

### Infrastructure Hosting

- **Infrastructure** - Secure dedicated servers with encrypted storage
- **Network** - VPC isolation, private subnets
- **Firewalls** - Strict ingress/egress rules
- **Physical security** - Secure data center with controlled access

### Application Security

#### Code Security
- **Dependency scanning** - Automated vulnerability detection
- **Static analysis** - CodeQL security analysis
- **Secret scanning** - No credentials in code
- **Security updates** - Patched within 48 hours of disclosure

#### Runtime Security
- **Container isolation** - Docker containers with minimal attack surface
- **Least privilege** - Services run with minimum necessary permissions
- **Security headers** - CSP, HSTS, X-Frame-Options, etc.
- **Rate limiting** - Protection against brute force and DoS

#### API Security
- **Input validation** - All inputs sanitized and validated
- **SQL injection prevention** - Parameterized queries only
- **XSS protection** - Content Security Policy, output encoding
- **CSRF protection** - Token-based validation

### Monitoring & Detection

- **Security monitoring** - 24/7 automated threat detection
- **Intrusion detection** - Anomaly detection and alerting
- **Error tracking** - Sentry for error monitoring
- **Audit logs** - All sensitive operations logged
- **Uptime monitoring** - UptimeRobot checks every 5 minutes

---

## Third-Party Security

We carefully vet all third-party services:

### Critical Services

| Service | Purpose | Security Features |
|---------|---------|-------------------|
| **Google BigQuery** | Cloud data warehouse | SOC 2, ISO 27001, GDPR compliant |
| **Snowflake** | Cloud data warehouse | SOC 2 Type II, HIPAA eligible |
| **AWS Redshift** | Cloud data warehouse | SOC 1/2/3, ISO 27001 |
| **Azure Synapse** | Cloud analytics | SOC 1/2, ISO 27001, GDPR |
| **Databricks** | Lakehouse platform | SOC 2 Type II, HIPAA eligible |
| **PostgreSQL/MySQL** | Self-hosted databases | Your security responsibility |
| **ClickHouse** | Analytics database | Your security responsibility |
| **Google OAuth** | Authentication | Industry-standard OAuth 2.0 |
| **Slack** | Team messaging integration | SOC 2, Enterprise Grid, E2E encryption |
| **Stripe** | Payment processing | PCI DSS Level 1 |

### Data Processing Agreements

We have DPAs in place with all services that process customer data, ensuring:
- GDPR compliance
- Data protection requirements
- Incident notification procedures
- Secure data handling practices

---

## Security Best Practices for Users

### For Workspace Owners

- ✅ **Enable 2FA on your Google account** - Protect your Kyomi access
- ✅ **Review workspace members** - Remove users who no longer need access
- ✅ **Monitor usage** - Check for suspicious activity
- ✅ **Keep contact info current** - So we can reach you about security issues

### For All Users

- ✅ **Don't share credentials** - Each team member should have their own account
- ✅ **Log out on shared computers** - Or use private browsing
- ✅ **Report suspicious activity** - Email [security@kyomi.ai](mailto:security@kyomi.ai)
- ✅ **Keep your browser updated** - For the latest security patches
- ✅ **Be cautious with dashboard sharing** - Understand who can see shared dashboards

### For Data Warehouse Security

**For Cloud Data Warehouses (BigQuery, Snowflake, Redshift, Databricks):**
- Use least-privilege IAM/roles—Grant Kyomi only necessary permissions
- Enable audit logging—Track all data warehouse access
- Monitor costs—Detect unusual query patterns
- Use network restrictions where available

**For Self-Hosted Databases (PostgreSQL, MySQL, ClickHouse):**
- Use read-only database users for Kyomi
- Restrict network access (firewall rules, VPN, SSH tunnels)
- Enable database audit logging
- Rotate credentials regularly
- Use SSL/TLS for all connections

### For Kyomi Connect Users

- ✅ **Use a dedicated database user** — Create a separate, read-only database user for the Connect agent with only the permissions it needs
- ✅ **Enable database SSL** — Set SSL mode to `require` or `verify-full` for the connection between the agent and your database
- ✅ **Secure the host** — Restrict access to the machine running the agent. Protect the config files in `~/.config/kyomi-connect/` (they contain your database credentials)
- ✅ **Use Kubernetes Secrets** — If running on Kubernetes, inject credentials from Secrets rather than embedding them in config files or Helm values
- ✅ **Monitor the agent** — Collect the agent's stdout logs with your existing log infrastructure. Watch for unexpected connection drops or query errors
- ✅ **Keep the agent updated** — Update to new versions promptly, especially when we announce security patches
- ✅ **Restrict outbound network** — The agent only needs to reach Kyomi's backend (`api.kyomi.ai` on port 443) and your database. Block all other outbound traffic if your network policy allows it
- ✅ **Revoke tokens when decommissioning** — If you stop using the agent, revoke its token from Settings before removing it from your infrastructure

---

## Incident Response

### If You Suspect a Security Issue

**Act quickly:**

1. **Email us immediately** - [security@kyomi.ai](mailto:security@kyomi.ai)
2. **Include details** - What you observed, when, any evidence
3. **Don't publicly disclose** - Give us time to fix it first

**We'll respond within 24 hours** (usually much faster)

### Our Incident Response Process

If we detect a security incident:

1. **Contain** - Stop the threat immediately
2. **Investigate** - Understand scope and impact
3. **Notify** - Inform affected users within 72 hours
4. **Remediate** - Fix the vulnerability
5. **Post-mortem** - Learn and improve

### What You'll Receive

- **Transparent communication** - We'll tell you what happened
- **Timeline** - When it occurred, when we detected it
- **Impact assessment** - What data (if any) was affected
- **Remediation steps** - What we're doing to fix it
- **Prevention** - How we're preventing it from happening again

---

## Vulnerability Disclosure

### Responsible Disclosure

We welcome security researchers to help us stay secure:

- **Report vulnerabilities** - [security@kyomi.ai](mailto:security@kyomi.ai)
- **Give us time to fix** - 90 days before public disclosure
- **Don't exploit** - Don't access user data or disrupt service
- **Act in good faith** - We won't take legal action against responsible disclosures

### Bug Bounty Program

Currently, we don't have a formal bug bounty program, but we:

- Acknowledge all valid reports
- Fix vulnerabilities promptly
- Credit researchers (with permission) in our security updates
- Provide public thanks for significant findings

Interested in a formal bug bounty? Email us—we're considering it for the future.

---

## Compliance & Certifications

### Current Status

- **GDPR** - Compliant with EU data protection regulations
- **CCPA** - Compliant with California privacy laws
- **SOC 2** - Planned for when we have enterprise customers requiring it
- **ISO 27001** - Under consideration for global certification

### Privacy Regulations

We apply EU-level data protection to all users:

- **Data minimization** - Collect only what's necessary
- **Purpose limitation** - Use data only for stated purposes
- **Storage limitation** - Retain only as long as needed
- **Integrity & confidentiality** - Appropriate security measures

---

## Security Updates

We publish security updates when appropriate:

- **Security advisories** - For vulnerabilities affecting users
- **Changelog** - For security-related feature updates
- **Blog posts** - For major security improvements

**Subscribe to updates:** [Blog](/blog/) or follow our changelog

---

## Questions About Security?

We're happy to discuss our security practices:

- **General questions** - [security@kyomi.ai](mailto:security@kyomi.ai)
- **Vulnerability reports** - [security@kyomi.ai](mailto:security@kyomi.ai)
- **Enterprise inquiries** - [enterprise@kyomi.ai](mailto:enterprise@kyomi.ai)

**Response time:** 24 hours for security issues, 2 business days for general questions

---

## Security is a Journey

Security isn't a destination—it's an ongoing commitment. We continuously:

- Monitor threats and vulnerabilities
- Update our security practices
- Educate our team
- Listen to our users
- Invest in security infrastructure

**Your trust is our most valuable asset.** We take that responsibility seriously.

---

*Last updated: March 3, 2026*

*For our Privacy Policy, see [kyomi.ai/privacy](/privacy)*
