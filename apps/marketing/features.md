---
layout: page
title: Features
description: Everything you need to analyze data with AI
---

<div class="features-page">

<!-- Hero -->
<div style="text-align: center; padding-top: 3rem; margin-bottom: 2rem;">
  <h1 style="font-size: 2.5rem; font-weight: 700; margin-bottom: 0.5rem;">From Question to Insight in Seconds</h1>
  <p style="font-size: 1.25rem; color: var(--color-muted-foreground); max-width: 40rem; margin: 0 auto;">A natural language data analytics platform and AI data assistant that learns your business, answers your questions, and watches your data while you sleep.</p>
</div>

<!-- Security Trust Banner -->
<div style="max-width: 48rem; margin: 0 auto 3rem; padding: 1.25rem 1.5rem; background: linear-gradient(135deg, #f0fdf4 0%, #dcfce7 100%); border: 1px solid #86efac; border-radius: 0.75rem;">
  <div style="display: flex; align-items: flex-start; gap: 1rem;">
    <div style="width: 2.25rem; height: 2.25rem; background: #166534; border-radius: 0.5rem; display: flex; align-items: center; justify-content: center; flex-shrink: 0;">
      <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="white" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
        <rect x="3" y="11" width="18" height="11" rx="2" ry="2"></rect>
        <path d="M7 11V7a5 5 0 0 1 10 0v4"></path>
      </svg>
    </div>
    <div>
      <strong style="color: #166534;">Your data never leaves your warehouse.</strong>
      <span style="color: #15803d; font-size: 0.9rem;"> Kyomi queries directly, AI sees max 20 rows, you control access with existing permissions.</span>
      <br/><span style="color: #15803d; font-size: 0.9rem;"><strong>Need even more control?</strong> Deploy <a href="/docs/connect/" style="color: #166534; text-decoration: underline;">Kyomi Connect</a> inside your network — credentials stay on your infrastructure, only query results travel to Kyomi. Connect is <a href="https://github.com/kyomi-ai/kyomi-connect" style="color: #166534; text-decoration: underline;">open-source</a> (Apache 2.0) — audit every line of code yourself.</span>
    </div>
  </div>
</div>

<!-- Datasources -->
<div style="text-align: center; margin-bottom: 3rem;">
  <p style="font-size: 0.8rem; color: var(--color-muted-foreground); margin-bottom: 0.5rem;">Works with your existing data stack</p>
  <p style="font-size: 0.9rem; color: var(--color-foreground);">
    BigQuery · Snowflake · PostgreSQL · MySQL · ClickHouse · Redshift · Databricks · SQL Server · Azure Synapse
  </p>
</div>

<!-- PILLAR 1: Ask Questions, Get Answers -->
<div style="margin: 3rem 0; padding: 2.5rem; background: var(--color-muted); border-radius: 1rem;">
  <div style="display: flex; align-items: center; gap: 0.75rem; margin-bottom: 1rem;">
    <div style="width: 2.25rem; height: 2.25rem; background: var(--color-primary); border-radius: 0.5rem; display: flex; align-items: center; justify-content: center; flex-shrink: 0;">
      <svg width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="white" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
        <path d="M21 15a2 2 0 0 1-2 2H7l-4 4V5a2 2 0 0 1 2-2h14a2 2 0 0 1 2 2z"></path>
      </svg>
    </div>
    <h2 style="font-size: 1.5rem; font-weight: 700; margin: 0;">Ask Questions, Get Answers</h2>
  </div>

  <p style="color: var(--color-muted-foreground); font-size: 1.05rem; margin-bottom: 1.5rem; max-width: 40rem;">
    No SQL required. No waiting on analysts. This self-service analytics tool lets anyone ask questions in plain English and get instant visualizations.
  </p>

  <div style="display: grid; grid-template-columns: repeat(auto-fit, minmax(250px, 1fr)); gap: 1rem; margin-bottom: 1.5rem;">
    <div style="background: var(--color-background); border-radius: 0.5rem; padding: 1.25rem; border: 1px solid var(--color-border);">
      <strong style="color: var(--color-primary);">Natural Language Queries</strong>
      <p style="margin: 0.5rem 0 0; font-size: 0.875rem; color: var(--color-muted-foreground);">
        "Show me revenue by region last quarter" — Kyomi's text-to-SQL engine finds the right tables, writes optimized queries, and generates charts automatically.
      </p>
    </div>
    <div style="background: var(--color-background); border-radius: 0.5rem; padding: 1.25rem; border: 1px solid var(--color-border);">
      <strong style="color: var(--color-primary);">Automatic Visualizations</strong>
      <p style="margin: 0.5rem 0 0; font-size: 0.875rem; color: var(--color-muted-foreground);">
        Charts appear alongside your data. Forecast lines with confidence bands, multi-source charts combining data from different databases, and all the standard types — beautifully rendered and interactive.
      </p>
    </div>
    <div style="background: var(--color-background); border-radius: 0.5rem; padding: 1.25rem; border: 1px solid var(--color-border);">
      <strong style="color: var(--color-primary);">One-Click Dashboards</strong>
      <p style="margin: 0.5rem 0 0; font-size: 0.875rem; color: var(--color-muted-foreground);">
        Found an insight? Save it to a dashboard instantly. No rebuilding, no exporting.
      </p>
    </div>
  </div>

  <div style="margin-top: 1.5rem;">
    <img src="/images/slack-mention-chart.png" alt="Asking Kyomi a question in Slack and getting a chart response" style="width: 100%; max-width: 500px; border-radius: 0.5rem; border: 1px solid var(--color-border);" />
  </div>
</div>

<!-- PILLAR 2: Production Dashboards -->
<div style="margin: 3rem 0; padding: 2.5rem; background: var(--color-muted); border-radius: 1rem;">
  <div style="display: flex; align-items: center; gap: 0.75rem; margin-bottom: 1rem;">
    <div style="width: 2.25rem; height: 2.25rem; background: var(--color-primary); border-radius: 0.5rem; display: flex; align-items: center; justify-content: center; flex-shrink: 0;">
      <svg width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="white" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
        <rect x="3" y="3" width="18" height="18" rx="2" ry="2"></rect>
        <line x1="3" y1="9" x2="21" y2="9"></line>
        <line x1="9" y1="21" x2="9" y2="9"></line>
      </svg>
    </div>
    <h2 style="font-size: 1.5rem; font-weight: 700; margin: 0;">Production-Ready Dashboards</h2>
  </div>

  <p style="color: var(--color-muted-foreground); font-size: 1.05rem; margin-bottom: 1.5rem; max-width: 40rem;">
    Full dashboard builder with AI copilot. Edit code or use the visual builder — your choice.
  </p>

  <div style="display: grid; grid-template-columns: repeat(auto-fit, minmax(250px, 1fr)); gap: 1rem; margin-bottom: 1.5rem;">
    <div style="background: var(--color-background); border-radius: 0.5rem; padding: 1.25rem; border: 1px solid var(--color-border);">
      <strong style="color: var(--color-primary);">AI Dashboard Copilot</strong>
      <p style="margin: 0.5rem 0 0; font-size: 0.875rem; color: var(--color-muted-foreground);">
        Describe what you want, copilot builds it. "Add a chart showing breakdown by country" — done.
      </p>
    </div>
    <div style="background: var(--color-background); border-radius: 0.5rem; padding: 1.25rem; border: 1px solid var(--color-border);">
      <strong style="color: var(--color-primary);">Code-Based Control</strong>
      <p style="margin: 0.5rem 0 0; font-size: 0.875rem; color: var(--color-muted-foreground);">
        Dashboards are Markdown + ChartML. No proprietary formats, no vendor lock-in. Full control.
      </p>
    </div>
    <div style="background: var(--color-background); border-radius: 0.5rem; padding: 1.25rem; border: 1px solid var(--color-border);">
      <strong style="color: var(--color-primary);">Version History</strong>
      <p style="margin: 0.5rem 0 0; font-size: 0.875rem; color: var(--color-muted-foreground);">
        Every save creates a version. Preview, compare, restore with one click. Never lose work.
      </p>
    </div>
    <div style="background: var(--color-background); border-radius: 0.5rem; padding: 1.25rem; border: 1px solid var(--color-border);">
      <strong style="color: var(--color-primary);">Built-in Forecasting</strong>
      <p style="margin: 0.5rem 0 0; font-size: 0.875rem; color: var(--color-muted-foreground);">
        Forecast trends with confidence intervals. Ask about future metrics and get predictions with uncertainty bands — no Python notebooks required.
      </p>
    </div>
    <div style="background: var(--color-background); border-radius: 0.5rem; padding: 1.25rem; border: 1px solid var(--color-border);">
      <strong style="color: var(--color-primary);">PDF Export</strong>
      <p style="margin: 0.5rem 0 0; font-size: 0.875rem; color: var(--color-muted-foreground);">
        Download any dashboard as a professional PDF. High-resolution charts, clean formatting, page numbers — ready to email to executives.
      </p>
    </div>
  </div>

  <div style="margin-top: 1.5rem;">
    <img src="/images/dashboard-editor.png" alt="Dashboard editor with AI copilot" style="width: 100%; border-radius: 0.5rem; border: 1px solid var(--color-border);" />
  </div>
</div>

<!-- PILLAR 3: Learns Your Business -->
<div style="margin: 3rem 0; padding: 2.5rem; background: var(--color-muted); border-radius: 1rem;">
  <div style="display: flex; align-items: center; gap: 0.75rem; margin-bottom: 1rem;">
    <div style="width: 2.25rem; height: 2.25rem; background: var(--color-primary); border-radius: 0.5rem; display: flex; align-items: center; justify-content: center; flex-shrink: 0;">
      <svg width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="white" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
        <path d="M12 2a4 4 0 0 0-4 4v2H6a2 2 0 0 0-2 2v10a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2V10a2 2 0 0 0-2-2h-2V6a4 4 0 0 0-4-4z"></path>
        <circle cx="12" cy="14" r="2"></circle>
      </svg>
    </div>
    <h2 style="font-size: 1.5rem; font-weight: 700; margin: 0;">Learns Your Business</h2>
  </div>

  <p style="color: var(--color-muted-foreground); font-size: 1.05rem; margin-bottom: 1.5rem; max-width: 40rem;">
    Kyomi remembers what "revenue" means in YOUR business. Every conversation makes it smarter.
  </p>

  <div style="display: grid; grid-template-columns: repeat(auto-fit, minmax(250px, 1fr)); gap: 1rem;">
    <div style="background: var(--color-background); border-radius: 0.5rem; padding: 1.25rem; border: 1px solid var(--color-border);">
      <strong style="color: var(--color-primary);">Metric Definitions</strong>
      <p style="margin: 0.5rem 0 0; font-size: 0.875rem; color: var(--color-muted-foreground);">
        Define formulas once — "MRR excludes trials" — Kyomi applies them consistently everywhere.
      </p>
    </div>
    <div style="background: var(--color-background); border-radius: 0.5rem; padding: 1.25rem; border: 1px solid var(--color-border);">
      <strong style="color: var(--color-primary);">Table Knowledge</strong>
      <p style="margin: 0.5rem 0 0; font-size: 0.875rem; color: var(--color-muted-foreground);">
        "Customer data is in users table, not customers" — saved once, never forgotten.
      </p>
    </div>
    <div style="background: var(--color-background); border-radius: 0.5rem; padding: 1.25rem; border: 1px solid var(--color-border);">
      <strong style="color: var(--color-primary);">Business Context</strong>
      <p style="margin: 0.5rem 0 0; font-size: 0.875rem; color: var(--color-muted-foreground);">
        "Exclude @test.com from user counts" — your rules, automatically applied.
      </p>
    </div>
  </div>

  <p style="margin-top: 1.5rem; font-size: 0.95rem; color: var(--color-foreground);">
    New team members get instant access to years of accumulated data knowledge. No more asking "which table has customer data?"
  </p>
</div>

<!-- PILLAR 4: Watches Your Data -->
<div style="margin: 3rem 0; padding: 2.5rem; background: var(--color-muted); border-radius: 1rem;">
  <div style="display: flex; align-items: center; gap: 0.75rem; margin-bottom: 1rem;">
    <div style="width: 2.25rem; height: 2.25rem; background: var(--color-primary); border-radius: 0.5rem; display: flex; align-items: center; justify-content: center; flex-shrink: 0;">
      <svg width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="white" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
        <path d="M1 12s4-8 11-8 11 8 11 8-4 8-11 8-11-8-11-8z"></path>
        <circle cx="12" cy="12" r="3"></circle>
      </svg>
    </div>
    <h2 style="font-size: 1.5rem; font-weight: 700; margin: 0;">Watches Your Data 24/7</h2>
  </div>

  <p style="color: var(--color-muted-foreground); font-size: 1.05rem; margin-bottom: 1.5rem; max-width: 40rem;">
    AI agents monitor your data and alert you when something needs attention. No dashboards to check.
  </p>

  <div style="display: grid; grid-template-columns: repeat(auto-fit, minmax(250px, 1fr)); gap: 1rem; margin-bottom: 1.5rem;">
    <div style="background: var(--color-background); border-radius: 0.5rem; padding: 1.25rem; border: 1px solid var(--color-border);">
      <strong style="color: var(--color-primary);">Plain English Alerts</strong>
      <p style="margin: 0.5rem 0 0; font-size: 0.875rem; color: var(--color-muted-foreground);">
        "Alert me if revenue drops more than 10%" — describe what matters, Kyomi handles the rest.
      </p>
    </div>
    <div style="background: var(--color-background); border-radius: 0.5rem; padding: 1.25rem; border: 1px solid var(--color-border);">
      <strong style="color: var(--color-primary);">Scheduled Reports</strong>
      <p style="margin: 0.5rem 0 0; font-size: 0.875rem; color: var(--color-muted-foreground);">
        "Send me a weekly sales summary every Monday" — automatic reports on your schedule.
      </p>
    </div>
    <div style="background: var(--color-background); border-radius: 0.5rem; padding: 1.25rem; border: 1px solid var(--color-border);">
      <strong style="color: var(--color-primary);">Smart Notifications</strong>
      <p style="margin: 0.5rem 0 0; font-size: 0.875rem; color: var(--color-muted-foreground);">
        Alerts via Slack or email. Only notified when something is actually noteworthy.
      </p>
    </div>
  </div>

  <div style="margin-top: 1.5rem;">
    <img src="/images/slack-alert.png" alt="Kyomi Watch alert configuration in Slack" style="width: 100%; max-width: 500px; border-radius: 0.5rem; border: 1px solid var(--color-border);" />
  </div>
</div>

<!-- PILLAR 4.5: Built-in Website Analytics -->
<div style="margin: 3rem 0; padding: 2.5rem; background: var(--color-muted); border-radius: 1rem;">
  <div style="display: flex; align-items: center; gap: 0.75rem; margin-bottom: 1rem;">
    <div style="width: 2.25rem; height: 2.25rem; background: var(--color-primary); border-radius: 0.5rem; display: flex; align-items: center; justify-content: center; flex-shrink: 0;">
      <svg width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="white" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
        <path d="M18 20V10"></path>
        <path d="M12 20V4"></path>
        <path d="M6 20v-6"></path>
      </svg>
    </div>
    <h2 style="font-size: 1.5rem; font-weight: 700; margin: 0;">Built-in Website Analytics</h2>
  </div>

  <p style="color: var(--color-muted-foreground); font-size: 1.05rem; margin-bottom: 1.5rem; max-width: 40rem;">
    Privacy-focused website analytics built right into Kyomi. One script tag, no cookies, and the same AI answers questions about your traffic.
  </p>

  <div style="display: grid; grid-template-columns: repeat(auto-fit, minmax(250px, 1fr)); gap: 1rem; margin-bottom: 1.5rem;">
    <div style="background: var(--color-background); border-radius: 0.5rem; padding: 1.25rem; border: 1px solid var(--color-border);">
      <strong style="color: var(--color-primary);">Lightweight & Private</strong>
      <p style="margin: 0.5rem 0 0; font-size: 0.875rem; color: var(--color-muted-foreground);">
        ~1KB tracking script. No cookies, no personal data, no consent banners. IP-based visitor hashing that never stores raw IPs.
      </p>
    </div>
    <div style="background: var(--color-background); border-radius: 0.5rem; padding: 1.25rem; border: 1px solid var(--color-border);">
      <strong style="color: var(--color-primary);">AI-Powered Traffic Insights</strong>
      <p style="margin: 0.5rem 0 0; font-size: 0.875rem; color: var(--color-muted-foreground);">
        Ask "where is my traffic coming from?" or "which blog posts convert best?" — the same AI that queries your data warehouse answers traffic questions too.
      </p>
    </div>
    <div style="background: var(--color-background); border-radius: 0.5rem; padding: 1.25rem; border: 1px solid var(--color-border);">
      <strong style="color: var(--color-primary);">Auto-Provisioned Datasource</strong>
      <p style="margin: 0.5rem 0 0; font-size: 0.875rem; color: var(--color-muted-foreground);">
        Create an analytics site and it appears as a queryable datasource. Write SQL against your traffic data, build dashboards, set up alerts — just like any other datasource.
      </p>
    </div>
    <div style="background: var(--color-background); border-radius: 0.5rem; padding: 1.25rem; border: 1px solid var(--color-border);">
      <strong style="color: var(--color-primary);">Included on Every Plan</strong>
      <p style="margin: 0.5rem 0 0; font-size: 0.875rem; color: var(--color-muted-foreground);">
        Free tier includes 50K events/month. Scale up to 25M events/month on Team. No separate analytics subscription needed.
      </p>
    </div>
  </div>
</div>

<!-- PILLAR 5: Works Where You Work -->
<div style="margin: 3rem 0; padding: 2.5rem; background: var(--color-muted); border-radius: 1rem;">
  <div style="display: flex; align-items: center; gap: 0.75rem; margin-bottom: 1rem;">
    <div style="width: 2.25rem; height: 2.25rem; background: var(--color-primary); border-radius: 0.5rem; display: flex; align-items: center; justify-content: center; flex-shrink: 0;">
      <svg width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="white" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
        <rect x="2" y="3" width="20" height="14" rx="2" ry="2"></rect>
        <line x1="8" y1="21" x2="16" y2="21"></line>
        <line x1="12" y1="17" x2="12" y2="21"></line>
      </svg>
    </div>
    <h2 style="font-size: 1.5rem; font-weight: 700; margin: 0;">Works Where You Work</h2>
  </div>

  <p style="color: var(--color-muted-foreground); font-size: 1.05rem; margin-bottom: 1.5rem; max-width: 40rem;">
    Slack, Claude Code, Cursor — same intelligence, same context, wherever you are.
  </p>

  <div style="display: grid; grid-template-columns: repeat(auto-fit, minmax(250px, 1fr)); gap: 1rem;">
    <div style="background: var(--color-background); border-radius: 0.5rem; padding: 1.25rem; border: 1px solid var(--color-border);">
      <strong style="color: var(--color-primary);">Slack Integration</strong>
      <p style="margin: 0.5rem 0 0; font-size: 0.875rem; color: var(--color-muted-foreground);">
        @kyomi in any channel. Charts render in threads. Conversations sync to web app.
      </p>
    </div>
    <div style="background: var(--color-background); border-radius: 0.5rem; padding: 1.25rem; border: 1px solid var(--color-border);">
      <strong style="color: var(--color-primary);">MCP for Claude Code / Cursor</strong>
      <p style="margin: 0.5rem 0 0; font-size: 0.875rem; color: var(--color-muted-foreground);">
        Query your data while coding. Search catalog, run queries, save learnings — without leaving your IDE.
      </p>
    </div>
    <div style="background: var(--color-background); border-radius: 0.5rem; padding: 1.25rem; border: 1px solid var(--color-border);">
      <strong style="color: var(--color-primary);">Context That Follows You</strong>
      <p style="margin: 0.5rem 0 0; font-size: 0.875rem; color: var(--color-muted-foreground);">
        Your learnings and business context work everywhere — web, Slack, or Claude Code.
      </p>
    </div>
  </div>

  <div style="margin-top: 1.5rem;">
    <img src="/images/claude-code-dashboard.png" alt="Creating a dashboard from Claude Code using Kyomi MCP integration" style="width: 100%; border-radius: 0.5rem; border: 1px solid var(--color-border);" />
    <p style="font-size: 0.8rem; color: var(--color-muted-foreground); margin-top: 0.5rem; text-align: center;">Creating a dashboard directly from Claude Code via MCP</p>
  </div>
</div>

<!-- Quick Features Table -->
<div style="margin: 4rem 0;">
  <h2 style="text-align: center; margin-bottom: 2rem;">Everything Else You Need in an AI Analytics Tool</h2>

  <div style="display: grid; grid-template-columns: repeat(auto-fit, minmax(280px, 1fr)); gap: 2rem;">
    <div>
      <h3 style="font-size: 1rem; margin-bottom: 0.75rem;">Query Management</h3>
      <ul style="margin: 0; padding-left: 1.25rem; color: var(--color-muted-foreground); font-size: 0.9rem;">
        <li>Full query history with search</li>
        <li>Save and star favorite queries</li>
        <li>Cost estimation with scan limits</li>
      </ul>
    </div>
    <div>
      <h3 style="font-size: 1rem; margin-bottom: 0.75rem;">Team Collaboration</h3>
      <ul style="margin: 0; padding-left: 1.25rem; color: var(--color-muted-foreground); font-size: 0.9rem;">
        <li>Multi-user workspaces</li>
        <li>Shared dashboards and knowledge</li>
        <li>Team AI usage pools</li>
      </ul>
    </div>
    <div>
      <h3 style="font-size: 1rem; margin-bottom: 0.75rem;">Security</h3>
      <ul style="margin: 0; padding-left: 1.25rem; color: var(--color-muted-foreground); font-size: 0.9rem;">
        <li>Kyomi Connect: <a href="https://github.com/kyomi-ai/kyomi-connect">open-source</a>, credentials never leave your network</li>
        <li>OAuth for cloud platforms</li>
        <li>Encrypted credentials at rest</li>
        <li>Read-only queries, audit logging</li>
      </ul>
    </div>
  </div>
</div>

<!-- Datasource Support -->
<div style="margin: 3rem 0; padding: 2rem; border: 1px solid var(--color-border); border-radius: 0.75rem;">
  <h3 style="text-align: center; margin-bottom: 1.5rem;">Same Features Across All 9 Datasources</h3>
  <div style="display: grid; grid-template-columns: repeat(auto-fit, minmax(200px, 1fr)); gap: 0.75rem; font-size: 0.875rem;">
    <div style="display: flex; align-items: center; gap: 0.5rem;">
      <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="var(--color-success-foreground)" stroke-width="3"><polyline points="20 6 9 17 4 12"></polyline></svg>
      AI natural language queries
    </div>
    <div style="display: flex; align-items: center; gap: 0.5rem;">
      <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="var(--color-success-foreground)" stroke-width="3"><polyline points="20 6 9 17 4 12"></polyline></svg>
      Automatic chart generation
    </div>
    <div style="display: flex; align-items: center; gap: 0.5rem;">
      <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="var(--color-success-foreground)" stroke-width="3"><polyline points="20 6 9 17 4 12"></polyline></svg>
      Dashboard creation
    </div>
    <div style="display: flex; align-items: center; gap: 0.5rem;">
      <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="var(--color-success-foreground)" stroke-width="3"><polyline points="20 6 9 17 4 12"></polyline></svg>
      SQL editor with autocomplete
    </div>
    <div style="display: flex; align-items: center; gap: 0.5rem;">
      <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="var(--color-success-foreground)" stroke-width="3"><polyline points="20 6 9 17 4 12"></polyline></svg>
      Schema catalog indexing
    </div>
    <div style="display: flex; align-items: center; gap: 0.5rem;">
      <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="var(--color-success-foreground)" stroke-width="3"><polyline points="20 6 9 17 4 12"></polyline></svg>
      Query history and caching
    </div>
    <div style="display: flex; align-items: center; gap: 0.5rem;">
      <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="var(--color-success-foreground)" stroke-width="3"><polyline points="20 6 9 17 4 12"></polyline></svg>
      Built-in forecasting
    </div>
    <div style="display: flex; align-items: center; gap: 0.5rem;">
      <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="var(--color-success-foreground)" stroke-width="3"><polyline points="20 6 9 17 4 12"></polyline></svg>
      PDF dashboard export
    </div>
  </div>
</div>

<!-- CTA -->
<div class="section" style="background: linear-gradient(135deg, #d97706 0%, #b45309 100%); color: white; border-radius: 1rem; text-align: center; padding: 4rem 1.5rem; margin: 4rem 0;">
  <h2 style="font-size: 2.5rem; font-weight: 700; margin-bottom: 1rem; color: white;">See it in action</h2>
  <p style="font-size: 1.25rem; margin-bottom: 2rem; opacity: 0.95;">Start free with AI included. No credit card required.</p>
  <div style="display: flex; justify-content: center; gap: 1rem; margin-top: 2rem;">
    <a href="https://app.kyomi.ai/login" style="display: inline-flex; align-items: center; justify-content: center; background: white; color: #d97706; font-weight: 700; font-size: 1.125rem; padding: 1rem 2.5rem; border-radius: 0.5rem; text-decoration: none; transition: background-color 0.2s;">
      Get Started Free →
    </a>
  </div>
</div>

</div>

<style scoped>
.features-page {
  max-width: 68rem;
  margin: 0 auto;
  padding: 0 1.5rem 4rem;
}

.features-page h2 {
  margin-top: 0;
}

.features-page ul {
  line-height: 1.8;
}
</style>
