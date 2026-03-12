---
layout: doc
title: Terms of Service
description: Terms and conditions for using Kyomi
---

# Terms of Service

**Last Updated:** March 3, 2026

**Effective Date:** November 16, 2025

## Welcome to Kyomi

These Terms of Service ("Terms") govern your use of Kyomi, an AI-powered data analytics platform. By using Kyomi, you agree to these Terms. If you don't agree, please don't use our service.

**The short version:** Be reasonable, respect our service and other users, and we'll do the same for you. These Terms protect both of us.

---

## 1. Account Terms

### 1.1 Account Creation

- You must be at least 13 years old to use Kyomi
- You must provide a valid email address
- You're responsible for keeping your account secure
- You're responsible for all activity under your account

### 1.2 Account Security

- **Choose a strong password**, use Google OAuth, or set up a passkey
- **Enable two-factor authentication** (recommended for password-based accounts)
- **Don't share your credentials** with anyone
- **Notify us immediately** if you suspect unauthorized access: [security@kyomi.ai](mailto:security@kyomi.ai)

### 1.3 Workspace Ownership

- The person who creates a workspace is the **workspace owner**
- Workspace owners can invite team members, manage billing, and delete the workspace
- If you're invited to a workspace, the workspace owner controls your access
- Workspaces can be transferred to another user by contacting support

---

## 2. Acceptable Use

### 2.1 What You Can Do

- ✅ Use Kyomi for data analysis and visualization
- ✅ Connect your data warehouses and databases
- ✅ Install and run Kyomi Connect on your infrastructure to bridge your databases to Kyomi
- ✅ Create dashboards and share them with your team
- ✅ Use AI features for querying and analysis
- ✅ Export your dashboards and queries

### 2.2 What You Can't Do

- ❌ **Violate laws** - Don't use Kyomi for illegal purposes
- ❌ **Harm our service** - No hacking, DDoS attacks, or abuse
- ❌ **Spam or scam** - No phishing, fraud, or misleading content
- ❌ **Violate others' privacy** - Respect data protection laws
- ❌ **Resell our service** - Don't white-label or resell Kyomi without permission
- ❌ **Reverse engineer** - Don't decompile, disassemble, or hack our proprietary code (this does not apply to Kyomi Connect, which is open-source under the Apache 2.0 License)
- ❌ **Excessive usage** - Don't abuse rate limits or consume excessive resources

### 2.3 Enforcement

If you violate these Terms, we may:
- Warn you and ask you to stop
- Temporarily suspend your account
- Terminate your account (with notice when possible)
- Report illegal activity to authorities

We'll always try to work with you first, but we reserve the right to protect our service and other users.

---

## 3. Your Data & Privacy

### 3.1 You Own Your Data

- **Your data is yours** - We never claim ownership of your data warehouse or database data
- **Your dashboards are yours** - Export them anytime
- **Your queries are yours** - Download your query history
- **Your chat history is yours** - Delete it anytime

### 3.2 How We Handle Your Data

**Data warehouse connections (BigQuery, PostgreSQL, etc.):**
- **Your data stays in your warehouse** — It streams through our servers but isn't persisted
- **We cache metadata only** — Table/column names for search
- **We process your queries** — To execute them and show results

**Kyomi Connect (on-premise agent):**
- **Your data stays on your infrastructure** — Query results pass through the Connect agent on your network to Kyomi's backend, but are never stored by the agent
- **Your database credentials stay with you** — Unlike direct connections, we never receive or store your database credentials when using Connect
- **The agent collects nothing beyond its core function** — No telemetry, no analytics, no diagnostics
- **You control the agent** — Install, update, and remove it on your own schedule

**Kyomi Analytics (built-in website tracking):**
- **We store analytics event data** — If you create an analytics site, we collect and store anonymized event data from your website visitors on our infrastructure
- **We are the data processor** — You are the data controller
- **No personal data** — No cookies, no raw IPs, no identifiable information

- **We store chat messages** - To provide chat history functionality (you can view, search, and delete your conversations)
- **We respect your privacy** - See our [Privacy Policy](/privacy) for details

### 3.3 Website Analytics Data

If you use Kyomi's built-in website analytics feature:

- **You are the data controller** — You decide to deploy the tracking script on your website
- **We are the data processor** — We collect and store anonymized event data on your behalf
- **Your visitors' data** — We collect page views, device info, and geographic location from your website visitors (no cookies, no personal data, no raw IPs)
- **You can delete it** — Remove your analytics site at any time to permanently delete all collected data
- **Retention limits apply** — Event data is automatically deleted based on your subscription tier's retention period
- **Your responsibility** — You must comply with privacy laws applicable to your website and visitors

### 3.4 Data Processing Agreement

By using Kyomi, you acknowledge that:
- We process your data as described in our [Privacy Policy](/privacy)
- We use trusted third-party services (Google, Stripe) as described
- We maintain appropriate security measures

---

## 4. Billing & Subscriptions

### 4.1 Free Tier

- Free forever for SQL editor and up to 5 dashboards
- Limited AI features (resets monthly)
- Website analytics (event quotas per tier)
- MCP support for Claude Code, Cursor, and other MCP clients
- No credit card required

### 4.2 Paid Plans

- **Billing cycles** - Monthly or annual
- **Payment processing** - Handled by Stripe (we don't store credit cards)
- **AI usage tracking** - Based on LLM tokens consumed
- **Overages** - No surprise charges; AI features pause at 100% usage

### 4.3 Subscription Changes

- **Upgrades** - Take effect immediately, prorated billing
- **Downgrades** - Take effect at next billing cycle
- **Cancellation** - Cancel anytime, no penalties
- **Refunds** - Prorated refunds for annual plans (first 30 days)

### 4.4 Data Platform Billing

**Important:** You pay your cloud provider directly for data warehouse usage:
- **BigQuery**: Query processing and storage (billed by Google)
- **Snowflake**: Compute and storage credits (billed by Snowflake)
- **Redshift/Databricks**: Cluster/compute costs (billed by AWS/Databricks)
- **Self-hosted databases**: Your infrastructure costs
- **Kyomi Connect**: Your infrastructure costs for hosting the agent (compute, network). The Connect agent software itself is included with your Kyomi subscription at no additional cost

Kyomi only charges for AI features and premium capabilities. See our [Pricing](/pricing) page for details.

---

## 5. Service Availability

### 5.1 Uptime & Reliability

We strive for 99.9% uptime, but we can't guarantee it. Things happen:

- **Maintenance** - We'll notify you of planned downtime
- **Outages** - We'll work quickly to resolve issues
- **Third-party issues** - Google, Stripe, etc. are outside our control

### 5.2 Service Changes

We may update Kyomi with new features, improvements, or changes:

- **New features** - We'll announce them and provide documentation
- **Breaking changes** - We'll give advance notice when possible
- **Deprecations** - At least 90 days notice for feature removals

### 5.3 Beta Features

Some features may be labeled "Beta":

- Beta features are provided "as-is" without guarantees
- We may change or remove beta features without notice
- Your feedback helps us improve beta features

---

## 6. Intellectual Property

### 6.1 Kyomi's IP

We own Kyomi's code, design, and branding:

- **Our code** - Kyomi's application and infrastructure
- **Our trademarks** - "Kyomi" name and logo
- **Our content** - Documentation, blog posts, marketing materials

### 6.2 Your IP

You retain all rights to:

- Your BigQuery data
- Your dashboard content
- Your queries and analyses
- Your workspace branding

### 6.3 License to Use Kyomi

We grant you a limited, non-exclusive, non-transferable license to:

- Use Kyomi for your internal business purposes
- Create and share dashboards
- Use AI features within your subscription limits
- Install and run Kyomi Connect on your infrastructure solely to connect your databases to Kyomi

**Kyomi Connect software license:**
- Kyomi Connect is open-source software licensed under the [Apache License 2.0](https://github.com/kyomi-ai/kyomi-connect/blob/main/LICENSE)
- You may use, modify, and redistribute Kyomi Connect in accordance with the Apache 2.0 License
- The source code is publicly available at [github.com/kyomi-ai/kyomi-connect](https://github.com/kyomi-ai/kyomi-connect)
- Contributions to Kyomi Connect are subject to the project's [Contributing Guidelines](https://github.com/kyomi-ai/kyomi-connect/blob/main/CONTRIBUTING.md) and Developer Certificate of Origin (DCO)
- The Kyomi Connect agent requires a valid Kyomi account and authentication token to connect to Kyomi's backend services

### 6.4 Feedback

If you provide feedback, suggestions, or ideas:

- We can use them to improve Kyomi
- You don't get compensation (but you have our gratitude!)
- We're not obligated to implement your suggestions

---

## 7. Third-Party Services

### 7.1 Integrations

Kyomi integrates with:

- **Data Warehouses**: Google BigQuery, Snowflake, Redshift, Azure Synapse, Databricks
- **Databases**: PostgreSQL, MySQL, SQL Server, ClickHouse
- **On-Premise Agent**: Kyomi Connect (runs on your infrastructure to bridge databases to Kyomi)
- **Authentication**: Google OAuth (for login), OAuth-enabled datasources
- **Messaging**: Slack (Slack's terms apply)
- **Payment**: Stripe (Stripe's terms apply)

### 7.2 Kyomi Connect

If you install Kyomi Connect on your infrastructure:

**Our responsibilities:**
- We develop and maintain the open-source Connect agent software ([github.com/kyomi-ai/kyomi-connect](https://github.com/kyomi-ai/kyomi-connect))
- We publish pre-built binaries, Docker images, and Helm charts for easy installation
- We maintain the backend infrastructure that communicates with the agent
- We ensure the agent is secure and free from malicious code — the source code is publicly auditable
- We provide documentation for installation, configuration, and troubleshooting

**Your responsibilities:**
- **Hosting** — You provide and maintain the infrastructure (server, container, or Kubernetes cluster) that runs the agent
- **Security** — You secure the machine running the agent, including the local configuration files that contain your database credentials
- **Updates** — The agent does not auto-update. You are responsible for updating to newer versions when available. We recommend updating promptly when we announce security patches
- **Network** — You ensure the agent can reach your database and make outbound connections to Kyomi's backend
- **Decommissioning** — When you no longer need the agent, revoke its authentication token from Kyomi's settings and remove the agent and its configuration files from your infrastructure

**Liability:**
- We are not liable for issues caused by the infrastructure you provide to host the Connect agent, including hardware failures, network misconfigurations, or insufficient resources
- We are not liable for unauthorized access to your database credentials stored locally on your infrastructure
- Standard liability limitations in Section 8 apply to the Connect agent software

### 7.3 Your Responsibility

When you connect third-party services:

- You're responsible for complying with their terms
- You grant us permission to access them on your behalf
- You maintain adequate permissions and licenses

---

## 8. Limitation of Liability

### 8.1 No Warranties

Kyomi is provided "as-is" without warranties of any kind:

- We don't guarantee error-free operation
- We don't guarantee specific results
- We don't guarantee 100% uptime

### 8.2 Liability Limits

To the maximum extent permitted by law:

- **Our liability is limited to** the amount you paid us in the past 12 months
- **We're not liable for** indirect, incidental, or consequential damages
- **This includes** lost profits, data loss, or business interruption

### 8.3 Exceptions

These limitations don't apply to:

- Our gross negligence or willful misconduct
- Death or personal injury caused by our negligence
- Fraud or fraudulent misrepresentation
- Anything that can't be limited by law

---

## 9. Indemnification

You agree to indemnify and hold us harmless from claims arising from:

- Your violation of these Terms
- Your violation of laws or regulations
- Your violation of third-party rights
- Your use of Kyomi in a negligent or improper manner

**Translation:** If you do something wrong and someone sues us because of it, you'll cover our legal costs.

---

## 10. Termination

### 10.1 You Can Leave Anytime

- **Cancel your subscription** - No penalties, no questions asked
- **Delete your account** - Permanent deletion within 30 days
- **Export your data first** - We can't recover deleted data

### 10.2 We Can Terminate For Cause

We may suspend or terminate your account if:

- You violate these Terms
- You engage in illegal activity
- You abuse our service or harm other users
- Your account has been inactive for 2+ years (with notice)

### 10.3 What Happens After Termination

- **Your access ends** - You can't log in or use Kyomi
- **Your data is deleted** - Permanent deletion within 30 days
- **No refunds** - Except as described in Section 4.3
- **Surviving provisions** - Sections 6, 8, 9, and 11 survive termination

---

## 11. General Legal Stuff

### 11.1 Governing Law

These Terms are governed by the laws of New South Wales, Australia, without regard to conflict of law principles.

### 11.2 Dispute Resolution

**We prefer friendly resolution:** If you have a problem, email us at [support@kyomi.ai](mailto:support@kyomi.ai) and we'll work it out.

**If that doesn't work:**
- Informal negotiation for 30 days
- Mediation (if we both agree)
- Arbitration or court (depending on your location)

### 11.3 Entire Agreement

These Terms, along with our [Privacy Policy](/privacy), constitute the entire agreement between you and Kyomi.

### 11.4 Changes to These Terms

We may update these Terms:

- **Notice** - We'll email you at least 30 days before changes
- **Effective date** - Changes take effect on the date specified
- **Continued use** - Using Kyomi after changes means you accept them

If you don't agree with changes, you can cancel your account.

### 11.5 Severability

If any part of these Terms is found invalid or unenforceable, the rest remains in effect.

### 11.6 No Waiver

Our failure to enforce a provision doesn't waive our right to enforce it later.

### 11.7 Assignment

- **You can't assign** - Don't transfer your account without our permission
- **We can assign** - We may transfer our rights (e.g., if we're acquired)

---

## Contact Us

Questions about these Terms?

- **Email:** [legal@kyomi.ai](mailto:legal@kyomi.ai)
- **General support:** [support@kyomi.ai](mailto:support@kyomi.ai)

We'll respond within 2 business days.

---

**Our Philosophy:** We wrote these Terms to be fair, clear, and protective of both parties. We're not trying to trick you—we want you to succeed with Kyomi while protecting our service and other users.

*Last updated: March 2, 2026*
