---
layout: page
title: The Explore-First Pattern for AI Data Agents
description: The breakthrough wasn't better prompts—it was letting the agent explore the data warehouse before writing queries.
---

<div class="blog-post">

<div class="blog-post-header">
  <h1>The Explore-First Pattern for AI Data Agents</h1>
  <p class="blog-post-meta">January 25, 2026 · Jason Adams</p>
</div>

<div class="blog-post-content">

When we built Kyomi, the breakthrough wasn't better prompts or smarter models—it was letting the agent explore the data warehouse before writing queries. Here's the pattern that made the difference.

## Why Exploration Matters

Data warehouses are discovered, not memorized.

When someone asks "what was our revenue last quarter?", the answer lives somewhere in your warehouse. But where? Maybe it's `sales.transactions`, or `finance.revenue_summary`, or `analytics.fact_orders`. The column might be called `revenue`, `total_amount`, `order_value`, or `gmv`.

An AI agent faces the same challenge a new analyst does on day one: the schema is unfamiliar, the naming conventions are inconsistent, and the only way to figure it out is to look around.

The explore-first pattern embraces this reality. Instead of expecting the agent to generate the perfect query immediately, we give it tools to investigate—and the freedom to iterate until it finds what it needs.

## The Explore-First Loop

The pattern follows a simple loop:

```
Search Catalog → Inspect Schema → Sample Data → Write Query
       ↑                                            |
       └─────────── iterate if needed ─────────────┘
```

Each step reduces uncertainty:

1. **Search Catalog** — Find candidate tables that might contain the data
2. **Inspect Schema** — Understand what columns exist and their types
3. **Sample Data** — See what values actually look like
4. **Write Query** — Now write the SQL with confidence

If the query doesn't return what's expected, the agent loops back—tries different search terms, inspects related tables, samples different columns. Most successful queries come after 2-3 exploration steps, not one.

## Semantic Search for Discovery

The first instinct might be to dump the entire schema into the prompt. "Here are all 500 tables, now answer the question."

This doesn't scale. Token limits aside, the noise drowns out the signal. The agent needs to find the 3-5 relevant tables, not wade through hundreds.

Semantic search solves this. Instead of exact keyword matching, we embed table names, column names, and descriptions into vectors. When the agent searches for "customer spending patterns", it finds `analytics.customer_lifetime_value` even though "spending" doesn't appear anywhere in the table name.

The implementation uses a multi-level weighting approach:

- **Table names** get the highest weight (most specific signal)
- **Table descriptions** come next (human-written context)
- **Column names** add detail (what data is actually available)
- **Column descriptions** provide additional context

When a user asks about "revenue", the search might return:

```
1. sales.transactions (0.89) - "Daily transaction records with order totals"
2. finance.monthly_summary (0.82) - "Aggregated monthly revenue by region"
3. analytics.arr_metrics (0.76) - "Annual recurring revenue calculations"
```

The agent now has a starting point—not a haystack.

## Inspecting the Schema

Once the agent has candidate tables, it needs to understand their structure. A simple `get_table_info` call returns:

```
Table: sales.transactions
Columns:
  - transaction_id (STRING) - Unique identifier
  - customer_id (STRING) - FK to customers table
  - order_total (NUMERIC) - Total order value in USD
  - transaction_date (DATE) - When the transaction occurred
  - status (STRING) - Transaction status code
  - region_code (STRING) - Sales region
```

Now the agent knows: revenue likely lives in `order_total`, and there's a `status` column that might need filtering.

But what values does `status` contain?

## Sampling: The Secret Weapon

This is where many agents go wrong. They see `status (STRING)` and assume values like `'completed'`, `'pending'`, `'cancelled'`. Then they write:

```sql
SELECT SUM(order_total)
FROM sales.transactions
WHERE status = 'completed'
```

And get zero rows back. Because the actual values are `'C'`, `'P'`, `'X'`.

Sampling prevents this. Before writing the real query, the agent runs:

```sql
SELECT DISTINCT status FROM sales.transactions LIMIT 20
```

And discovers:

```
status
------
C
P
X
R
```

Now it knows to ask: "What do these status codes mean?" Or it samples a few complete rows:

```sql
SELECT * FROM sales.transactions LIMIT 5
```

And sees that `'C'` rows have `completed_at` populated while `'P'` rows don't. Context emerges from the data itself.

This sampling step—just 5-20 rows—prevents entire categories of errors:

- **Enum values** that don't match expectations
- **Date formats** that need specific handling
- **Null patterns** that affect aggregations
- **Data quality issues** that need filtering

The key is keeping sample queries cheap. A `LIMIT 20` with no aggregation returns almost instantly and fits easily in the context window.

## Writing the Query with Confidence

After exploration, the agent has:

- Found the right table (`sales.transactions`)
- Understood the schema (`order_total` for revenue, `transaction_date` for time)
- Discovered actual values (`status = 'C'` for completed)

Now it can write:

```sql
SELECT
  DATE_TRUNC('month', transaction_date) as month,
  SUM(order_total) as revenue
FROM sales.transactions
WHERE status = 'C'
  AND transaction_date >= DATE_TRUNC('quarter', CURRENT_DATE - INTERVAL '3 months')
  AND transaction_date < DATE_TRUNC('quarter', CURRENT_DATE)
GROUP BY 1
ORDER BY 1
```

The query is grounded in reality, not assumptions.

## Building on What You Learn

The explore-first pattern gets more powerful over time when the agent can remember what it discovers.

After figuring out that `status = 'C'` means completed, the agent saves this as workspace knowledge:

> "In sales.transactions, the status column uses single-letter codes: C=completed, P=pending, X=cancelled, R=refunded"

The next time anyone asks about completed transactions, the agent doesn't need to sample—it already knows.

This creates a compounding effect. Each exploration teaches the agent something about your data warehouse:

- Which tables to use for which questions
- What columns actually mean
- How values are encoded
- Which joins make sense

Over weeks of usage, the agent develops the kind of institutional knowledge that usually lives only in senior analysts' heads.

The key is being selective about what to remember. Not "revenue was $1.2M last quarter" (that's a result, not knowledge), but "revenue data lives in sales.transactions.order_total, filtered by status='C'" (that's reusable navigation knowledge).

## Implementing the Pattern

The pattern requires a few capabilities:

**1. A search tool** that finds relevant tables semantically:

```python
def search_catalog(query: str, limit: int = 5) -> list[Table]:
    """Find tables matching a natural language query."""
    # Embed the query
    # Search against table/column embeddings
    # Return top matches with relevance scores
```

**2. A schema inspection tool** that returns structure:

```python
def get_table_info(table_name: str) -> TableSchema:
    """Get detailed schema for a specific table."""
    # Return columns, types, descriptions
    # Include row count estimate if available
```

**3. A query tool** with built-in limits for sampling:

```python
def query_datasource(sql: str) -> QueryResult:
    """Execute SQL and return results (max 20 rows for exploration)."""
    # Validate SQL syntax first (dry-run)
    # Execute with row limit
    # Return data + metadata
```

**4. A learning tool** to persist discoveries:

```python
def save_learning(insight: str, context: str) -> None:
    """Save reusable knowledge about the data warehouse."""
    # Validate it's navigation knowledge, not analysis results
    # Embed for future semantic retrieval
    # Store with workspace scope
```

The agent's system prompt encourages exploration:

> "You have tools to search for tables, inspect schemas, and run queries. Use them iteratively. Search first, then inspect what you find, then sample data to understand values before writing your final query. If something doesn't work, try a different approach."

## The Mindset Shift

The explore-first pattern represents a shift in how we think about AI data agents.

Instead of "generate the perfect SQL in one shot," it's "investigate the warehouse like an analyst would."

Instead of "memorize the entire schema," it's "search for what's relevant."

Instead of "assume standard values," it's "look at the actual data."

This makes the agent more robust to messy real-world warehouses—the kind with legacy naming, undocumented columns, and status codes that only make sense if you've been at the company for five years.

It also makes the agent more trustworthy. When you can see it searching, sampling, and reasoning about what it finds, you understand why it wrote the query it did. The exploration is part of the explanation.

---

*This pattern emerged from building [Kyomi](https://kyomi.ai), a data intelligence platform that connects to your warehouse and answers questions in plain English. The explore-first approach is core to how it works.*

</div>
</div>
