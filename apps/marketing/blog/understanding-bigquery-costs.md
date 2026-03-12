---
layout: page
title: Understanding Your BigQuery Costs - How Kyomi Keeps You in Control
description: Learn how Kyomi's built-in cost controls help you analyze data in BigQuery without surprise bills, including dry-run validation, smart table sampling, and a cost-optimized AI agent.
---

<div class="blog-post">

<div class="blog-post-header">
  <h1>Understanding Your BigQuery Costs: How Kyomi Keeps You in Control</h1>
  <p class="blog-post-meta">December 13, 2025 · Jason Adams</p>
</div>

<div class="blog-post-content">

If you've ever worked with BigQuery, you've probably experienced that moment of dread: you run a query, wait for results, and then check your billing dashboard only to discover you just scanned 500GB of data. What should have cost a few cents turned into a $3 charge—or worse.

This "bill shock" is one of the biggest barriers to adopting BigQuery for ad-hoc analytics. The pay-per-scan pricing model is powerful for controlling infrastructure costs, but it can feel unpredictable when you're exploring data or letting an AI agent help with analysis.

**We built Kyomi with cost control as a first-class feature.** Here's how it works.

## The Problem: BigQuery's Pricing Model

BigQuery charges based on the amount of data your query scans, not the results returned. This means:

- A `SELECT *` from a 1TB table costs the same whether you return 10 rows or 10 million
- Poorly written queries can accidentally scan entire partitions
- AI-generated queries might not be optimized for cost

At $6.25 per terabyte scanned, costs can add up quickly—especially when you're exploring unfamiliar datasets or iterating on analysis.

## Solution 1: Real-Time Cost Estimation

Every query in Kyomi runs through a **dry-run validation** before execution. This uses BigQuery's built-in estimation to tell you exactly how much data will be scanned—before you spend a cent.

In the SQL Editor, you'll see this appear automatically as you type:

> ✓ Will scan: **2.34 GB** — Est. cost: **$0.0146**

This happens in real-time, with about 800ms of debouncing after you stop typing. You always know the cost before you commit.

For AI-generated queries, the same validation runs automatically. The agent sees the cost estimate and can adjust its approach if needed—for example, adding filters to reduce scan size or using a sampled preview instead of a full table scan.

## Solution 2: Configurable Query Limits

For AI-powered analysis, Kyomi adds an extra layer of protection: **configurable query size limits**.

In Settings → BigQuery, you can set a maximum scan size for AI-generated queries:

- **Default: 50 GB** — Good for most exploratory analysis
- **Up to 1,000 GB** — For when you need to analyze larger datasets
- **As low as 1 GB** — For tight cost control during exploration

When the AI agent generates a query that would exceed your limit, Kyomi blocks it *before execution* and explains why:

> Query will process 75.2 GB, which exceeds your limit of 50 GB.

The agent then adjusts—adding date filters, sampling the data, or breaking the analysis into smaller chunks.

This limit only applies to AI-generated queries. When you're writing SQL manually in the editor, you have full control and can run queries of any size (though you'll still see the cost estimate).

## Solution 3: Smart Table Sampling

When the AI agent needs to explore an unfamiliar table—checking column values, understanding data patterns, or verifying structure—it doesn't scan the entire table. Instead, Kyomi uses BigQuery's `TABLESAMPLE` feature to read just a tiny fraction of the data.

**Why TABLESAMPLE matters (it's not just LIMIT)**

A common misconception: adding `LIMIT 10` to a query makes it cheap. It doesn't. BigQuery still scans the entire table to find those 10 rows—you pay for the full scan regardless of how many rows you return.

`TABLESAMPLE` is fundamentally different. It tells BigQuery to read only a percentage of the underlying storage blocks. If you sample 0.01% of a table, you're billed for 0.01% of the data—not the full table.

Here's what happens when the agent samples a table:

1. **Row Count Check** — First, we check how many rows the table contains
2. **Percentage Calculation** — We calculate the minimum sample percentage needed to get representative data
3. **TABLESAMPLE Query** — The query uses `TABLESAMPLE SYSTEM (0.01 PERCENT)` instead of scanning everything

For a 100 million row table, getting 10 sample rows might scan only 0.001% of the data. That's the difference between a query that costs $0.50 and one that costs $0.0001.

The agent uses this automatically whenever it needs to preview data or understand table structure. You get the exploration you need without the exploration costs.

## Solution 4: Cost-Optimized AI Agent

Kyomi's AI agent is specifically designed to minimize BigQuery costs while still delivering accurate analysis. Here's how:

**Preview-First Architecture**

When the agent tests a query, it retrieves only 20 rows—just enough to verify the query works and the results make sense. The full dataset only loads when you view the final visualization. This means iterating on complex queries costs almost nothing.

**Intelligent Query Patterns**

The agent is trained to write cost-efficient SQL:

- **Wildcard tables over UNION ALL** — For partitioned tables like yearly data, the agent uses `table_*` wildcard syntax instead of expensive UNION ALL statements
- **Filter-first queries** — Date ranges and WHERE clauses are applied early to reduce scan size
- **Column selection** — The agent selects only the columns needed for analysis, not `SELECT *`

**Validation Before Execution**

Every query the agent generates goes through cost estimation first. If a query would exceed your configured limit, the agent adjusts its approach—adding filters, sampling data, or breaking the analysis into smaller pieces—before ever touching your billing.

This isn't just a safety net. It's how the agent thinks: cost-awareness is built into every step of the analysis workflow.

## How It Works Under the Hood

When you write or generate a query, here's what happens:

1. **Dry Run Request** — Kyomi sends your SQL to BigQuery's `jobs.query` endpoint with `dryRun: true`
2. **Instant Response** — BigQuery returns the estimated bytes to scan and validates syntax (no data is processed, no cost incurred)
3. **Cost Calculation** — Kyomi calculates the estimated cost at $6.25/TiB
4. **Limit Check** — For AI queries, we compare against your configured limit
5. **Block or Execute** — If under limit (or manual query), proceed; otherwise, block and explain

The entire validation takes about 200-400ms—fast enough to feel instant, thorough enough to prevent surprises.

## Best Practices for Cost-Effective BigQuery Analysis

Even with Kyomi's safeguards, here are some tips for keeping costs low:

### 1. Use Partitioned Tables
If your tables are partitioned by date, always include a date filter. Scanning `WHERE date >= '2024-01-01'` instead of the full table can reduce costs by 90%+.

### 2. Select Only What You Need
Avoid `SELECT *` when you only need a few columns. BigQuery is columnar—selecting fewer columns means scanning less data.

### 3. Start with Samples
When exploring a new dataset, ask the AI to "show me a sample of 1000 rows" before running aggregations on the full table.

### 4. Leverage Query Caching
BigQuery caches query results for 24 hours. If you run the same query twice, the second run is free. Kyomi shows when a query hits the cache:

> Cache hit: No bytes scanned

### 5. Set Appropriate Limits
Start with a conservative limit (10-20 GB) and increase it only when you need to analyze larger datasets. You can always override for specific queries.

## Your Data, Your Costs, Your Control

Kyomi connects directly to your BigQuery project using your Google Cloud credentials. This means:

- **You pay Google directly** for BigQuery usage—it appears in your GCP billing
- **Kyomi never stores your data** — queries run in your project, results stay in your browser
- **Full visibility** — check your GCP console anytime to see exactly what's been billed

We believe cost transparency is essential for data analytics. You shouldn't need a finance degree to understand your BigQuery bill, and you shouldn't be afraid to explore your data.

## Try It Yourself

Ready to analyze your BigQuery data without the bill anxiety? Kyomi's cost controls are available on all plans, including the free tier.

<div style="text-align: center; margin: 3rem 0; padding: 2rem; background: var(--color-muted); border-radius: 0.75rem;">
  <h3 style="margin-top: 0;">Start analyzing with confidence</h3>
  <p style="color: var(--color-muted-foreground); margin-bottom: 2rem;">Free tier includes full SQL editor with cost estimation. No credit card required.</p>
  <a href="https://app.kyomi.ai/login" class="cta-primary">
    Get Started Free →
  </a>
</div>

---

**Have questions about BigQuery costs or Kyomi's approach?** Reach out at [hello@kyomi.ai](mailto:hello@kyomi.ai)—we'd love to hear from you.

</div>

<div class="blog-post-footer">
  <a href="/blog" class="blog-back-link">← Back to Blog</a>
</div>

</div>
