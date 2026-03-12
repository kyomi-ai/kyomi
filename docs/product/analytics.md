---
layout: doc
title: Website Analytics - Built-in Traffic Tracking
description: Set up privacy-focused website analytics with one script tag
---

# Website Analytics

Kyomi includes built-in website analytics — a lightweight, privacy-focused alternative to Google Analytics. Add one script tag and start tracking page views, referrers, devices, and more. Your traffic data appears as a queryable datasource, so you can ask questions in plain English, build dashboards, and set up alerts just like any other data source.

## Quick Start

The fastest way to set up analytics is through your coding agent. If you use Claude Code, Cursor, or another MCP-compatible tool with Kyomi connected, just ask:

> "Add website analytics to my site and build me a traffic dashboard"

Your agent creates the analytics site, inserts the tracking snippet into your HTML `<head>`, and builds a dashboard — all in one step. No copy-pasting, no switching between tools.

### What your agent does

1. **Creates the analytics site** — provisions a ClickHouse datasource scoped to your workspace
2. **Adds the script tag to your code** — finds your HTML layout and inserts the ~1KB tracking snippet
3. **Indexes the schema** — the `events` table becomes searchable in your data catalog
4. **Builds a dashboard** — creates a traffic dashboard with charts tailored to your site

### Setting up from the web UI

You can also set up analytics from the Kyomi web app:

1. Open **Settings** in the sidebar
2. Go to the **Analytics** tab
3. Click **Create Analytics Site**
4. Enter your website's domain (e.g., `example.com`)
5. Copy the generated tracking snippet into your site's `<head>`

### Start asking questions

Once events flow in, query your traffic like any other datasource — through your agent or the Kyomi chat:

- "How many page views did I get this week?"
- "What are my top pages by traffic?"
- "Where is my traffic coming from?"
- "Show me a breakdown of devices and browsers"
- "Compare this week's traffic to last week"

## What's Tracked

| Data Point | Example | Notes |
|-----------|---------|-------|
| Page URL | `/blog/my-post` | Path and query string |
| Referrer | `google.com` | Where visitors came from |
| Country | `US`, `AU` | Derived from IP, IP is never stored |
| Device type | `Desktop`, `Mobile` | Parsed from user agent |
| Browser | `Chrome`, `Safari` | Parsed from user agent |
| OS | `Windows`, `macOS` | Parsed from user agent |
| Screen size | `1920x1080` | Viewport dimensions |
| UTM parameters | `utm_source=twitter` | Campaign tracking |

### What's NOT Tracked

- No cookies
- No raw IP addresses (hashed for unique visitor counts, then discarded)
- No personal data
- No cross-site tracking
- No fingerprinting

This means most privacy regulations (GDPR, CCPA) don't require consent banners for Kyomi Analytics.

## Building Dashboards

Your analytics data is a regular Kyomi datasource. You can build dashboards with ChartML:

````yaml
```chartml
data:
  source: my-analytics-site
  query: |
    SELECT
      toDate(timestamp) as date,
      count(*) as page_views,
      uniqExact(visitor_hash) as unique_visitors
    FROM events
    WHERE timestamp >= now() - INTERVAL 30 DAY
    GROUP BY date
    ORDER BY date
visualize:
  mark: line
  columns: date
  rows: [page_views, unique_visitors]
  style:
    title: Traffic — Last 30 Days
```
````

## Setting Up Alerts

Combine analytics with [Kyomi Watch](/docs/watches) to monitor your traffic. Ask your agent or Kyomi chat:

- "Alert me if daily page views drop more than 20% compared to last week"
- "Send me a weekly traffic summary every Monday morning"
- "Notify me when a page gets more than 1,000 views in a day"

Your agent can create these watches directly via MCP — no need to switch to the web UI.

## Event Quotas by Plan

| Plan | Events/Month | Data Retention |
|------|-------------|----------------|
| Free | 50,000 | 30 days |
| Starter | 1,000,000 | 180 days |
| Pro | 5,000,000 | 1 year |
| Team | 25,000,000 | 2 years |

When you approach your event limit, Kyomi shows a usage indicator in settings. Events beyond the limit are dropped until the next billing cycle.

## Custom Events

You can track custom events beyond page views using the JavaScript API:

```javascript
// Track a button click
kyomi.event('signup_click', { plan: 'pro' });

// Track a form submission
kyomi.event('contact_form', { source: 'pricing_page' });
```

Custom events appear in the same `events` table and can be filtered by `event_name`.

## Managing Your Analytics Site

### Changing your domain

Ask your agent to update the domain:

> "Update my analytics site to also allow staging.example.com"

Or go to **Settings > Analytics** in the web UI. Note: changing domains regenerates the tracking snippet — you'll need to update the `<script>` tag on your site.

### Deleting your site

> "Delete the analytics site for example.com"

This **permanently deletes all collected event data** and cannot be undone. You can also delete from **Settings > Analytics** in the web UI.

## Privacy & Compliance

- **You are the data controller** — you decide to deploy the tracking script
- **Kyomi is the data processor** — we collect and store anonymized data on your behalf
- **No consent banner needed** — no cookies or personal data means most regulations don't require opt-in
- **Your responsibility** — ensure compliance with privacy laws applicable to your website and visitors

See our [Privacy Policy](/privacy) for full details on how analytics data is handled.

## Troubleshooting

### Events not appearing?

1. **Check the script tag** — make sure it's in the `<head>` and the `data-key` matches your site
2. **Check the domain** — the script only sends events when the page domain matches your configured domain
3. **Wait a few minutes** — events are batched and may take 1-2 minutes to appear
4. **Check your ad blocker** — some ad blockers may block analytics scripts. The script loads from `analytics.kyomi.ai`

### Visitor counts seem low?

Kyomi uses IP-based hashing (without storing IPs) for unique visitor counts. This is less accurate than cookie-based tracking but more privacy-friendly. VPN users and shared IP addresses may be counted as one visitor.
