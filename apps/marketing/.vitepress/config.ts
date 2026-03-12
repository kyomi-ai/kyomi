import { defineConfig } from 'vitepress'

export default defineConfig({
  title: "Kyomi",
  description: "AI-powered data analytics that speaks your language",
  lang: 'en-US',

  // Disable dark mode toggle
  appearance: false,

  // Clean URLs (no .html extension)
  cleanUrls: true,

  // Connect docs pages are created in a separate task — suppress dead link errors
  ignoreDeadLinks: [
    /\/docs\/connect\//
  ],

  // Base URL for production
  base: '/',

  // Head tags for SEO
  head: [
    ['link', { rel: 'icon', type: 'image/x-icon', href: '/favicon.ico' }],
    ['link', { rel: 'icon', type: 'image/svg+xml', href: '/kyomi_small_logo.svg' }],
    ['meta', { name: 'theme-color', content: '#d97706' }], // Primary brand color
    ['meta', { name: 'og:type', content: 'website' }],
    ['meta', { name: 'og:locale', content: 'en' }],
    ['meta', { name: 'og:site_name', content: 'Kyomi' }],
    ['meta', { name: 'og:image', content: '/images/og-image.png' }],
    ['meta', { name: 'twitter:card', content: 'summary_large_image' }],
    ['meta', { name: 'twitter:image', content: '/images/og-image.png' }],
    ['meta', { name: 'keywords', content: 'AI analytics, BigQuery, Snowflake, PostgreSQL, MySQL, ClickHouse, Redshift, Databricks, SQL Server, Azure Synapse, data visualization, natural language SQL, AI dashboards, multi-datasource analytics, website analytics, privacy-focused analytics' }],
    // Kyomi Analytics — signed key mode
    ['script', { defer: '', 'data-key': 'eyJzIjoiYzAzNGRkZTU3YTRiM2I0NiIsInciOiJ3b3Jrc3BhY2UtOTlmMjRkMDUtNjczZDI1YjgiLCJkIjpbImt5b21pLmFpIl19.PrxwbaLdZ-4amhpCBU01EU5Reb_J0-zg8NZERzWm2X4', src: 'https://analytics.kyomi.ai/k.js' }],
    // Schema.org structured data for AI and search engine discoverability
    ['script', { type: 'application/ld+json' }, JSON.stringify({
      "@context": "https://schema.org",
      "@type": "SoftwareApplication",
      "name": "Kyomi",
      "description": "AI-powered data analytics platform that learns your business, answers questions in plain English, monitors your data 24/7, and integrates with Slack and developer tools. Connect BigQuery, Snowflake, PostgreSQL, and 6 more datasources.",
      "url": "https://kyomi.ai",
      "applicationCategory": "BusinessApplication",
      "applicationSubCategory": "AI Analytics Platform, Business Intelligence Tool, Natural Language SQL",
      "operatingSystem": "Web",
      "offers": {
        "@type": "AggregateOffer",
        "lowPrice": "0",
        "highPrice": "129",
        "priceCurrency": "USD",
        "offerCount": "3"
      },
      "featureList": [
        "Natural language data queries (text-to-SQL)",
        "AI-powered dashboard creation",
        "Proactive data monitoring with AI agents",
        "Slack integration with chart rendering",
        "MCP integration for Claude Code and Cursor",
        "Built-in forecasting with confidence intervals",
        "9 datasource connectors (BigQuery, Snowflake, PostgreSQL, MySQL, ClickHouse, Redshift, Databricks, SQL Server, Azure Synapse)",
        "Accumulated business knowledge that compounds over time",
        "Built-in privacy-focused website analytics",
        "PDF dashboard export",
        "Multi-source charts combining data from different databases"
      ],
      "screenshot": "https://kyomi.ai/images/og-image.png"
    })],
  ],

  themeConfig: {
    // Logo
    logo: '/kyomi_full_logo.svg',
    siteTitle: false, // Hide text title, logo shows full branding

    // Navigation
    nav: [
      { text: 'Features', link: '/features' },
      { text: 'Pricing', link: '/pricing' },
      { text: 'Blog', link: '/blog' },
      { text: 'Docs', link: '/docs/' }, // Kyomi documentation
      { text: 'Alternatives', link: '/alternatives/metabase' },
      {
        text: 'Sign In',
        link: 'https://app.kyomi.ai/login',
        target: '_self'
      }
    ],

    // Sidebar - only shows on /docs/ pages
    sidebar: {
      '/docs/': [
        {
          text: 'Getting Started',
          items: [
            { text: 'Overview', link: '/docs/' },
            { text: 'Quick Start', link: '/docs/#quick-start' },
          ]
        },
        {
          text: 'Datasource Setup',
          collapsed: false,
          items: [
            { text: 'Overview', link: '/docs/datasources/' },
            { text: 'Google BigQuery', link: '/docs/datasources/bigquery' },
            { text: 'Snowflake', link: '/docs/datasources/snowflake' },
            { text: 'PostgreSQL', link: '/docs/datasources/postgres' },
            { text: 'MySQL', link: '/docs/datasources/mysql' },
            { text: 'ClickHouse', link: '/docs/datasources/clickhouse' },
            { text: 'SQL Server', link: '/docs/datasources/sqlserver' },
            { text: 'Amazon Redshift', link: '/docs/datasources/redshift' },
            { text: 'Databricks', link: '/docs/datasources/databricks' },
            { text: 'Azure Synapse', link: '/docs/datasources/synapse' },
          ]
        },
        {
          text: 'ChartML',
          collapsed: false,
          items: [
            { text: 'Overview', link: '/docs/chartml/' },
            { text: 'Data Sources', link: '/docs/chartml/source' },
            { text: 'Parameters', link: '/docs/chartml/params' },
            { text: 'Chart & Visualize', link: '/docs/chartml/chart' },
            { text: 'Transform Pipeline', link: '/docs/chartml/transform' },
            { text: 'Style & Formatting', link: '/docs/chartml/style' },
            { text: 'Config', link: '/docs/chartml/config' },
            { text: 'Grid Layout', link: '/docs/chartml/grid' },
          ]
        },
        {
          text: 'Kyomi Connect',
          collapsed: false,
          items: [
            { text: 'Overview', link: '/docs/connect/' },
            { text: 'Installation', link: '/docs/connect/installation' },
            { text: 'Configuration', link: '/docs/connect/configuration' },
            { text: 'Security Model', link: '/docs/connect/security' },
            { text: 'Troubleshooting', link: '/docs/connect/troubleshooting' },
          ]
        },
        {
          text: 'Features',
          items: [
            { text: 'Kyomi Watch', link: '/docs/watches' },
            { text: 'Website Analytics', link: '/docs/analytics' },
            { text: 'Slack Integration', link: '/docs/slack' },
            { text: 'MCP Integration', link: '/docs/mcp' },
            { text: 'Creating Charts', link: '/docs/#creating-charts' },
            { text: 'ChartML Basics', link: '/docs/#chartml-basics' },
            { text: 'Chart Types', link: '/docs/#chart-types' },
            { text: 'Dashboard Parameters', link: '/docs/#dashboard-parameters' },
            { text: 'SQL Editor', link: '/docs/#sql-editor' },
          ]
        },
        {
          text: 'Advanced',
          items: [
            { text: 'DuckDB Processing', link: '/docs/#client-side-data-processing-duckdb' },
            { text: 'Forecasting', link: '/docs/#forecasting' },
            { text: 'Multi-Source Charts', link: '/docs/#multi-source-charts' },
            { text: 'Line Styles & Bands', link: '/docs/#line-styles-confidence-bands' },
            { text: 'PDF Export', link: '/docs/#pdf-export' },
            { text: 'AI Agent Features', link: '/docs/#ai-agent-features' },
            { text: 'Styling & Customization', link: '/docs/#styling-customization' },
          ]
        },
        {
          text: 'Reference',
          items: [
            { text: 'ChartML Spec', link: 'https://chartml.org' },
          ]
        }
      ]
    },

    // Footer
    footer: {
      message: 'Built with privacy in mind. <a href="/privacy">Privacy</a> · <a href="/terms">Terms</a> · <a href="/security">Security</a> · <a href="/cookies">Cookies</a>',
      copyright: '© 2026 Alytic Pty Ltd. All rights reserved.'
    },

    // Search
    search: {
      provider: 'local'
    }
  },

  // Sitemap for SEO
  sitemap: {
    hostname: 'https://kyomi.ai',
    transformItems: (items) => {
      return items.filter(item =>
        !item.url.includes('SCREENSHOTS_TODO')
      )
    }
  },

  // Markdown configuration - allow raw HTML
  markdown: {
    html: true
  },

  // Vite server configuration for network access
  vite: {
    server: {
      host: '0.0.0.0',  // Listen on all network interfaces
      port: 5175
    }
  }
})
