---
layout: page
title: "One AI, 9 Data Platforms: Announcing Multi-Datasource Support"
description: "Kyomi now connects to BigQuery, Snowflake, PostgreSQL, MySQL, ClickHouse, Redshift, Databricks, SQL Server, and Azure Synapse—all from the same AI-powered interface."
---

<div class="blog-post">

<div class="blog-post-header">
  <h1>One AI, 9 Data Platforms: Announcing Multi-Datasource Support</h1>
  <p class="blog-post-meta">January 11, 2026 · Jason Adams</p>
</div>

<div class="blog-post-content">

**Today we're excited to announce that Kyomi now supports 9 data platforms**, making it the most versatile AI analytics tool for self-service data access.

## The Problem We're Solving

Modern organizations don't use just one data platform. Your company might have:
- **BigQuery** for your analytics warehouse
- **PostgreSQL** for your application database
- **Snowflake** for your enterprise data lake
- **ClickHouse** for real-time analytics

Before today, getting insights meant switching between different tools, learning different SQL dialects, and maintaining separate dashboards for each platform.

**No more.**

## One Interface, Any Data Platform

With Kyomi's multi-datasource support, you can now:

### 1. Query Any Platform with Natural Language

Ask "Show me revenue by region for last quarter" and Kyomi automatically:
- Identifies which datasource has your sales data
- Writes the correct SQL dialect (BigQuery SQL, PostgreSQL, Snowflake SQL, etc.)
- Returns visualizations in the same familiar format

### 2. Build Cross-Platform Dashboards

Create a single dashboard that pulls from multiple datasources:
- Widget 1: Customer metrics from **PostgreSQL**
- Widget 2: Revenue analytics from **BigQuery**
- Widget 3: Real-time events from **ClickHouse**

All updating together, all in one place.

### 3. Use the Same AI Everywhere

The AI learns your business context once and applies it across all platforms:
- Define "revenue" in your workspace knowledge
- That definition works whether you're querying BigQuery or Snowflake
- No re-teaching, no inconsistencies

## Supported Platforms

We're launching with support for 9 data platforms:

**Cloud Data Warehouses**
- Google BigQuery (OAuth)
- Snowflake (Username/Password or OAuth)
- Amazon Redshift (Username/Password)
- Azure Synapse (SQL Auth, Service Principal, or OAuth)
- Databricks (Access Token or OAuth)

**Relational Databases**
- PostgreSQL (Username/Password, SSH Tunnel)
- MySQL (Username/Password, SSH Tunnel)
- SQL Server (Username/Password)

**Analytics Databases**
- ClickHouse (Username/Password)

## Enterprise-Ready Security

We've built multi-datasource support with enterprise security in mind:

**Credential Security**
- All credentials encrypted with AES-256-GCM at rest
- TLS 1.3 for all connections
- OAuth where supported (BigQuery, Databricks)
- SSH tunnel support for on-premise databases

**Access Control**
- Shared credentials for team-wide access
- Personal credentials for individual audit trails
- Admin-controlled datasource configuration

**Same Privacy Principles**
- Your data stays in your warehouse
- We store only table/column metadata for search
- 20-row samples for AI analysis only

## Getting Started

Already a Kyomi user? Multi-datasource is available now:

1. Go to **Settings → Datasources**
2. Click **Add Datasource**
3. Select your platform and enter credentials
4. Start querying!

New to Kyomi? [Get started free →](https://app.kyomi.ai/login)

Have a datasource you'd like us to support? [Let us know →](mailto:hello@kyomi.ai)

---

*Try multi-datasource support today at [app.kyomi.ai](https://app.kyomi.ai)*

</div>

<div class="blog-post-footer">
  <a href="/blog" class="blog-back-link">← Back to Blog</a>
</div>

</div>
