---
layout: doc
title: Privacy Policy
description: How Kyomi protects your data and respects your privacy
---

# Privacy Policy

**Last Updated:** March 3, 2026

**Effective Date:** November 16, 2025

## Our Privacy Philosophy

At Kyomi, we believe privacy is a fundamental right, not a luxury. We built Kyomi to be privacy-first from the ground up. This means:

- **Your data warehouse data stays in your warehouse** — We cache table/column names (metadata) for search, but never copy your actual data rows. Kyomi Analytics (optional built-in website tracking) works differently — see [Two Data Models](#how-kyomi-handles-data-two-distinct-models) below.
- **Minimal data collection** - We only collect what's necessary to operate the service
- **No selling your data** - Ever. Your data is yours, not a product
- **Transparent practices** - This policy explains exactly what we do (and don't do)

---

## How Kyomi Handles Data — Three Distinct Models

Kyomi has three separate ways of handling data. Understanding these distinctions is key to understanding our privacy practices:

### 1. Data Warehouse Connections (BigQuery, PostgreSQL, etc.)

When you connect your data warehouse or database to Kyomi directly:

- **Your data stays in your warehouse** — We never copy, move, or store your data rows
- **We cache metadata only** — Table names, column names, and descriptions (for search)
- **Queries run in your warehouse** — Results stream to your browser or through our server temporarily
- **We are NOT a data store** — Your cloud provider or database host retains your data

This applies to: BigQuery, PostgreSQL, MySQL, Snowflake, Redshift, Databricks, Azure Synapse, ClickHouse, SQL Server.

### 2. Kyomi Connect (On-Premise Agent)

When you use Kyomi Connect to bridge your database to Kyomi:

- **Your data stays on your infrastructure** — Query results pass through the Connect agent running on your network to Kyomi's backend, but are never stored by the agent
- **Database credentials stay on your machine** — Unlike direct connections where we store encrypted credentials, Connect keeps your database username and password on your infrastructure only
- **No local data cache** — The Connect agent does not cache, log, or persist any query results or database contents
- **Outbound-only connection** — The agent makes an outbound WebSocket connection to Kyomi. No inbound firewall ports are required
- **No telemetry** — The Connect agent does not send analytics, crash reports, diagnostics, or any data beyond query results back to Kyomi

This applies to: PostgreSQL, MySQL, Redshift, ClickHouse, SQL Server, Azure Synapse, Snowflake, Databricks.

### 3. Kyomi Analytics (Built-in Website Tracking)

When you create an analytics site to track your own website:

- **We ARE the data store** — We collect and store event data on our infrastructure
- **We act as a data processor** — You (the site owner) are the data controller
- **Anonymized data only** — No cookies, no raw IPs, no personal data
- **Tier-based retention** — Data automatically deleted after your retention period (30 days to 2 years)
- **You can delete anytime** — Removing your analytics site permanently deletes all collected data

These three models have different privacy implications, and they are described separately throughout this policy.

---

## What Data We Collect

### Account Data
When you create an account, we collect:

- **Email address** - For authentication and important service notifications
- **Name** (optional) - For personalization
- **Workspace information** - Workspace name, settings, and preferences

### Chat Messages & Analysis
When you use Kyomi's AI features, we store:

- **Chat messages** - Your questions and AI responses
- **Query summaries** - The AI may include summaries of your data in chat responses
- **Session metadata** - Timestamps, token usage for billing

**Important:** While chat messages may reference your data (e.g., "revenue increased 20%"), we never store the underlying BigQuery data itself. Your data remains in your data warehouse.

### Data Warehouse Catalog Metadata
To enable intelligent table and column search, we cache metadata about your data warehouse tables:

- **Table names and descriptions** - The names and descriptions of your tables
- **Column names and types** - Column names, data types, and descriptions
- **Schema structure** - Table and dataset organization
- **Modified timestamps** - When tables were last updated (for incremental indexing)

**Supported Data Platforms:**
- Cloud Data Warehouses: BigQuery, Snowflake, Redshift, Azure Synapse, Databricks
- Relational Databases: PostgreSQL, MySQL, SQL Server
- Analytics Databases: ClickHouse

The same privacy principles apply to all platforms—we store only table/column metadata for search, never your actual data rows.

**Important clarifications:**
- ✅ **Metadata only** - We store table/column names and descriptions, NOT the actual data in your tables
- ✅ **Incremental updates** - We only re-index tables that have changed
- ✅ **Workspace-isolated** - Your catalog metadata is only accessible to your workspace
- ✅ **Enables smart search** - Powers semantic search to help you find relevant tables quickly

**Example:** If you have a table called `sales_2024` with columns `revenue`, `region`, `date`, we store those names and types—but not the actual sales data.

### Technical Data
To operate the service, we automatically collect:

- **Authentication tokens** - To keep you logged in securely
- **OAuth tokens** - When you connect datasources via OAuth (Google/BigQuery, Snowflake, Databricks, Microsoft/Azure Synapse), we store your OAuth access and refresh tokens (encrypted) to access your data on your behalf
- **Server logs** - Access logs, error logs, API usage, and performance metrics for debugging, security monitoring, and service improvement
- **IP address** - For security and fraud prevention (stored with active sessions, deleted when session expires or within 30 days)
- **Browser/device information** - To ensure compatibility and display in session management

### Authentication Data

Depending on how you sign up and authenticate, we store:

**If you use email/password:**
- Password hash (bcrypt) - We never store your actual password, only a one-way hash

**If you use Google OAuth:**
- OAuth tokens (encrypted) - To keep you logged in and access connected services

**If you use passkeys:**
- Public key credential - The public portion of your passkey (your private key never leaves your device)
- Credential metadata - Authenticator type, creation date

**If you enable 2FA:**
- TOTP secret (encrypted) - Used to verify your authenticator app codes
- Recovery codes (hashed) - For account recovery if you lose your authenticator

### Website Analytics Event Data (Kyomi Analytics)
If you create an analytics site to track your own website, we collect and store event data from your website's visitors on our infrastructure. This is the one area where Kyomi stores data beyond metadata — see [Two Data Models](#how-kyomi-handles-data-two-distinct-models) for context.

- **Page views** — URLs, referrers, UTM parameters
- **Device information** — Browser, OS, screen size (from User Agent)
- **Geographic location** — Country/region derived from hashed IP
- **Custom events** — Events you configure via the tracking script

**Important:** This data is collected from your website visitors, not from your data warehouse. Raw IP addresses are never stored — they are hashed with a daily rotating salt. No cookies or personal data are collected. See the [Kyomi Analytics section](#kyomi-analytics-built-in-website-analytics-data-we-store-on-your-behalf) for full details.

---

## What Data We DON'T Collect

- ❌ **Your data warehouse data** - We cache table/column names (metadata) but never the actual data rows
- ❌ **Credit card numbers** - Handled by Stripe, we never see them
- ❌ **Browsing history** - We don't track you across the web
- ❌ **Advertising data** - No ads, no tracking pixels, no surveillance

**Note:** The above applies to your data warehouse connections (Model 1) and Kyomi Connect (Model 2) — in both cases, we never store your actual data rows. If you use Kyomi Analytics (Model 3 — built-in website tracking), anonymized event data from your website visitors IS collected and stored on our infrastructure. This data contains no personal information. See [Kyomi Analytics](#kyomi-analytics-built-in-website-analytics-data-we-store-on-your-behalf) for details.

---

## How We Use Your Data

### Service Operation
- **Authentication** - Log you in and keep your account secure
- **Billing** - Track AI usage for subscription billing
- **Support** - Help you when you need assistance. Our support team does not access your chat history (which includes 20-row data samples) unless you explicitly request help with a specific issue and share details with us.
- **Security & Performance** - Monitor server logs for security threats, bugs, and performance issues
- **Product Improvement** - Analyze usage patterns from server logs to improve features and fix issues

---

## How Your Data Flows Through Kyomi

**Note:** This section describes how data flows for your **data warehouse connections** (BigQuery, PostgreSQL, etc.) — whether connected directly or via Kyomi Connect. For Kyomi Analytics (website tracking), data flows differently — see [Kyomi Analytics](#kyomi-analytics-built-in-website-analytics-data-we-store-on-your-behalf).

Kyomi has three different data access modes, each with different privacy implications. If you use **Kyomi Connect**, the Connect agent sits between your database and Kyomi's backend — see [Kyomi Connect Data Flow](#kyomi-connect-data-flow) below for how this changes each mode.

### Mode 1: AI Agent Queries (Server-Side, Limited)

When you ask the AI agent a question:

1. **AI generates query** - Creates SQL based on your question
2. **Query executed** - Runs in your data warehouse with a 20-row limit
3. **Results to our server** - Limited result set (max 20 rows) sent to our backend
4. **AI analyzes** - Agent summarizes the data to answer your question (see "Third-Party AI Processing" below)
5. **Stored in chat history** - The AI's summary AND the 20-row sample are saved

**Third-Party AI Processing:**
To provide AI-powered data analysis, we send the 20-row sample to **Anthropic (Claude AI)** for processing. This is necessary to generate insights, answer your questions, and create summaries of your data.

**What's sent to Anthropic:**
- Your question and the 20-row data sample
- Table/column metadata for context
- Previous chat messages in the conversation

**What's NOT sent:**
- Your full dataset (only 20-row samples)
- Your credentials or OAuth tokens
- Data from queries you run manually (only AI-initiated queries)

**Anthropic's commitments:**
- Anthropic does not train models on data sent via their API (per their [Commercial Terms](https://www.anthropic.com/legal/commercial-terms))
- Data is processed only to generate your AI response
- Anthropic's Data Processing Addendum (DPA) applies to our commercial API usage, which includes GDPR-compliant Standard Contractual Clauses (SCCs)

**What we store:**
- The AI's summary/answer (e.g., "Revenue increased 15% in Q3")
- The 20-row sample data (displayed in the "thinking bubble" for context)

**How it's protected:**
- ✅ Encrypted at rest in our database (AES-256-GCM authenticated encryption)
- ✅ Only accessible to you (workspace-isolated)
- ✅ Only used to display your chat history—never for training, analytics, or any other purpose
- ✅ You can delete chat history anytime

**Why we store the 20 rows:** To provide a rich user experience—you can see exactly what data the AI analyzed when answering your question. This transparency helps you trust the AI's answers.

### Mode 2: Standard Dashboard Access (Direct to Browser, No Server)

When you view dashboards or run manual queries:

1. **Query executed** - Runs in your data warehouse
2. **Direct to browser** - Results stream directly from your data warehouse API to your browser
3. **Bypasses our servers** - Data never touches our infrastructure
4. **Client-side processing** - DuckDB WASM processes data entirely in your browser

**What we store:** Query metadata (SQL text, execution time, bytes processed)
**What we don't store:** The query results or any row data

**This is the default mode** for dashboards and SQL editor.

### Mode 3: Arrow Streaming (Opt-In, Through Server, Faster)

If you enable Arrow streaming (optional checkbox):

1. **Query executed** - Runs in your BigQuery
2. **Arrow format** - Results streamed in efficient Arrow format
3. **Through our server** - Data passes through our backend
4. **To your browser** - Delivered to your browser for display
5. **In-memory only** - Data is NOT written to disk on our servers

**What we store:** Nothing—data streams through memory and is immediately discarded
**What we don't store:** The query results (only exists in RAM briefly)

**Why use this:** 20-100x faster data transfer for large result sets
**Privacy trade-off:** Data passes through our servers (but isn't stored)

### Summary Table

| Mode | Data Through Server? | Data Stored? | Use Case |
|------|---------------------|--------------|----------|
| **AI Agent** | ✅ Yes (20 rows max) | 20-row sample + summary (encrypted) | "Show me revenue trends" |
| **Standard** | ❌ No (direct to browser) | No | Default dashboard viewing |
| **Arrow Streaming** | ✅ Yes (in-memory only) | No | Fast large data downloads |

### Kyomi Connect Data Flow

If you use Kyomi Connect instead of a direct database connection, the data flow changes. The Connect agent runs on your infrastructure and acts as a secure bridge:

1. **Kyomi backend sends query** — SQL query is sent over an encrypted WebSocket to the Connect agent on your network
2. **Connect agent executes query** — The agent connects to your database locally and runs the query
3. **Results sent back** — Query results are sent back over the same encrypted WebSocket to Kyomi's backend
4. **No local storage** — The Connect agent does not cache, store, or log any query results

**What the Connect agent processes:**
- SQL queries from Kyomi's backend
- Query results from your database
- Schema discovery requests (table/column names for catalog indexing)

**What the Connect agent does NOT process:**
- Your chat messages or AI conversations
- Dashboard definitions or markdown
- Any data unrelated to the queries Kyomi sends it

**How this changes the modes above:**

| Mode | Direct Connection | With Kyomi Connect |
|------|------------------|--------------------|
| **AI Agent** | Results go directly from warehouse API to Kyomi backend | Results go from database → Connect agent → Kyomi backend |
| **Standard** | Results stream directly from warehouse API to your browser | Results go from database → Connect agent → Kyomi backend → your browser |

**Note:** Arrow Streaming (Mode 3) is a BigQuery-specific optimization and is not available for Kyomi Connect datasources. Connect uses its own streaming protocol over WebSocket.

**Privacy note:** With Kyomi Connect, Mode 2 (Standard) no longer bypasses Kyomi's servers — all queries flow through the Connect agent and then to the backend. However, query results are still not stored by either the Connect agent or Kyomi's backend (except for AI Agent 20-row samples, same as direct connections).

### What We Always Store (All Modes)

- **Query metadata** - SQL text, execution time, bytes processed
- **Chart definitions** - ChartML code for your dashboards
- **Dashboard markdown** - Your dashboard content
- **AI summaries** - The agent's analysis and answers (Mode 1 only)
- **Data warehouse catalog metadata** - Table/column names, descriptions, types (enables intelligent search)

**We never store:** The actual query results or raw data rows from your warehouse (only metadata like table/column names).

---

## Data Storage & Security

### Where Your Data Lives

- **Application data** - PostgreSQL database on secure dedicated infrastructure
- **Your data warehouse** - Stays in your cloud provider or on-premise infrastructure (you control the location)
- **Kyomi Connect agent** - Runs on your infrastructure; database credentials stored locally on your machine (not on ours)
- **Chat messages** - Stored in our database, encrypted at rest

### Security Measures

- ✅ **Encryption in transit** - All data encrypted with TLS 1.3
- ✅ **Encryption at rest** - Chat messages and OAuth tokens encrypted with AES-256-GCM
- ✅ **Secure authentication** - Google OAuth, optional 2FA/passkeys
- ✅ **Access controls** - Role-based permissions, workspace isolation
- ✅ **Regular backups** - Automated daily backups (encrypted)
- ✅ **Security monitoring** - Automated threat detection

### Data Retention

- **Active accounts** - Data retained while your account is active
- **Deleted accounts** - Permanent deletion within 30 days
- **Chat history** - Kept for the life of your workspace (you can delete anytime)
- **Logs** - Security logs retained for 90 days

---

## Third-Party Services

We use trusted third-party services to operate Kyomi:

### Required Services

- **Google OAuth** - Authentication and BigQuery access (Google's Privacy Policy applies). We store your OAuth tokens (encrypted) to access BigQuery on your behalf.
- **Microsoft OAuth** - Azure Synapse access (Microsoft's Privacy Policy applies). We store your OAuth tokens (encrypted) to access Synapse on your behalf.
- **Snowflake OAuth** - Snowflake access for enterprise deployments (Snowflake's Privacy Policy applies). We store your OAuth tokens (encrypted) to access Snowflake on your behalf.
- **Databricks OAuth** - Databricks access (Databricks' Privacy Policy applies). We store your OAuth tokens (encrypted) to access Databricks on your behalf.

*You can disconnect any OAuth connection anytime in Settings.*

- **Anthropic (Claude AI)** - AI-powered data analysis. When you use the AI chat feature, we send your questions and 20-row data samples to Anthropic's Claude API for processing. Anthropic does not train models on API data ([Commercial Terms](https://www.anthropic.com/legal/commercial-terms)). See "Mode 1: AI Agent Queries" section for full details.
- **Stripe** - Payment processing (Stripe's Privacy Policy applies)
- **Email delivery (SMTP)** - For account notifications, password resets, Watch alerts, and workspace invitations. We send emails via SMTP to addresses you provide.
- **Sentry** - Error monitoring service. When errors occur, we send error reports (stack traces, request context) to Sentry to help us identify and fix bugs. Reports may include your user ID but not your data warehouse data.
- **Your connected data warehouses (BigQuery, Snowflake, PostgreSQL, etc.)** - Your data (you control, not us)

### Optional Integrations

- **Google Cloud** - If you connect your GCP project (your data, your control)
- **Slack** - If you connect your Slack workspace (your messages, your control)
- **MCP (Model Context Protocol)** - If you connect Kyomi to Claude Code or Cursor (see below)

### Slack Integration

If you connect Kyomi to Slack, we store:

**What we store:**
- **Slack bot token** - Workspace-level token for posting messages (encrypted at rest)
- **Slack user token** - Your personal token for posting as yourself (encrypted at rest)
- **Slack user ID** - To link your Slack identity to your Kyomi account
- **Connection metadata** - When you connected, token expiry times

**What's sent to Slack:**
- AI responses to your @kyomi mentions
- Chart images rendered from your data (uses same 20-row sample limit as AI queries)
- Messages you send from Kyomi web that sync to Slack threads

**Bi-directional sync:**
- Messages in Slack threads appear in your Kyomi conversation history
- Messages sent from Kyomi web can sync back to Slack threads (if you've connected)

**Your control:**
- Run `/kyomi disconnect` in Slack to revoke access anytime
- Tokens are automatically refreshed; you don't need to reconnect
- Disconnecting removes all stored Slack tokens immediately

### Kyomi Watch (Data Monitoring)

If you create Watches to monitor your data, we store:

**What we store:**
- **Watch configuration** - Your monitoring instructions, schedule, and notification preferences
- **Execution history** - When watches ran, their status, and AI analysis results
- **Alert data** - Notifications generated when conditions are met

**How watches process your data:**
- Watches use the same AI agent as chat (Anthropic Claude)
- Same 20-row sample limit applies to watch queries
- Watch results are stored to display in your alert inbox

**Notifications:**
- **Slack alerts** - Posted to channels you configure (uses your Slack connection)
- **Email alerts** - Sent to email addresses you specify
- **In-app notifications** - Stored in your Kyomi notification inbox

**Your control:**
- Disable or delete watches anytime
- Configure which channels/emails receive alerts
- Delete watch execution history

### MCP Integration (Claude Code & Cursor)

If you connect Kyomi via MCP (Model Context Protocol), you can access your data intelligence from Claude Code or Cursor.

**How MCP works:**
- MCP is an open protocol that lets AI assistants connect to external tools
- Kyomi's MCP server exposes your data catalog, query capabilities, and learnings
- Authentication uses your existing Kyomi OAuth tokens (same security as web app)

**What's accessible via MCP:**
- **Data catalog search** - Find tables and columns across your connected datasources
- **Table information** - View schema details, column types, descriptions
- **Query execution** - Run SQL queries against your data warehouses (same limits apply)
- **Learnings** - Access and save workspace knowledge

**What we store:**
- MCP requests are logged the same as regular API requests
- No additional data is stored beyond what the web app already stores

**Privacy notes:**
- ✅ Same authentication and authorization as the web app
- ✅ Same 20-row limit for AI queries
- ✅ Same workspace isolation
- ✅ You control which datasources are accessible
- ✅ Disconnect anytime from Settings

**Third-party involvement:**
- **Anthropic** - Claude Code is an Anthropic product. When you use MCP with Claude Code, your queries go through Claude's interface. Anthropic's privacy policy applies to their products.
- **Cursor** - Cursor is a third-party IDE with AI capabilities. When you use MCP with Cursor, your queries go through Cursor's interface. Cursor's privacy policy applies to their product.
- **Your data** - Query results flow from your data warehouse → Kyomi → Claude/Cursor. Same data handling as the Kyomi web app.

### Kyomi Connect (On-Premise Agent)

If you install Kyomi Connect on your infrastructure, a lightweight agent runs on your network to bridge your database to Kyomi.

**What the agent is:**
- An open-source Rust application ([source code on GitHub](https://github.com/kyomi-ai/kyomi-connect)) available as a standalone binary (~10 MB) or Docker container that runs on your infrastructure
- It makes an outbound WebSocket connection to Kyomi's backend — no inbound ports are required
- It executes SQL queries against your local database on behalf of Kyomi
- The source code is publicly auditable — you can verify our privacy claims yourself

**What the agent stores locally:**
- A configuration file with your database connection details and Kyomi authentication token
- No query results, no cached data, no logs to disk (logs go to stdout only)

**What the agent sends to Kyomi:**
- Query results (streamed back to Kyomi's backend over an encrypted WebSocket)
- Schema metadata (table/column names during catalog discovery)

**What the agent does NOT send to Kyomi:**
- Database credentials (these never leave your network)
- Telemetry, analytics, crash reports, or diagnostics of any kind
- Any data beyond query results and schema metadata

**Network connections made by the agent:**
- One-time HTTPS request at startup to fetch Kyomi's public signing key (JWKS endpoint)
- Persistent outbound WebSocket to Kyomi's backend (TLS-encrypted)
- Local connection to your database
- No other network connections

**Your control:**
- Revoke access anytime from Settings (invalidates the agent's authentication token)
- Stop or uninstall the agent at any time — no data is left behind beyond the config file
- The agent has no auto-update mechanism — you control when and whether to update it

### In-App Notifications

Kyomi has an in-app notification system for alerts and updates:

**What we store:**
- **Notification content** - The message shown to you
- **Notification metadata** - Type, source (e.g., which Watch triggered it), timestamps
- **Read/unread status** - To show unread badges

**Retention:**
- Active notifications are kept until you delete them or they expire
- Deleted notifications are permanently removed within 30 days

**Important:** We have data processing agreements with all third-party services to ensure they handle your data responsibly.

### Database Credentials

#### Direct Connections

For non-OAuth datasources (PostgreSQL, MySQL, Snowflake, etc.) connected directly to Kyomi, we store connection credentials:

**What we store:**
- Encrypted at rest with AES-256-GCM
- Username, password, or access tokens
- Host, port, database configuration

**Security measures:**
- Credentials are workspace-isolated
- Only used to execute queries on your behalf
- Admin-configurable shared credentials option
- You can delete credentials anytime in Settings

**OAuth Credentials (BigQuery, Snowflake, Databricks, Microsoft/Azure Synapse):**
- Stored as encrypted refresh tokens
- Automatically rotated by the OAuth provider
- You can disconnect OAuth anytime in Settings

#### Kyomi Connect

When you use Kyomi Connect, your database credentials are stored differently:

**What's stored on your infrastructure (not ours):**
- Database username, host, port, and database name — stored in a local config file (`~/.config/kyomi-connect/config.toml`)
- Database password — stored in a separate local file (`~/.config/kyomi-connect/.db-password`)
- Both files are created with restricted permissions (owner read/write only)
- For Kubernetes deployments, credentials are injected from Kubernetes Secrets and never written to disk

**What's stored on Kyomi's infrastructure:**
- A signed authentication token (JWT) that identifies the Connect agent — stored encrypted in our database
- No database credentials — we never receive or store your database host, username, or password when using Connect

**Your responsibility:**
- Secure the machine running the Connect agent, including the config files containing your database credentials
- Rotate database credentials as needed — update the Connect agent's local configuration accordingly
- Revoke the Connect token from Kyomi's settings if the agent is decommissioned

---

## Your Rights & Controls

You have full control over your data:

### Access
- **View your data** - See all data we have about you
- **Export your data** - Download dashboards, queries, chat history
- **Request a data report** - Email us for a complete data export

### Control
- **Update your information** - Change email, name, preferences anytime
- **Delete chat history** - Delete individual chats or all history
- **Delete your account** - Permanent deletion within 30 days

### Data Portability
- **Export dashboards** - Download as markdown files
- **Export queries** - Download query history as CSV
- **Export chat history** - Download as JSON

**How to exercise these rights:** Email [privacy@kyomi.ai](mailto:privacy@kyomi.ai) or manage your data through Settings

---

## Legal Compliance

### GDPR (European Union)
If you're in the EU, you have additional rights:
- Right to access, rectification, erasure, restriction, portability
- Right to object to processing
- Right to withdraw consent
- Right to lodge a complaint with supervisory authority

### CCPA (California)
If you're in California, you have rights to:
- Know what personal information we collect
- Delete your personal information
- Opt out of sale (we don't sell data anyway)
- Non-discrimination for exercising your rights

### International Users
We apply EU-level data protection to all users, regardless of location. Everyone deserves privacy.

---

## Website Analytics

We use our own privacy-focused analytics system to understand how visitors use kyomi.ai and app.kyomi.ai:

### What Our Analytics Collects
- **Page URL** — Which pages you visit
- **HTTP Referrer** — Where you came from
- **Browser type** — Which browser you're using
- **Operating system** — Your OS type
- **Device type** — Desktop, mobile, or tablet
- **Country** — Derived from IP address (IP not stored)

### Why Our Analytics is Privacy-Friendly
- ✅ **No cookies** — Completely cookie-free tracking
- ✅ **No personal data** — Cannot identify individual visitors
- ✅ **No cross-site tracking** — Doesn't follow you around the web
- ✅ **Self-hosted** — Data stored on our own infrastructure, not third parties
- ✅ **GDPR/CCPA compliant** — No consent banner needed
- ✅ **IP hashing** — IP addresses are hashed daily and never stored in raw form

### What We Use Analytics For
- Understand which features are most useful
- Improve user experience and navigation
- Measure marketing campaign effectiveness
- Identify and fix broken pages

**Important:** All analytics data is aggregated and anonymized. We cannot identify individual visitors or track your behavior across websites.

---

## Kyomi Analytics — Built-in Website Analytics (Data We Store on Your Behalf)

Kyomi offers built-in website analytics as a product feature. When you create an analytics site, you add a lightweight tracking script to **your own website**, and Kyomi collects and stores event data from your website's visitors **on our infrastructure**.

**This is different from our data warehouse integrations** — with BigQuery, PostgreSQL, etc., your data stays in your warehouse and we only cache metadata. With Kyomi Analytics, we are the data store. We act as a **data processor** on your behalf.

### What's Collected from Your Website Visitors

When a visitor loads a page on your website with the Kyomi tracking script:

- **Page view data** — URL, page title, referrer, UTM parameters
- **Browser/device info** — User agent string, screen size, device type
- **Geographic location** — Country and region derived from visitor IP address
- **Custom events** — Any events you explicitly track via the JavaScript API (e.g., button clicks, form submissions)
- **Session identification** — A daily-rotating hash of IP + User Agent (not the raw IP)

### What is NOT Collected

- ❌ **Raw IP addresses** — IPs are hashed with a daily rotating salt and discarded; raw IPs are never stored
- ❌ **Cookies** — No cookies are set on your visitors' browsers
- ❌ **Personal data** — No names, emails, or identifiable information is collected
- ❌ **Cross-site tracking** — Each analytics site is isolated; visitors cannot be tracked across different websites
- ❌ **Fingerprinting** — We don't use browser fingerprinting techniques

### Data Storage

- **Where:** Analytics events are stored in ClickHouse on our dedicated infrastructure
- **Isolation:** Each workspace's analytics data is isolated and only accessible to that workspace
- **Encryption:** Data is encrypted in transit (TLS 1.3) and at rest
- **Retention:** Depends on your subscription tier:
  - Free: 30 days
  - Starter: 180 days
  - Pro: 1 year
  - Team: 2 years
- **Automatic cleanup:** Data older than your retention period is automatically deleted
- **Manual deletion:** You can delete your analytics site at any time, which permanently removes all associated event data

### Our Role as Data Processor (GDPR)

When you use Kyomi Analytics, we act as a **data processor** under GDPR. You (the site owner) are the **data controller**:

- **You decide** what data to collect by deploying the tracking script on your website
- **We process** that data on your behalf, storing it securely and making it queryable
- **You can delete** all collected data at any time by removing your analytics site
- **We comply** with data subject access and deletion requests forwarded by you

### Your Responsibilities as Site Owner

By deploying the Kyomi tracking script on your website, you are responsible for:

- **Privacy compliance** — Ensuring your website's privacy policy discloses the use of analytics
- **Legal basis** — Determining the appropriate legal basis for data collection in your jurisdiction
- **Visitor communication** — Informing visitors about data collection if required by applicable law
- **Data subject requests** — Handling any data subject requests from your website visitors

**Note:** Since Kyomi Analytics is cookie-free and does not collect personal data (no raw IPs, no cookies, no identifiable information), most privacy regulations (including GDPR and CCPA) do not require a consent banner. However, we recommend consulting with a legal professional for your specific jurisdiction.

---

## Cookies

We use minimal cookies:

- **Essential cookies** - Keep you logged in (session token)
- **No tracking cookies** - We don't use advertising or analytics cookies

See our [Cookie Policy](/cookies) for details.

---

## Children's Privacy

Kyomi is not intended for users under 13 years old. We don't knowingly collect data from children. If you believe a child has provided us data, contact [privacy@kyomi.ai](mailto:privacy@kyomi.ai) and we'll delete it immediately.

---

## Changes to This Policy

We may update this policy to reflect changes in our practices or legal requirements. When we make significant changes:

- **Notice** - We'll email you at least 30 days before changes take effect
- **Version history** - Previous versions available on request
- **Continued use** - Using Kyomi after changes means you accept the new policy

---

## Contact Us

Questions about privacy? We're here to help:

- **Email:** [privacy@kyomi.ai](mailto:privacy@kyomi.ai)
- **Privacy Officer:** Jason (Founder)
- **Response time:** We aim to respond within 48 hours

For security issues, email [security@kyomi.ai](mailto:security@kyomi.ai)

---

**Our Commitment:** Privacy isn't just a policy for us—it's a core value. We're building the analytics tool we'd want to use ourselves, and that means treating your data with the respect it deserves.

*Last updated: March 2, 2026*
