---
layout: page
title: "Built-in Website Analytics and a Smarter Knowledge Base"
description: "Track your website traffic with one script tag, and teach Kyomi your business faster with the improved knowledge base"
---

<div class="blog-post">

<div class="blog-post-header">
  <h1>Built-in Website Analytics and a Smarter Knowledge Base</h1>
  <p class="blog-post-meta">February 22, 2026</p>
</div>

<div class="blog-post-content">

Two updates shipping today that make Kyomi more useful out of the box: **built-in website analytics** that works like any other datasource, and an **improved knowledge base** that helps Kyomi understand your business faster.

## Website Analytics: One Script Tag, Full AI Power

Most analytics tools give you a dashboard. Kyomi gives you an AI analyst that happens to know your traffic data.

Add a single `<script>` tag to your site and Kyomi starts collecting page views, referrers, devices, and geography. The data appears as a queryable datasource — same as BigQuery, PostgreSQL, or any of the other 9 platforms we support.

That means you can:

- **Ask questions in plain English** — "What pages get the most views?" or "Where is my traffic coming from this week?"
- **Build dashboards** — Create traffic dashboards with ChartML, combine website data with your warehouse data in multi-source charts
- **Set up alerts** — "Notify me if daily signups drop below 10" works the same way as any other Kyomi Watch

### Privacy by Default

The tracking script is ~1KB, loads asynchronously, and collects zero personal data:

- **No cookies** — no consent banners required
- **No raw IPs** — visitor hashing for unique counts, IPs never stored
- **No cross-site tracking** — no fingerprinting, no third-party data sharing

This isn't a stripped-down version of analytics. It's a different philosophy: collect only what you need, make it queryable with AI, and skip the privacy headaches.

### Included With Kyomi

Website analytics is included on every deployment — Cloud, self-hosted, and desktop. No separate subscription needed.

Get started in Settings > Analytics. Full setup guide: [Website Analytics docs](/docs/analytics).

---

## Improved Knowledge Base

The knowledge base is what makes Kyomi different from a generic SQL tool — it remembers your business. We've made several improvements to how Kyomi learns and applies that knowledge.

### Better Metric Definitions

When you tell Kyomi "MRR excludes trial accounts" or "active users means logged in within 30 days," those definitions are now applied more consistently across conversations, dashboards, and watches. Corrections stick better and propagate to your whole team.

### Smarter Table Discovery

Kyomi's understanding of your data catalog is sharper. When you ask a question, the AI is better at finding the right tables — even when your schema is complex or uses non-obvious naming conventions. Learnings like "customer data is in the `users` table, not `customers`" now carry more weight in table selection.

### Knowledge That Compounds

The core idea hasn't changed: every conversation teaches Kyomi something about your data. But with these improvements, that accumulated knowledge is more reliable and more useful — both for you and for everyone on your team.

---

## Get Started

**Website analytics:** Go to Settings > Analytics, create a site, add the script tag. Takes under a minute.

**Knowledge base:** Just keep using Kyomi. Correct it when it's wrong, teach it your metric definitions, and it gets smarter over time.

New to Kyomi? [Sign up free](https://app.kyomi.ai/register) — AI included, no credit card required.

</div>

<div class="blog-post-footer">
  <a href="/blog" class="blog-back-link">← Back to Blog</a>
</div>

</div>
