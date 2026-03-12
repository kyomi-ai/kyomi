---
layout: page
title: Pricing
description: Simple, transparent pricing for teams of all sizes
---

<script setup>
import { ref } from 'vue'

const billingCycle = ref('annual')

const toggleBilling = () => {
  billingCycle.value = billingCycle.value === 'annual' ? 'monthly' : 'annual'
}
</script>

<div class="pricing-page">

<div style="text-align: center; padding-top: 3rem;">
  <h1 style="font-size: 2.5rem; font-weight: 700; margin-bottom: 0.5rem;">Simple, Transparent Pricing</h1>
  <p style="font-size: 1.25rem; color: var(--color-muted-foreground);">Start free. Upgrade when you're ready. Cancel anytime.</p>
</div>

<div style="text-align: center; margin: 1.5rem 0;">
  <div style="display: inline-flex; gap: 0.5rem; align-items: center; padding: 0.35rem; background: var(--color-muted); border-radius: 0.5rem;">
    <button
      @click="toggleBilling"
      :class="billingCycle === 'monthly' ? 'billing-active' : 'billing-inactive'"
      style="padding: 0.4rem 0.75rem; border-radius: 0.375rem; border: none; font-weight: 600; font-size: 0.875rem; cursor: pointer; transition: all 0.2s;"
    >
      Monthly
    </button>
    <button
      @click="toggleBilling"
      :class="billingCycle === 'annual' ? 'billing-active' : 'billing-inactive'"
      style="padding: 0.4rem 0.75rem; border-radius: 0.375rem; border: none; font-weight: 600; font-size: 0.875rem; cursor: pointer; transition: all 0.2s; position: relative;"
    >
      Annual
      <span class="status-badge success" style="position: absolute; top: -14px; right: -14px; font-size: 0.65rem; padding: 0.1rem 0.3rem;">Save 25%</span>
    </button>
  </div>
</div>

<div class="pricing-grid">
  <!-- Free Tier -->
  <div class="pricing-card">
    <h3>Free</h3>
    <p class="card-description">Full SQL editor and dashboards, free forever</p>
    <div class="price">
      $0
      <span class="period">forever</span>
    </div>
    <p class="billing-info">No credit card required</p>
    <a href="https://app.kyomi.ai/login" class="cta-primary cta-sm" style="width: 100%; margin-bottom: 1rem;">Start Free</a>
    <ul>
      <li>Limited AI budget/month</li>
      <li>Up to 5 dashboards</li>
      <li>Built-in forecasting</li>
      <li>Website analytics (50K events/mo)</li>
      <li>7-day query history</li>
      <li>Full SQL editor</li>
      <li>MCP support</li>
      <li>Community support</li>
    </ul>
    <p class="card-footer">Free forever with limited AI</p>
  </div>

  <!-- Starter Tier -->
  <div class="pricing-card">
    <h3>Starter</h3>
    <p class="card-description">Perfect for individuals getting started</p>
    <div class="price" v-if="billingCycle === 'annual'">
      $15
      <span class="period">/month</span>
    </div>
    <div class="price" v-else>
      $20
      <span class="period">/month</span>
    </div>
    <p class="billing-info" v-if="billingCycle === 'annual'">Billed annually at $180/year</p>
    <p class="billing-info" v-else>Billed monthly</p>
    <a href="https://app.kyomi.ai/login" class="cta-primary cta-sm" style="width: 100%; margin-bottom: 1rem;">Get Started</a>
    <ul>
      <li><strong>AI chat and analysis</strong></li>
      <li>Unlimited dashboards</li>
      <li>Built-in forecasting</li>
      <li>Website analytics (1M events/mo)</li>
      <li>30-day query history</li>
      <li>Full SQL editor</li>
      <li>MCP Support</li>
      <li>Email support</li>
    </ul>
    <p class="card-footer">Perfect for individual users</p>
  </div>

  <!-- Pro Tier (Featured) -->
  <div class="pricing-card featured">
    <div class="badge">Most Popular</div>
    <h3>Pro</h3>
    <p class="card-description">For power users and daily analytics</p>
    <div class="price" v-if="billingCycle === 'annual'">
      $29
      <span class="period">/month</span>
    </div>
    <div class="price" v-else>
      $39
      <span class="period">/month</span>
    </div>
    <p class="billing-info" v-if="billingCycle === 'annual'">Billed annually at $348/year</p>
    <p class="billing-info" v-else>Billed monthly</p>
    <a href="https://app.kyomi.ai/login" class="cta-primary cta-sm" style="width: 100%; margin-bottom: 1rem;">Get Started</a>
    <ul>
      <li><strong>3x AI usage vs Starter</strong></li>
      <li>Unlimited dashboards</li>
      <li>Built-in forecasting</li>
      <li>Website analytics (5M events/mo)</li>
      <li>Unlimited query history</li>
      <li>PDF dashboard export</li>
      <li>Kyomi Watch monitoring</li>
      <li>MCP Support</li>
    </ul>
    <p class="card-footer" style="color: var(--color-success-foreground); font-weight: 600;">Best value for power users</p>
  </div>

  <!-- Team Tier -->
  <div class="pricing-card">
    <h3>Team</h3>
    <p class="card-description">For teams and collaboration</p>
    <div class="price" v-if="billingCycle === 'annual'">
      $99
      <span class="period">/month</span>
    </div>
    <div class="price" v-else>
      $129
      <span class="period">/month</span>
    </div>
    <p class="billing-info" v-if="billingCycle === 'annual'">Billed annually at $1,188/year</p>
    <p class="billing-info" v-else>Billed monthly</p>
    <a href="https://app.kyomi.ai/login" class="cta-primary cta-sm" style="width: 100%; margin-bottom: 1rem;">Get Started</a>
    <ul>
      <li><strong>Shared AI pool</strong></li>
      <li>Unlimited dashboards</li>
      <li>Built-in forecasting</li>
      <li>Website analytics (25M events/mo)</li>
      <li>Unlimited query history</li>
      <li>PDF dashboard export</li>
      <li>Kyomi Watch monitoring</li>
      <li>MCP Support</li>
      <li>Slack Integration</li>
      <li>Up to 5 users (+$15-20/user)</li>
    </ul>
    <p class="card-footer">Save 48% vs individual Pro plans</p>
  </div>
</div>

## Feature Comparison

<div style="overflow-x: auto; margin-top: 2rem;">
  <table style="width: 100%; border-collapse: collapse; background: white; border-radius: 0.5rem; overflow: hidden;">
    <thead>
      <tr style="background: var(--color-primary); color: white;">
        <th style="padding: 1rem; text-align: left; border-bottom: 1px solid var(--color-border);">Feature</th>
        <th style="padding: 1rem; text-align: center; border-bottom: 1px solid var(--color-border);">Free</th>
        <th style="padding: 1rem; text-align: center; border-bottom: 1px solid var(--color-border);">Starter</th>
        <th style="padding: 1rem; text-align: center; border-bottom: 1px solid var(--color-border);">Pro</th>
        <th style="padding: 1rem; text-align: center; border-bottom: 1px solid var(--color-border);">Team</th>
      </tr>
    </thead>
    <tbody>
      <tr>
        <td style="padding: 1rem; border-bottom: 1px solid var(--color-border);"><strong>AI Budget</strong></td>
        <td style="padding: 1rem; text-align: center; border-bottom: 1px solid var(--color-border);">Limited</td>
        <td style="padding: 1rem; text-align: center; border-bottom: 1px solid var(--color-border);">AI enabled</td>
        <td style="padding: 1rem; text-align: center; border-bottom: 1px solid var(--color-border);">AI enabled (3x Starter)</td>
        <td style="padding: 1rem; text-align: center; border-bottom: 1px solid var(--color-border);">Shared AI pool</td>
      </tr>
      <tr style="background: var(--color-muted);">
        <td style="padding: 1rem; border-bottom: 1px solid var(--color-border);"><strong>Dashboards</strong></td>
        <td style="padding: 1rem; text-align: center; border-bottom: 1px solid var(--color-border);">5 max</td>
        <td style="padding: 1rem; text-align: center; border-bottom: 1px solid var(--color-border);">✓ Unlimited</td>
        <td style="padding: 1rem; text-align: center; border-bottom: 1px solid var(--color-border);">✓ Unlimited</td>
        <td style="padding: 1rem; text-align: center; border-bottom: 1px solid var(--color-border);">✓ Unlimited</td>
      </tr>
      <tr>
        <td style="padding: 1rem; border-bottom: 1px solid var(--color-border);"><strong>Query History</strong></td>
        <td style="padding: 1rem; text-align: center; border-bottom: 1px solid var(--color-border);">7 days</td>
        <td style="padding: 1rem; text-align: center; border-bottom: 1px solid var(--color-border);">30 days</td>
        <td style="padding: 1rem; text-align: center; border-bottom: 1px solid var(--color-border);">✓ Unlimited</td>
        <td style="padding: 1rem; text-align: center; border-bottom: 1px solid var(--color-border);">✓ Unlimited</td>
      </tr>
      <tr style="background: var(--color-muted);">
        <td style="padding: 1rem; border-bottom: 1px solid var(--color-border);"><strong>Support</strong></td>
        <td style="padding: 1rem; text-align: center; border-bottom: 1px solid var(--color-border);">Community</td>
        <td style="padding: 1rem; text-align: center; border-bottom: 1px solid var(--color-border);">Email</td>
        <td style="padding: 1rem; text-align: center; border-bottom: 1px solid var(--color-border);">Priority Email</td>
        <td style="padding: 1rem; text-align: center; border-bottom: 1px solid var(--color-border);">Priority Email</td>
      </tr>
      <tr>
        <td style="padding: 1rem; border-bottom: 1px solid var(--color-border);"><strong>Forecasting</strong></td>
        <td style="padding: 1rem; text-align: center; border-bottom: 1px solid var(--color-border);">✓</td>
        <td style="padding: 1rem; text-align: center; border-bottom: 1px solid var(--color-border);">✓</td>
        <td style="padding: 1rem; text-align: center; border-bottom: 1px solid var(--color-border);">✓</td>
        <td style="padding: 1rem; text-align: center; border-bottom: 1px solid var(--color-border);">✓</td>
      </tr>
      <tr style="background: var(--color-muted);">
        <td style="padding: 1rem; border-bottom: 1px solid var(--color-border);"><strong>Kyomi Watch</strong></td>
        <td style="padding: 1rem; text-align: center; border-bottom: 1px solid var(--color-border);">—</td>
        <td style="padding: 1rem; text-align: center; border-bottom: 1px solid var(--color-border);">—</td>
        <td style="padding: 1rem; text-align: center; border-bottom: 1px solid var(--color-border);">✓</td>
        <td style="padding: 1rem; text-align: center; border-bottom: 1px solid var(--color-border);">✓</td>
      </tr>
      <tr>
        <td style="padding: 1rem; border-bottom: 1px solid var(--color-border);"><strong>MCP Support</strong></td>
        <td style="padding: 1rem; text-align: center; border-bottom: 1px solid var(--color-border);">✓</td>
        <td style="padding: 1rem; text-align: center; border-bottom: 1px solid var(--color-border);">✓</td>
        <td style="padding: 1rem; text-align: center; border-bottom: 1px solid var(--color-border);">✓</td>
        <td style="padding: 1rem; text-align: center; border-bottom: 1px solid var(--color-border);">✓</td>
      </tr>
      <tr style="background: var(--color-muted);">
        <td style="padding: 1rem; border-bottom: 1px solid var(--color-border);"><strong>PDF Export</strong></td>
        <td style="padding: 1rem; text-align: center; border-bottom: 1px solid var(--color-border);">—</td>
        <td style="padding: 1rem; text-align: center; border-bottom: 1px solid var(--color-border);">—</td>
        <td style="padding: 1rem; text-align: center; border-bottom: 1px solid var(--color-border);">✓</td>
        <td style="padding: 1rem; text-align: center; border-bottom: 1px solid var(--color-border);">✓</td>
      </tr>
      <tr>
        <td style="padding: 1rem; border-bottom: 1px solid var(--color-border);"><strong>Users</strong></td>
        <td style="padding: 1rem; text-align: center; border-bottom: 1px solid var(--color-border);">1</td>
        <td style="padding: 1rem; text-align: center; border-bottom: 1px solid var(--color-border);">1</td>
        <td style="padding: 1rem; text-align: center; border-bottom: 1px solid var(--color-border);">1</td>
        <td style="padding: 1rem; text-align: center; border-bottom: 1px solid var(--color-border);">5 (+$15-20/user)</td>
      </tr>
      <tr style="background: var(--color-muted);">
        <td style="padding: 1rem; border-bottom: 1px solid var(--color-border);"><strong>Slack Integration</strong></td>
        <td style="padding: 1rem; text-align: center; border-bottom: 1px solid var(--color-border);">—</td>
        <td style="padding: 1rem; text-align: center; border-bottom: 1px solid var(--color-border);">—</td>
        <td style="padding: 1rem; text-align: center; border-bottom: 1px solid var(--color-border);">—</td>
        <td style="padding: 1rem; text-align: center; border-bottom: 1px solid var(--color-border);">✓</td>
      </tr>
      <tr>
        <td style="padding: 1rem; border-bottom: 1px solid var(--color-border);"><strong>Website Analytics</strong></td>
        <td style="padding: 1rem; text-align: center; border-bottom: 1px solid var(--color-border);">50K events/mo</td>
        <td style="padding: 1rem; text-align: center; border-bottom: 1px solid var(--color-border);">1M events/mo</td>
        <td style="padding: 1rem; text-align: center; border-bottom: 1px solid var(--color-border);">5M events/mo</td>
        <td style="padding: 1rem; text-align: center; border-bottom: 1px solid var(--color-border);">25M events/mo</td>
      </tr>
      <tr style="background: var(--color-muted);">
        <td style="padding: 1rem; border-bottom: 1px solid var(--color-border);"><strong>Analytics Retention</strong></td>
        <td style="padding: 1rem; text-align: center; border-bottom: 1px solid var(--color-border);">30 days</td>
        <td style="padding: 1rem; text-align: center; border-bottom: 1px solid var(--color-border);">180 days</td>
        <td style="padding: 1rem; text-align: center; border-bottom: 1px solid var(--color-border);">1 year</td>
        <td style="padding: 1rem; text-align: center; border-bottom: 1px solid var(--color-border);">2 years</td>
      </tr>
    </tbody>
  </table>
</div>

<h2 style="text-align: center; margin-top: 3rem;">Frequently Asked Questions</h2>

<div style="margin: 2rem auto; max-width: 48rem;">
  <div style="margin-bottom: 1.5rem;">
    <p style="font-weight: 600; margin-bottom: 0.5rem;">What counts as AI usage?</p>
    <p style="color: var(--color-muted-foreground);">Each message sent to the AI chat interface or SQL copilot counts toward your monthly AI budget. We track your usage as a percentage.</p>
  </div>

  <div style="margin-bottom: 1.5rem;">
    <p style="font-weight: 600; margin-bottom: 0.5rem;">Can I upgrade or downgrade anytime?</p>
    <p style="color: var(--color-muted-foreground);">Yes! Change your plan at any time. Upgrades take effect immediately. Downgrades take effect at your next billing cycle.</p>
  </div>

  <div style="margin-bottom: 1.5rem;">
    <p style="font-weight: 600; margin-bottom: 0.5rem;">What happens when I hit my AI budget limit?</p>
    <p style="color: var(--color-muted-foreground);">You'll receive notifications at 80%, 90%, and 100% usage. When you reach 100%, AI features pause until next month or you upgrade. Dashboards and manual SQL queries continue working.</p>
  </div>

  <div style="margin-bottom: 1.5rem;">
    <p style="font-weight: 600; margin-bottom: 0.5rem;">Is there a free plan?</p>
    <p style="color: var(--color-muted-foreground);">The Free tier is free forever with a limited AI budget that resets every month. You get the full SQL editor, up to 5 dashboards, and MCP support permanently. Upgrade anytime for more AI capacity.</p>
  </div>

  <h3 style="margin-top: 2rem; margin-bottom: 1rem;">Data Platform Billing</h3>
  <p style="margin-bottom: 1rem;"><strong>You pay your cloud provider directly for compute/storage:</strong></p>

  <table style="width: 100%; border-collapse: collapse; background: white; border-radius: 0.5rem; overflow: hidden; margin-bottom: 1rem;">
    <thead>
      <tr style="background: var(--color-muted);">
        <th style="padding: 0.75rem 1rem; text-align: left; border-bottom: 1px solid var(--color-border);">Platform</th>
        <th style="padding: 0.75rem 1rem; text-align: left; border-bottom: 1px solid var(--color-border);">What You Pay For</th>
      </tr>
    </thead>
    <tbody>
      <tr>
        <td style="padding: 0.75rem 1rem; border-bottom: 1px solid var(--color-border);">BigQuery</td>
        <td style="padding: 0.75rem 1rem; border-bottom: 1px solid var(--color-border);">Bytes processed per query</td>
      </tr>
      <tr style="background: var(--color-muted);">
        <td style="padding: 0.75rem 1rem; border-bottom: 1px solid var(--color-border);">Snowflake</td>
        <td style="padding: 0.75rem 1rem; border-bottom: 1px solid var(--color-border);">Compute credits used</td>
      </tr>
      <tr>
        <td style="padding: 0.75rem 1rem; border-bottom: 1px solid var(--color-border);">Redshift</td>
        <td style="padding: 0.75rem 1rem; border-bottom: 1px solid var(--color-border);">Cluster uptime + storage</td>
      </tr>
      <tr style="background: var(--color-muted);">
        <td style="padding: 0.75rem 1rem; border-bottom: 1px solid var(--color-border);">Databricks</td>
        <td style="padding: 0.75rem 1rem; border-bottom: 1px solid var(--color-border);">DBUs consumed</td>
      </tr>
      <tr>
        <td style="padding: 0.75rem 1rem; border-bottom: 1px solid var(--color-border);">Azure Synapse</td>
        <td style="padding: 0.75rem 1rem; border-bottom: 1px solid var(--color-border);">DWUs + storage</td>
      </tr>
      <tr style="background: var(--color-muted);">
        <td style="padding: 0.75rem 1rem; border-bottom: 1px solid var(--color-border);">PostgreSQL/MySQL/ClickHouse/SQL Server</td>
        <td style="padding: 0.75rem 1rem; border-bottom: 1px solid var(--color-border);">Your infrastructure costs</td>
      </tr>
    </tbody>
  </table>

  <p><strong>Kyomi charges only for AI features.</strong> Your data platform costs are separate and billed directly by your provider.</p>
</div>

<!-- Self-Hosting Callout -->
<div style="margin: 3rem auto; max-width: 48rem; padding: 2rem; border: 2px solid var(--color-primary); border-radius: 0.75rem; text-align: center;">
  <h3 style="margin: 0 0 0.5rem; font-size: 1.25rem;">Prefer to self-host?</h3>
  <p style="color: var(--color-muted-foreground); margin-bottom: 1rem;">Run Kyomi on your own infrastructure for free. Bring your own LLM API key, keep everything on your network. Available as a standalone binary or Docker image.</p>
  <a href="/self-hosting" style="display: inline-flex; align-items: center; justify-content: center; background: var(--color-primary); color: white; font-weight: 600; padding: 0.6rem 1.5rem; border-radius: 0.375rem; text-decoration: none;">Learn about self-hosting →</a>
</div>

<div class="section" style="background: linear-gradient(135deg, #d97706 0%, #b45309 100%); color: white; border-radius: 1rem; text-align: center; padding: 4rem 1.5rem; margin: 4rem auto;">
  <h2 style="font-size: 2.5rem; font-weight: 700; margin-bottom: 1rem; color: white;">Ready to get started?</h2>
  <p style="font-size: 1.25rem; margin-bottom: 2rem; opacity: 0.95;">Start free with AI included. No credit card required.</p>
  <div style="display: flex; justify-content: center; gap: 1rem; margin-top: 2rem;">
    <a href="https://app.kyomi.ai/login" style="display: inline-flex; align-items: center; justify-content: center; background: white; color: #d97706; font-weight: 700; font-size: 1.125rem; padding: 1rem 2.5rem; border-radius: 0.5rem; text-decoration: none; transition: background-color 0.2s;">
      Get Started Free →
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
.billing-active {
  background: white !important;
  color: var(--color-foreground) !important;
  box-shadow: 0 1px 3px 0 rgb(0 0 0 / 0.1);
}

.billing-inactive {
  background: transparent !important;
  color: var(--color-muted-foreground) !important;
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
