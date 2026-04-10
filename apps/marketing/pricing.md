---
layout: page
title: Pricing
description: Open source. Cheap cloud. Free self-hosting.
---

<div class="pricing-page">

<div style="text-align: center; padding-top: 3rem;">
  <h1 style="font-size: 2.5rem; font-weight: 700; margin-bottom: 0.5rem;">Simple Pricing</h1>
  <p style="font-size: 1.25rem; color: var(--color-muted-foreground);">The platform is cheap. AI usage is the variable cost.</p>
</div>

<div class="pricing-grid" style="margin-top: 2.5rem;">
  <!-- Standalone -->
  <div class="pricing-card">
    <h3>Standalone</h3>
    <p class="card-description">Single binary, no infrastructure needed</p>
    <div class="price">
      Free
    </div>
    <p class="billing-info">Bring your own AI key</p>
    <a href="/self-hosting" class="cta-primary cta-sm" style="width: 100%; margin-bottom: 1rem;">Download</a>
    <ul>
      <li>Single binary, 2GB RAM</li>
      <li>SQLite — no database to manage</li>
      <li>All features included</li>
      <li>Unlimited dashboards</li>
      <li>Unlimited knowledge</li>
      <li>MCP support</li>
      <li>Website analytics</li>
      <li>Bring your own LLM API key</li>
    </ul>
    <p class="card-footer">Run on your laptop or desktop</p>
  </div>

  <!-- Hosted Cloud (Featured) -->
  <div class="pricing-card featured">
    <div class="badge">Easiest</div>
    <h3>Hosted Cloud</h3>
    <p class="card-description">We handle everything — just connect your data</p>
    <div class="price">
      ~$5
      <span class="period">/user/month</span>
    </div>
    <p class="billing-info">+ AI usage costs</p>
    <a href="https://app.kyomi.ai/login" class="cta-primary cta-sm" style="width: 100%; margin-bottom: 1rem;">Get Started Free</a>
    <ul>
      <li><strong>No infrastructure to manage</strong></li>
      <li>All features included</li>
      <li>Unlimited dashboards</li>
      <li>Unlimited knowledge</li>
      <li>MCP support</li>
      <li>Slack integration</li>
      <li>Website analytics</li>
      <li>Kyomi Watch monitoring</li>
      <li>PDF export</li>
      <li>Priority support</li>
    </ul>
    <p class="card-footer" style="color: var(--color-success-foreground); font-weight: 600;">Free trial — no credit card required</p>
  </div>

  <!-- Self-Hosted -->
  <div class="pricing-card">
    <h3>Self-Hosted</h3>
    <p class="card-description">Your infrastructure, your control</p>
    <div class="price">
      Free
    </div>
    <p class="billing-info">Bring your own AI key</p>
    <a href="/self-hosting" class="cta-primary cta-sm" style="width: 100%; margin-bottom: 1rem;">Self-Host Guide</a>
    <ul>
      <li>Docker or standalone binary</li>
      <li>All features included</li>
      <li>Unlimited dashboards</li>
      <li>Unlimited knowledge</li>
      <li>Unlimited users</li>
      <li>MCP support</li>
      <li>Website analytics</li>
      <li>Bring your own LLM API key</li>
      <li>Community support</li>
    </ul>
    <p class="card-footer">Open source (AGPL)</p>
  </div>
</div>

<div style="text-align: center; margin: 2rem auto; max-width: 40rem;">
  <p style="color: var(--color-muted-foreground); font-size: 0.95rem;">
    All deployment options include the full feature set. The only difference is who manages the infrastructure and how AI costs are handled.
  </p>
</div>

<h2 style="text-align: center; margin-top: 3rem;">Frequently Asked Questions</h2>

<div style="margin: 2rem auto; max-width: 48rem;">
  <div style="margin-bottom: 1.5rem;">
    <p style="font-weight: 600; margin-bottom: 0.5rem;">What are "AI usage costs"?</p>
    <p style="color: var(--color-muted-foreground);">Every question you ask Kyomi uses an LLM to generate SQL and insights. On hosted cloud, AI costs are included in your usage and billed based on how much you use. Self-hosted and standalone users bring their own API key (Anthropic, OpenAI, or Google) and pay their LLM provider directly.</p>
  </div>

  <div style="margin-bottom: 1.5rem;">
    <p style="font-weight: 600; margin-bottom: 0.5rem;">Is Kyomi really open source?</p>
    <p style="color: var(--color-muted-foreground);">Yes. Kyomi is licensed under AGPL-3.0. The full source code is on <a href="https://github.com/kyomi-ai/kyomi">GitHub</a>. You can audit every line, self-host for free, and contribute. A commercial license is available for organizations that need non-AGPL terms.</p>
  </div>

  <div style="margin-bottom: 1.5rem;">
    <p style="font-weight: 600; margin-bottom: 0.5rem;">What's the difference between standalone and self-hosted?</p>
    <p style="color: var(--color-muted-foreground);">Standalone is a single binary that uses SQLite — no database to set up, just download and run. Self-hosted uses Docker with PostgreSQL for multi-user teams. Both are free and include all features.</p>
  </div>

  <div style="margin-bottom: 1.5rem;">
    <p style="font-weight: 600; margin-bottom: 0.5rem;">Does my data leave my database?</p>
    <p style="color: var(--color-muted-foreground);">No. Kyomi queries your database directly and the AI sees max 20 rows of results per query. Your data stays where it is. For maximum control, deploy <a href="/docs/connect/">Kyomi Connect</a> on your network — credentials never leave your infrastructure.</p>
  </div>

  <div style="margin-bottom: 1.5rem;">
    <p style="font-weight: 600; margin-bottom: 0.5rem;">Which LLM providers are supported?</p>
    <p style="color: var(--color-muted-foreground);">Anthropic Claude, OpenAI, and Google Gemini. Bring your own API key for self-hosted and standalone deployments.</p>
  </div>

  <h3 style="margin-top: 2rem; margin-bottom: 1rem;">Data Platform Costs</h3>
  <p style="margin-bottom: 1rem;"><strong>Kyomi charges for the platform and AI. Your database costs are separate:</strong></p>

  <table style="width: 100%; border-collapse: collapse; background: var(--color-background); border-radius: 0.5rem; overflow: hidden; margin-bottom: 1rem;">
    <thead>
      <tr style="background: var(--color-muted);">
        <th style="padding: 0.75rem 1rem; text-align: left; border-bottom: 1px solid var(--color-border);">Database</th>
        <th style="padding: 0.75rem 1rem; text-align: left; border-bottom: 1px solid var(--color-border);">What You Pay Your Provider</th>
      </tr>
    </thead>
    <tbody>
      <tr>
        <td style="padding: 0.75rem 1rem; border-bottom: 1px solid var(--color-border);">PostgreSQL / MySQL / ClickHouse / SQL Server</td>
        <td style="padding: 0.75rem 1rem; border-bottom: 1px solid var(--color-border);">Your infrastructure costs (or free for local)</td>
      </tr>
      <tr style="background: var(--color-muted);">
        <td style="padding: 0.75rem 1rem; border-bottom: 1px solid var(--color-border);">BigQuery</td>
        <td style="padding: 0.75rem 1rem; border-bottom: 1px solid var(--color-border);">Bytes processed per query</td>
      </tr>
      <tr>
        <td style="padding: 0.75rem 1rem; border-bottom: 1px solid var(--color-border);">Snowflake</td>
        <td style="padding: 0.75rem 1rem; border-bottom: 1px solid var(--color-border);">Compute credits used</td>
      </tr>
      <tr style="background: var(--color-muted);">
        <td style="padding: 0.75rem 1rem; border-bottom: 1px solid var(--color-border);">Redshift / Databricks / Azure Synapse</td>
        <td style="padding: 0.75rem 1rem; border-bottom: 1px solid var(--color-border);">Cluster/compute costs</td>
      </tr>
    </tbody>
  </table>
</div>

<div class="section" style="background: linear-gradient(135deg, #d97706 0%, #b45309 100%); color: white; border-radius: 1rem; text-align: center; padding: 4rem 1.5rem; margin: 4rem auto;">
  <h2 style="font-size: 2.25rem; font-weight: 700; margin-bottom: 0.75rem; color: white;">The knowledge layer between you and all your data.</h2>
  <p style="font-size: 1.25rem; margin-bottom: 2rem; opacity: 0.95;">Open source. No credit card required.</p>
  <div style="display: flex; justify-content: center; gap: 1rem; flex-wrap: wrap; margin-top: 2rem;">
    <a href="https://app.kyomi.ai/login" style="display: inline-flex; align-items: center; justify-content: center; background: white; color: #d97706; font-weight: 700; font-size: 1.125rem; padding: 1rem 2.5rem; border-radius: 0.5rem; text-decoration: none; transition: background-color 0.2s;">
      Try Hosted Free →
    </a>
    <a href="https://github.com/kyomi-ai/kyomi" style="display: inline-flex; align-items: center; justify-content: center; background: rgba(255,255,255,0.15); color: white; font-weight: 600; font-size: 1.125rem; padding: 1rem 2.5rem; border-radius: 0.5rem; text-decoration: none; border: 1px solid rgba(255,255,255,0.3); transition: background-color 0.2s;">
      View on GitHub
    </a>
  </div>
</div>

</div>

<style scoped>
.pricing-page {
  max-width: 68rem;
  margin: 0 auto;
  padding: 0 1.5rem 4rem;
}

table {
  font-size: 0.875rem;
}

@media (max-width: 768px) {
  table {
    font-size: 0.75rem;
  }

  th, td {
    padding: 0.5rem !important;
  }
}
</style>
