---
layout: page
title: Blog
description: Insights on AI-powered analytics and data science
---

<div class="blog-page">

<div style="text-align: center; padding-top: 3rem; margin-bottom: 3rem;">
  <h1 style="font-size: 2.5rem; font-weight: 700; margin-bottom: 0.5rem;">Blog</h1>
  <p style="font-size: 1.25rem; color: var(--color-muted-foreground);">Insights on AI-powered analytics and building better dashboards.</p>
</div>

<div class="blog-posts">

<a href="/blog/introducing-kyomi-connect" class="blog-card featured">
  <div class="blog-card-content">
    <span class="blog-badge">NEW</span>
    <h2>Introducing Kyomi Connect: Your Credentials Never Leave Your Network</h2>
    <p class="blog-date">March 3, 2026</p>
    <p class="blog-excerpt">Most analytics platforms ask you to trust them with your database credentials. Kyomi Connect eliminates that ask entirely — an open-source, on-premise agent where credentials never leave your infrastructure. Audit every line of code yourself.</p>
    <span class="blog-link">Read more →</span>
  </div>
</a>

<a href="/blog/website-analytics-and-knowledge-base" class="blog-card">
  <div class="blog-card-content">
    <h2>Built-in Website Analytics and a Smarter Knowledge Base</h2>
    <p class="blog-date">February 22, 2026</p>
    <p class="blog-excerpt">Track your website traffic with one script tag — privacy-focused, AI-queryable, and included on every plan. Plus improvements to how Kyomi learns your business.</p>
    <span class="blog-link">Read more →</span>
  </div>
</a>

<a href="/blog/how-to-talk-to-your-data-agent" class="blog-card">
  <div class="blog-card-content">
    <h2>How to Talk to Your Data Agent</h2>
    <p class="blog-date">February 3, 2026</p>
    <p class="blog-excerpt">The biggest improvement to your AI data experience isn't a better model—it's asking better questions. One real-world example shows why.</p>
    <span class="blog-link">Read more →</span>
  </div>
</a>

<a href="/blog/forecasting-and-pdf-export" class="blog-card">
  <div class="blog-card-content">
    <h2>Built-in Forecasting, Multi-Source Charts, and PDF Export</h2>
    <p class="blog-date">January 31, 2026</p>
    <p class="blog-excerpt">Forecast trends with confidence intervals, combine data from multiple databases in one chart, and export dashboards as professional PDFs — all natively in Kyomi.</p>
    <span class="blog-link">Read more →</span>
  </div>
</a>

<a href="/blog/explore-first-pattern-for-ai-data-agents" class="blog-card">
  <div class="blog-card-content">
    <h2>The Explore-First Pattern for AI Data Agents</h2>
    <p class="blog-date">January 25, 2026</p>
    <p class="blog-excerpt">The breakthrough wasn't better prompts—it was letting the agent explore the data warehouse before writing queries. Here's the pattern that made the difference.</p>
    <span class="blog-link">Read more →</span>
  </div>
</a>

<a href="/blog/v1-3-release" class="blog-card">
  <div class="blog-card-content">
    <h2>Kyomi v1.3: Your Data Intelligence, Everywhere</h2>
    <p class="blog-date">January 22, 2026</p>
    <p class="blog-excerpt">Kyomi now works with Claude Code and Cursor via MCP. Your data catalog, queries, and learnings follow you wherever you work. Plus dashboard search and discovery improvements.</p>
    <span class="blog-link">Read more →</span>
  </div>
</a>

<a href="/blog/v1-2-release" class="blog-card">
  <div class="blog-card-content">
    <h2>Kyomi v1.2: AI Watches and Slack Integration</h2>
    <p class="blog-date">January 19, 2026</p>
    <p class="blog-excerpt">Monitor your metrics 24/7 with AI-powered watches that alert you when things change. Plus, ask @kyomi questions directly in Slack with automatic chart rendering.</p>
    <span class="blog-link">Read more →</span>
  </div>
</a>

<a href="/blog/multi-datasource-support" class="blog-card">
  <div class="blog-card-content">
    <h2>One AI, 9 Data Platforms: Multi-Datasource Support</h2>
    <p class="blog-date">January 11, 2026</p>
    <p class="blog-excerpt">Kyomi now connects to BigQuery, Snowflake, PostgreSQL, MySQL, ClickHouse, Redshift, Databricks, SQL Server, and Azure Synapse—all from the same AI-powered interface.</p>
    <span class="blog-link">Read more →</span>
  </div>
</a>

<a href="/blog/understanding-bigquery-costs" class="blog-card">
  <div class="blog-card-content">
    <h2>Understanding Your BigQuery Costs</h2>
    <p class="blog-date">December 13, 2025</p>
    <p class="blog-excerpt">Learn how Kyomi's built-in cost controls help you analyze data in BigQuery without surprise bills, including smart table sampling and a cost-optimized AI agent.</p>
    <span class="blog-link">Read more →</span>
  </div>
</a>

<a href="/blog/welcome-to-kyomi" class="blog-card">
  <div class="blog-card-content">
    <h2>Welcome to Kyomi</h2>
    <p class="blog-date">November 16, 2025</p>
    <p class="blog-excerpt">Introducing Kyomi: AI-powered analytics that bridges the gap between simple chat interfaces and production dashboards.</p>
    <span class="blog-link">Read more →</span>
  </div>
</a>

</div>

<!-- Subscribe Section -->
<div style="text-align: center; margin: 4rem 0; padding: 2.5rem; background: var(--color-muted); border-radius: 1rem;">
  <h3 style="margin-top: 0; font-size: 1.5rem;">Stay Updated</h3>
  <p style="color: var(--color-muted-foreground); margin-bottom: 1.5rem;">Get notified about new posts and product updates.</p>

  <form id="blog-subscribe-form" style="display: flex; gap: 0.5rem; max-width: 400px; margin: 0 auto;">
    <input
      type="email"
      id="subscribe-email"
      placeholder="you@company.com"
      required
      style="flex: 1; padding: 0.75rem 1rem; border: 1px solid var(--color-border); border-radius: 0.5rem; background: var(--color-background); color: var(--color-foreground); font-size: 1rem;"
    />
    <button
      type="submit"
      id="subscribe-button"
      class="subscribe-btn"
    >
      Subscribe
    </button>
  </form>
  <div id="subscribe-message" style="margin-top: 1rem; font-size: 0.875rem;"></div>
</div>

</div>

<script>
if (typeof window !== 'undefined') {
  function initForm() {
    const form = document.getElementById('blog-subscribe-form');
    const button = document.getElementById('subscribe-button');

    if (!form || !button) {
      setTimeout(initForm, 100);
      return;
    }

    form.addEventListener('submit', async function(e) {
      e.preventDefault();
      e.stopPropagation();

      const email = document.getElementById('subscribe-email').value;
      const message = document.getElementById('subscribe-message');

      if (!email) return false;

      button.disabled = true;
      button.textContent = 'Subscribing...';
      message.textContent = '';

      try {
        const response = await fetch('https://app.kyomi.ai/api/v1/subscribe', {
          method: 'POST',
          headers: { 'Content-Type': 'application/json' },
          body: JSON.stringify({
            email: email,
            marketing_consent: true,
            source: 'marketing_site'
          })
        });

        if (response.ok) {
          message.innerHTML = '<span style="color: var(--color-success-foreground);">Thanks for subscribing!</span>';
          document.getElementById('subscribe-email').value = '';
        } else {
          message.innerHTML = '<span style="color: var(--color-error-foreground);">Failed to subscribe. Please try again.</span>';
        }
      } catch (error) {
        message.innerHTML = '<span style="color: var(--color-error-foreground);">Failed to subscribe. Please try again.</span>';
      } finally {
        button.disabled = false;
        button.textContent = 'Subscribe';
      }

      return false;
    });
  }

  if (document.readyState === 'loading') {
    document.addEventListener('DOMContentLoaded', initForm);
  } else {
    initForm();
  }
}
</script>

<style scoped>
.blog-page {
  max-width: 48rem;
  margin: 0 auto;
  padding: 0 1.5rem 4rem;
}

.blog-posts {
  display: flex;
  flex-direction: column;
  gap: 1.5rem;
}

.blog-card {
  display: block;
  padding: 2rem;
  border: 1px solid var(--color-border);
  border-radius: 0.75rem;
  text-decoration: none;
  transition: all 0.2s;
  background: var(--color-background);
}

.blog-card:hover {
  border-color: var(--color-primary);
  box-shadow: 0 4px 12px -2px rgb(0 0 0 / 0.1);
}

.blog-card.featured {
  border-left: 4px solid var(--color-primary);
}

.blog-card h2 {
  margin: 0 0 0.5rem;
  font-size: 1.25rem;
  font-weight: 600;
  color: var(--color-foreground);
}

.blog-badge {
  display: inline-block;
  padding: 0.25rem 0.75rem;
  background: var(--color-primary);
  color: white;
  font-size: 0.7rem;
  font-weight: 600;
  border-radius: 9999px;
  margin-bottom: 0.75rem;
}

.blog-date {
  color: var(--color-muted-foreground);
  font-size: 0.875rem;
  margin: 0 0 1rem;
}

.blog-excerpt {
  color: var(--color-muted-foreground);
  margin: 0 0 1rem;
  line-height: 1.6;
}

.blog-link {
  color: var(--color-primary);
  font-weight: 600;
}

.subscribe-btn {
  padding: 0.75rem 1.5rem;
  background: var(--color-primary);
  color: white;
  border: none;
  border-radius: 0.5rem;
  font-weight: 600;
  cursor: pointer;
  white-space: nowrap;
  transition: opacity 0.2s;
}

.subscribe-btn:hover {
  opacity: 0.9;
}
</style>
