import { defineConfig } from 'vitepress'

export default defineConfig({
  title: "Kyomi",
  description: "The knowledge layer between you and all your data",
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
    ['link', { rel: 'preconnect', href: 'https://fonts.googleapis.com' }],
    ['link', { rel: 'preconnect', href: 'https://fonts.gstatic.com', crossorigin: '' }],
    ['link', { href: 'https://fonts.googleapis.com/css2?family=DM+Sans:ital,opsz,wght@0,9..40,300;0,9..40,400;0,9..40,500;0,9..40,600;0,9..40,700;1,9..40,400&family=Instrument+Serif:ital@0;1&family=Geist+Mono:wght@400;500&display=swap', rel: 'stylesheet' }],
    ['link', { rel: 'icon', type: 'image/x-icon', href: '/favicon.ico' }],
    ['link', { rel: 'icon', type: 'image/svg+xml', href: '/kyomi_small_logo.svg' }],
    ['meta', { name: 'theme-color', content: '#d97706' }], // Primary brand color
    ['meta', { name: 'og:type', content: 'website' }],
    ['meta', { name: 'og:locale', content: 'en' }],
    ['meta', { name: 'og:site_name', content: 'Kyomi' }],
    ['meta', { name: 'og:image', content: '/images/og-image.png' }],
    ['meta', { name: 'twitter:card', content: 'summary_large_image' }],
    ['meta', { name: 'twitter:image', content: '/images/og-image.png' }],
    ['meta', { name: 'keywords', content: 'open source BI, data intelligence, knowledge layer, AI analytics, BigQuery, Snowflake, PostgreSQL, MySQL, ClickHouse, natural language SQL, AI dashboards, multi-datasource, MCP, source of truth, self-hosted analytics' }],
    // Kyomi Analytics — signed key mode
    ['script', { defer: '', 'data-key': 'eyJzIjoiYzAzNGRkZTU3YTRiM2I0NiIsInciOiJ3b3Jrc3BhY2UtOTlmMjRkMDUtNjczZDI1YjgiLCJkIjpbImt5b21pLmFpIl19.PrxwbaLdZ-4amhpCBU01EU5Reb_J0-zg8NZERzWm2X4', src: 'https://analytics.kyomi.ai/k.js' }],
    // Schema.org structured data for AI and search engine discoverability
    ['script', { type: 'application/ld+json' }, JSON.stringify({
      "@context": "https://schema.org",
      "@type": "SoftwareApplication",
      "name": "Kyomi",
      "description": "Open-source data intelligence platform. The knowledge layer between you and all your databases. Unify Postgres, BigQuery, Snowflake, and more under one layer of shared intelligence. Dashboards as source of truth. Ask from Claude, Slack, or the Kyomi app.",
      "url": "https://kyomi.ai",
      "applicationCategory": "BusinessApplication",
      "applicationSubCategory": "Data Intelligence Platform, Open Source Business Intelligence, Knowledge Layer",
      "operatingSystem": "Web",
      "offers": {
        "@type": "AggregateOffer",
        "lowPrice": "0",
        "highPrice": "5",
        "priceCurrency": "USD",
        "offerCount": "3"
      },
      "featureList": [
        "Unified knowledge layer across all databases",
        "Dashboards as source of truth and documentation",
        "Shared org-wide intelligence, not personal chatbots",
        "Open source (AGPL) — self-host or use hosted cloud",
        "Standalone desktop app — single binary, no infrastructure",
        "MCP integration for Claude Code, Claude.ai, and Cursor",
        "Slack integration with chart rendering",
        "9 database connectors (BigQuery, Snowflake, PostgreSQL, MySQL, ClickHouse, Redshift, Databricks, SQL Server, Azure Synapse)",
        "Proactive data monitoring with AI agents",
        "Built-in forecasting with confidence intervals",
        "Built-in privacy-focused website analytics",
        "Natural language to SQL with accumulated knowledge grounding"
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
      { text: 'Self-Host', link: '/self-hosting' },
      { text: 'Blog', link: '/blog' },
      { text: 'Docs', link: '/docs/' },
      {
        text: 'Compare',
        items: [
          { text: 'vs Metabase', link: '/alternatives/metabase' },
          { text: 'vs MCP Connectors', link: '/alternatives/mcp-connectors' },
        ]
      },
      {
        text: 'Sign In',
        link: 'https://app.kyomi.ai/login',
        target: '_self'
      }
    ],

    // GitHub link in nav bar
    socialLinks: [
      { icon: 'github', link: 'https://github.com/kyomi-ai/kyomi' }
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
      message: 'Open source (AGPL). <a href="/privacy">Privacy</a> · <a href="/terms">Terms</a> · <a href="/security">Security</a> · <a href="/cookies">Cookies</a>',
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
