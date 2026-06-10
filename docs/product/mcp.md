---
layout: doc
title: MCP Integration - Kyomi in Your IDE
description: Connect Kyomi to Cursor and Claude Code for AI-powered data analysis in your development environment
---

# MCP Integration

Access your data intelligence directly from Cursor or Claude Code. Query your data warehouse, search your catalog, and leverage your team's accumulated knowledge—all without leaving your IDE.

## What is MCP?

The **Model Context Protocol (MCP)** is an open standard that lets AI assistants connect to external tools and data sources. Kyomi's MCP server exposes your data intelligence to any MCP-compatible client.

## Available Tools

When you connect Kyomi via MCP, your AI assistant gains access to:

### Data Catalog
- **search_catalog** - Semantic search across all your connected datasources. Ask "find customer revenue tables" and get instant results.
- **get_table_info** - Get detailed schema information for any table, including column types and descriptions.
- **list_datasources** - See all datasources connected to your workspace.

### Query Execution
- **query_datasource** - Run SQL queries against BigQuery, Snowflake, PostgreSQL, and 6 other platforms.
- **validate_sql** - Check query syntax before executing.

### Knowledge Base
- **search_learnings** - Access your team's accumulated knowledge about your data.
- **save_learning** - Capture new insights about tables, metrics, or business rules.

### Dashboards
- **search_dashboards** - Find existing dashboards by content or title.
- **get_dashboard_info** - View dashboard details and ChartML content.
- **create_dashboard** - Create new dashboards with ChartML visualizations.

### Watches
- **search_watches** - Find existing data monitors.
- **create_watch** - Set up automated data monitoring with alerts.

### Website Analytics
- **create_analytics_site** - Set up website analytics tracking. Returns a `<script>` snippet and provisions a queryable datasource.
- **list_analytics_sites** - View all analytics sites and their tracking snippets.
- **update_analytics_site** - Change site name or allowed domains.
- **delete_analytics_site** - Remove an analytics site and all its data.

## Connecting from Cursor

1. Go to **Settings > Profile** in Kyomi
2. Click **Connect with Cursor**
3. Cursor will prompt you to confirm the installation
4. Authorize Kyomi when prompted in your browser

That's it! Kyomi's tools are now available in Cursor.

## Connecting from Claude Code

Run this command in your terminal:

```bash
claude mcp add kyomi --transport http https://app.kyomi.ai/mcp
```

Then restart Claude Code. When you first use a Kyomi tool, you'll be prompted to authorize the connection in your browser.

## Manual Configuration

If you prefer to edit your MCP config file directly, add this to your configuration:

```json
{
  "mcpServers": {
    "kyomi": {
      "type": "http",
      "url": "https://app.kyomi.ai/mcp"
    }
  }
}
```

**Config file locations:**
- **Cursor**: `~/.cursor/mcp.json`
- **Claude Code**: `~/.claude/claude_desktop_config.json`

## Example Usage

Once connected, you can ask your AI assistant questions like:

> "Search for tables related to customer orders"

> "What columns are in the sales.transactions table?"

> "Run a query to get yesterday's revenue by product category"

> "What does our team know about calculating MRR?"

> "Create a dashboard showing weekly active users trend"

> "Set up website analytics for my-app.com and give me the tracking snippet"

> "How much traffic did my site get this week?"

The AI will use Kyomi's tools to answer your questions, with full access to your data warehouse and your team's accumulated knowledge.

## Authentication

- **Browser-based OAuth** - When you first connect, you'll authorize via your browser
- **Automatic refresh** - Tokens refresh automatically, no need to reconnect
- **Same permissions** - MCP uses your existing Kyomi credentials and datasource access

## Security

- All queries use your credentials and respect datasource permissions
- Same 20-row preview limit as the Kyomi web app for AI queries
- Workspace isolation - you only see your workspace's data
- Disconnect anytime from Settings > Profile

## Availability

MCP integration is available on **Pro**, **Team**, and **Enterprise** plans.

---

## Troubleshooting

### "Unauthorized" errors
Your token may have expired. Disconnect and reconnect from Settings > Profile.

### Tools not appearing
Make sure you've authorized Kyomi in your browser. Check that your subscription includes MCP access.

### Connection timeout
Verify your network can reach app.kyomi.ai. Some corporate firewalls may block MCP connections.
