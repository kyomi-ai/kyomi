---
layout: page
title: "Introducing Kyomi Connect: Your Credentials Never Leave Your Network"
description: "Most analytics platforms ask you to trust them with your database credentials. Kyomi Connect eliminates that ask entirely."
---

<div class="blog-post">

<div class="blog-post-header">
  <h1>Introducing Kyomi Connect: Your Credentials Never Leave Your Network</h1>
  <p class="blog-post-meta">March 3, 2026 · Jason Adams</p>
</div>

<div class="blog-post-content">

Every analytics platform eventually asks the same question: "Can we have your database credentials?"

It's the moment the conversation gets uncomfortable. Your security team tenses up. Your DBA raises an eyebrow. And honestly? They should. Credential-based attacks were the #1 breach vector in 2025, responsible for nearly 4 in 10 incidents. Every additional place your credentials are stored is another surface to defend.

Today we're introducing **Kyomi Connect** — an open-source, on-premise agent that eliminates this ask entirely. Your database credentials stay on your infrastructure. Kyomi never sees them, never stores them, never transmits them. Not encrypted in our cloud. Not hashed. Not "securely managed." Simply *not there*.

And because Connect is [open-source under the Apache 2.0 License](https://github.com/kyomi-ai/kyomi-connect), you don't have to take our word for any of this. Read the code yourself.

## The Industry's Compromise

Every major analytics platform has tried to solve this problem. The solutions follow a pattern: add layers of protection around credentials that still live in the vendor's cloud.

**SSH tunnels** (Metabase, Looker, dbt) encrypt the connection but still require credentials stored in the vendor's platform. **PrivateLink** (Databricks, Snowflake, dbt) keeps traffic off the public internet but requires same-cloud-provider infrastructure and credentials in the vendor's cloud. **On-premise gateways** (Power BI, Tableau Bridge) run locally, but the vendor's cloud still orchestrates credential delivery — encrypted, but transiting through infrastructure you don't control.

These are reasonable approaches. They're also all variations of the same trade-off: "Trust us with your credentials, and we'll protect them."

We thought: what if we just removed the trade-off?

## How Kyomi Connect Works

Connect is a single Rust binary that runs inside your network. The entire source code is on [GitHub](https://github.com/kyomi-ai/kyomi-connect). The architecture is deliberately simple:

```
Your Network                              Kyomi Cloud
┌─────────────────────────┐              ┌──────────────┐
│                         │              │              │
│   Kyomi Connect         │   outbound   │  Kyomi API   │
│   (binary/container)    │──WebSocket──>│              │
│         |               │    only      │  Sends SQL   │
│         v               │              │  Gets results│
│    Your Database        │              │              │
└─────────────────────────┘              └──────────────┘
```

When Kyomi's AI agent needs to query your data, it sends the SQL to Connect over an outbound WebSocket. Connect runs the query against your database locally and returns the results. The credentials that Connect uses to talk to your database are configured locally — environment variables, Docker secrets, whatever your team already uses for secret management.

Kyomi's cloud never receives, processes, or stores your database credentials. Not during setup, not during operation, not ever. This isn't a policy. It's the architecture.

### What Kyomi Gets

When you set up a Connect datasource, Kyomi generates a signed JWT token. That token contains:

- A datasource identifier
- Your workspace ID
- The database type (PostgreSQL, MySQL, etc.)
- The WebSocket endpoint URL

That's it. No host, no port, no username, no password. The token tells Connect *where to phone home* — not how to reach your database. Those details live exclusively on your machine.

### Authentication: Trust, but Verify

The token is signed with ES256 (ECDSA with P-256 curve). When Connect starts, it fetches Kyomi's public key from a well-known endpoint and verifies the token signature before doing anything else. A tampered token won't even start the agent — the signature check fails and Connect refuses to run.

This is the same asymmetric key infrastructure that secures OAuth across the industry. The private key lives in Kyomi's infrastructure. Connect only needs the public key to verify authenticity. Neither side trusts the other blindly.

### Firewall Friendly

Connect opens a single **outbound** WebSocket connection to Kyomi. No inbound ports. No firewall rules to modify. No VPN to configure. No bastion host to maintain.

If your network allows outbound HTTPS (and it almost certainly does), Connect works. This is the same pattern that makes Slack, Teams, and every other modern SaaS tool work behind corporate firewalls — outbound connections that the client initiates.

## Two Minutes to Deploy

Connect ships as a Docker image, a standalone binary, a Helm chart, and a [crate on crates.io](https://crates.io/crates/kyomi-connect). The setup wizard walks you through it:

```
$ kyomi-connect setup

Paste your token from the Kyomi dashboard:
> eyJhbGciOiJFUzI1NiJ9...

Fetching datasource info...
✓ Token valid for: "Production Analytics" (PostgreSQL)

Database host: [localhost] > prod-db.internal
Database port: [5432] >
Database name: [] > analytics
Database user: [] > readonly_user
Database password: > ********

Testing database connection... ✓ Connected
Testing Kyomi connection...    ✓ WebSocket established

✅ Kyomi Connect is ready!
```

From token to connected in under two minutes. No infrastructure provisioning, no cloud-to-cloud networking, no support tickets.

## Why This Matters for Your Security Team

When your security team evaluates an analytics tool, they ask three questions:

**1. "Where are our credentials stored?"**

With Connect: on your infrastructure. Full stop. Your existing secrets management (Vault, AWS Secrets Manager, Kubernetes Secrets, Docker Secrets, even plain environment variables) handles credential storage. Kyomi's answer to "where are credentials?" is "ask your own infra team."

**2. "What network access does this require?"**

One outbound WebSocket to `wss://api.kyomi.ai`. No inbound ports. No VPN. No PrivateLink configuration. No same-cloud-provider requirement. Your firewall rules don't change.

**3. "What happens if the vendor gets breached?"**

An attacker who compromises Kyomi's cloud would find no database credentials — not encrypted, not hashed, not anywhere. There is no "decrypt the vault" scenario because there is no vault. The attack surface for credential theft through Kyomi is zero, because the credentials aren't there to steal.

This isn't defense-in-depth. It's eliminating the attack vector entirely.

## The Trust Gap for Growing Companies

We'll be direct about something: Kyomi is a growing company. We don't have a decade-long track record. We don't have a Fortune 500 client list to point to.

Enterprise buyers rightfully ask harder questions of newer vendors. SOC 2 Type II certification, penetration test reports, security questionnaires — these are reasonable gates, and we're working through them.

But Connect offers something that no certification can: **architectural proof backed by open source**. You don't have to trust that we handle credentials responsibly. You don't have to trust our encryption, our key rotation, our access controls around stored secrets. You don't have to trust us at all. The credentials are on your machine, managed by your team, governed by your policies. And the agent that manages them is [source code you can read](https://github.com/kyomi-ai/kyomi-connect).

Open-sourcing Connect under the Apache 2.0 License means our security claims are verifiable, not just marketing copy. Your security team can audit every line. Your engineers can build from source with `cargo install kyomi-connect`. Our CI/CD pipelines are public. There are no black boxes.

Trust-by-architecture, verified by open source, beats trust-by-policy. We'd rather earn your business by removing the need for trust than by asking you to take our word for it.

## What You Keep

Connect doesn't compromise any of Kyomi's capabilities. You still get:

- **AI-powered analytics** — Ask questions in natural language, get answers with SQL and charts
- **Accumulated knowledge** — Kyomi still learns your data catalog, metric definitions, and business context
- **Watches** — Automated monitoring that scans your data on a schedule and alerts you
- **Dashboards** — Build and share interactive dashboards with your team
- **MCP integration** — Use Kyomi from Claude Code, Cursor, or any MCP client

The only thing that changes is where the credentials live. Everything else works exactly the same — because Connect implements the same query interface as a direct connection. Kyomi's agent doesn't know or care whether it's talking to your database directly or through Connect. It just sends SQL and gets results.

## Getting Started

Connect supports PostgreSQL, MySQL, ClickHouse, SQL Server, Redshift, and Azure Synapse — every password-based datasource Kyomi works with.

If you're already using Kyomi with a direct connection, switching to Connect takes about five minutes: create a new Connect datasource in the dashboard, deploy the binary, and point it at the same database.

If you've been waiting to try Kyomi because your security team wasn't comfortable sharing credentials — Connect is the answer we built for you. And it's open-source, so your team can verify every claim before deploying it.

**[Set up Kyomi Connect →](/docs/connect/)** · **[View source on GitHub →](https://github.com/kyomi-ai/kyomi-connect)**

---

*Have questions about Connect's security architecture? Want to discuss deployment options for your environment? [Reach out to us](mailto:security@kyomi.ai) — we're happy to do a technical deep dive with your security team.*

</div>
</div>

<style scoped>
.blog-post {
  max-width: 48rem;
  margin: 0 auto;
  padding: 0 1.5rem 4rem;
}

.blog-post-header {
  text-align: center;
  padding-top: 3rem;
  margin-bottom: 3rem;
}

.blog-post-header h1 {
  font-size: 2.25rem;
  font-weight: 700;
  line-height: 1.2;
  margin-bottom: 1rem;
}

.blog-post-meta {
  color: var(--color-muted-foreground);
  font-size: 0.95rem;
}

.blog-post-content {
  font-size: 1.05rem;
  line-height: 1.75;
  color: var(--color-foreground);
}

.blog-post-content h2 {
  margin-top: 2.5rem;
  margin-bottom: 1rem;
  font-size: 1.5rem;
  font-weight: 600;
}

.blog-post-content h3 {
  margin-top: 2rem;
  margin-bottom: 0.75rem;
  font-size: 1.2rem;
  font-weight: 600;
}

.blog-post-content p {
  margin-bottom: 1.25rem;
}

.blog-post-content pre {
  margin: 1.5rem 0;
  padding: 1.25rem;
  background: var(--color-muted);
  border-radius: 0.5rem;
  overflow-x: auto;
}

.blog-post-content blockquote {
  border-left: 3px solid var(--color-primary);
  padding-left: 1.25rem;
  margin: 1.5rem 0;
  color: var(--color-muted-foreground);
}

.blog-post-content ul, .blog-post-content ol {
  margin-bottom: 1.25rem;
  padding-left: 1.5rem;
}

.blog-post-content li {
  margin-bottom: 0.5rem;
}
</style>
