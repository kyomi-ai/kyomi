# Product Marketing Context

*Last updated: 2026-04-10*

## Product Overview
**One-liner:** Kyomi is the knowledge layer between you and all your data — it learns your business, unifies your databases, and makes your dashboards the source of truth.
**What it does:** Kyomi sits between your team and their databases. It connects to all of them — Postgres, BigQuery, Snowflake, MySQL, ClickHouse, whatever — and brings them under a single umbrella of accumulated knowledge. Dashboards aren't just charts; they're curated sources of truth that ground every answer Kyomi gives, whether you're asking from Claude.ai, Claude Code, Slack, or the Kyomi app. Unlike individual MCP connectors with dispersed knowledge, Kyomi is the unified intelligence layer across all your data.
**Product category:** Data intelligence layer / knowledge platform for databases
**Product type:** Open-source with hosted, self-hosted, and desktop options
**Business model:** Open source (AGPL). See Pricing Strategy section below for full details. Commercial license available for non-AGPL use cases.

## Target Audience
**Target companies:** Anyone with a database. From a solo founder with a Postgres instance to a mid-market company with BigQuery, Snowflake, and three other databases they need unified. Teams tired of dispersed knowledge across tools, analysts who are the only ones who know what's in the data.
**Decision-makers:** Data team leads, heads of analytics, CTOs, engineering leads, founders/operators who run their own data
**Primary use case:** A unified knowledge layer across all your databases — ask questions from wherever you already work (Claude.ai, Claude Code, Slack) and get answers grounded in curated dashboards and accumulated business context
**Jobs to be done:**
- Unify multiple databases under one knowledge layer instead of managing separate MCP connectors with no shared context
- Build dashboards that serve as source of truth AND documentation — so every answer is grounded in curated, org-approved metrics
- Preserve institutional data knowledge so it doesn't walk out the door when people leave
- Get answers from your data without leaving your existing tools (Claude.ai, Claude Code, Cursor, Slack)
**Use cases:**
- Ask data questions from Claude.ai or Claude Code, grounded in your org's curated dashboards
- Build dashboards that double as documentation and source of truth for the whole org
- Proactive metric monitoring ("alert me if churn rate exceeds 5%")
- Unify 3+ databases under a single knowledge umbrella instead of per-database MCP connectors
- Onboarding new team members — the knowledge layer teaches them how the org thinks about data
- Privacy-focused website analytics (no cookies, built-in)
- Run as a desktop app for personal analytics on local databases

## Personas
| Persona | Cares about | Challenge | Value we promise |
|---------|-------------|-----------|------------------|
| Data Analyst (User/Champion) | Reducing ad-hoc request backlog, accuracy, dashboards as documentation | Drowning in "can you pull this?" requests, knowledge locked in their head | Dashboards become the source of truth everyone references; define metrics once, they're used everywhere |
| Product Manager (User) | Quick access to metrics from tools they already use | Blocked by analyst availability, has to context-switch to a BI tool | Ask from Claude.ai or Slack, get answers grounded in curated org dashboards |
| Engineering Lead (Technical Influencer) | Unified data access, no per-database MCP setup | Multiple databases, each with its own connector and no shared context | One knowledge layer across all databases, accessible from Claude Code |
| Founder/Operator (Decision Maker + User) | Simplicity, cost, runs anywhere | Can't justify enterprise BI, just needs to query their Postgres | Desktop desktop app or self-hosted, open source, affordable hosted option |
| VP/Head of Data (Decision Maker) | Org-wide shared intelligence, not personal chatbots | Knowledge trapped in individual conversations, no org-wide source of truth | Shared knowledge layer where dashboards ARE the documentation |

## Problems & Pain Points
**Core problem:** Data knowledge is scattered and personal. Each database has its own connector, each person has their own understanding of the metrics, dashboards rot because they're disconnected from the knowledge behind them, and AI chatbots forget everything between sessions. There's no shared, org-wide source of truth for "what does our data actually mean?"
**Why alternatives fall short:**
- Traditional BI tools (Looker, Tableau) produce dashboards disconnected from the knowledge behind them — they're charts, not documentation
- Metabase is easier but still per-database, no unified knowledge layer, no AI that learns
- Individual MCP connectors (one per database) have no shared context — ask about data that spans two databases and you're on your own
- AI chatbots (ChatGPT, Claude with raw MCP) are stateless and personal — knowledge stays in one person's conversation, not shared across the org
- Per-seat pricing (Metabase $85/mo + $6-12/user) scales unpredictably
**What it costs them:** Analyst time wasted re-explaining the same metrics. Decisions made on wrong data because there's no source of truth. Knowledge lost during turnover. Every new team member starts from zero.
**Emotional tension:** Frustration that "everyone has a different number for the same metric." Fear of making decisions on wrong data. Anxiety that key knowledge lives in one person's head or one person's chat history.

## Competitive Landscape
**Direct:** Metabase — open-source BI, easy setup, but built for the pre-AI era. No knowledge system, no learning, dashboards are charts not documentation, per-database with no unified layer, per-seat pricing. Hex — notebook-style analytics, powerful but complex, aimed at data teams not the whole org.
**Secondary:** Claude/ChatGPT + individual MCP connectors — you can connect to one database at a time, but there's no shared knowledge, no dashboards as source of truth, no org-wide intelligence. Knowledge stays trapped in individual conversations. Spreadsheets — universal but can't span multiple databases or accumulate shared knowledge.
**Indirect:** Hiring more data analysts — expensive, doesn't scale, knowledge still lives in people's heads. Building internal tools — months of engineering time, maintenance burden. Doing nothing — everyone just asks the one person who knows the data.

## Differentiation
**Key differentiators:**
- Unified data layer — brings ALL your databases (Postgres, BigQuery, Snowflake, MySQL, ClickHouse, etc.) under one knowledge umbrella. Not one connector per database with no shared context.
- Dashboards as source of truth — dashboards are unified with the knowledge system. When someone asks a question, Kyomi references curated dashboards to ground its answer in org-approved metrics, not hallucinated guesses from limited context. Dashboards are documentation and source of truth in the same stroke.
- Shared org intelligence, not personal chatbots — knowledge is designed to be shared across the org. When you ask from Claude.ai, Claude Code, or Slack, you get answers grounded in the same org-wide knowledge. Not siloed in one person's chat history.
- AI-native, not AI-bolted-on — Kyomi is next-gen BI, slimmed down and built to leverage AI from the ground up. Not a legacy dashboard tool with an AI chatbot stapled to the side.
- Proactive AI Monitoring (Watches) — AI agents scan data on schedule, detect anomalies, send contextual alerts in plain English.
- Open source, runs anywhere — hosted cloud, self-hosted, or desktop app. No vendor lock-in.
- Built-in website analytics — privacy-focused (no cookies, ~1KB script), queryable as a datasource.
**How we do it differently:** Kyomi is the knowledge layer between you and all your data. You don't come to Kyomi to chat — you chat wherever you already are (Claude.ai, Claude Code, Slack) and Kyomi is the intelligence layer that grounds every answer in your org's curated dashboards and accumulated knowledge. It unifies multiple databases into a single context that the whole org shares.
**Why that's better:** Individual MCP connectors give you one database at a time with no shared knowledge. Traditional BI gives you charts with no intelligence behind them. AI chatbots give you personal, stateless conversations. Kyomi gives you all your data, all your knowledge, shared across the org, grounded in curated dashboards.
**Why customers choose us:** Unified knowledge across all databases. Dashboards that are actually useful as source of truth. Works from the tools they already use. Open source. Runs anywhere from a desktop app to the cloud.

## Objections
| Objection | Response |
|-----------|----------|
| "We already have a BI tool" | Kyomi is next-gen BI, not another legacy tool. Your current BI gives you charts. Kyomi gives you a knowledge layer — dashboards that are source of truth, answers grounded in org-approved metrics, intelligence shared across the whole team. It's what BI becomes when you build for AI from the ground up. |
| "AI can't be trusted with our data" | Your data stays in your database. Kyomi sends the AI a schema + max 20 rows of results, never bulk data. Self-host it, run the desktop app, or use hosted cloud. Open source — audit every line. |
| "We tried text-to-SQL and it was inaccurate" | Stateless text-to-SQL guesses every time. Kyomi grounds answers in your curated dashboards and accumulated knowledge — org-approved metric definitions, table relationships, business rules. Not hallucinated guesses from a bare schema. |
| "We already use Claude/ChatGPT with MCP" | Individual MCP connectors give you one database at a time with zero shared context. Kyomi unifies all your databases under one knowledge layer, grounds answers in curated dashboards, and shares that intelligence across the whole org — not just one person's chat. |

**Anti-persona:** Companies that need pixel-perfect printed reports (use Tableau). Organizations where data access is intentionally restricted and self-service is unwanted. Teams that only have one small database and one person who queries it (they might be fine with a raw MCP connector).

## Switching Dynamics
**Push:** Frustrated that everyone has a different number for the same metric. Tired of managing separate connectors per database with no shared knowledge. Lost institutional knowledge when key analyst left. Current BI tool feels bloated for what they actually need.
**Pull:** One knowledge layer across all databases. Dashboards that are the source of truth. Ask from Claude.ai or Claude Code without switching tools. Open source, runs anywhere. Slimmed-down next-gen BI built for AI.
**Habit:** "We've always used [Metabase/Looker/Mode]." Team knows the existing tool. Existing dashboards would need migration. "Claude + MCP works fine for my personal queries."
**Anxiety:** "Will the AI get our metrics wrong?" "Is our data safe?" "Will the team actually adopt another tool?" "What if we invest time curating dashboards and then switch?" "Is open source sustainable?"

## Customer Language
**How they describe the problem:**
- "I just need a quick number and I have to wait two days for someone to pull it"
- "Every time I ask the AI, I have to re-explain what MRR means"
- "When [analyst name] left, half our dashboard knowledge walked out the door"
- "I have five databases and five MCP connectors and none of them know about each other"
- "Our dashboards are charts nobody trusts because they're disconnected from the actual definitions"
- "I don't want another tool to log into, I just want answers where I already am"
**How they describe us:**
- "It's the knowledge layer between us and our data"
- "I ask from Claude Code and get answers grounded in our actual dashboards"
- "It's what Metabase would be if you rebuilt it for AI"
- "Our dashboards are finally the source of truth, not just pretty charts"
**Words to use:** knowledge layer, source of truth, unified, grounded, next-gen BI, open source, shared intelligence, curated, org-wide
**Words to avoid:** AI-powered (overused), revolutionary, game-changing, synergy, leverage, robust, comprehensive, delve, cutting-edge, chatbot (Kyomi is not a chatbot)
**Glossary:**
| Term | Meaning |
|------|---------|
| Knowledge Layer | The unified intelligence layer between users and all their databases — accumulated metric definitions, table relationships, business rules, and curated dashboards shared across the org |
| Watch | An AI agent that monitors a metric on a schedule and sends alerts when something noteworthy happens |
| ChartML | Kyomi's open, markdown-based chart specification format |
| Kyomi Connect | Open-source agent deployed on-premise to query private databases without exposing credentials |
| Collection | An organizational unit that groups dashboards and knowledge together |

## Brand Voice
**Tone:** Sophisticated, warm, direct. Not cold/corporate. Not playful/cute. Think Bloomberg Terminal meets a beautifully typeset research paper.
**Style:** Clear and concrete. Lead with what it does, not how it works. Avoid jargon in marketing copy. Use specifics over generalities ("answers in seconds" not "fast insights").
**Personality:** Intelligent, trustworthy, alive, refined, opinionated

## Proof Points
**Metrics:**
- Unify N databases under one knowledge layer (vs. N separate connectors with no shared context)
- Dashboard creation in minutes (vs. weeks), and they serve as source of truth not just charts
- Knowledge compounds with every conversation, shared org-wide
- ~1KB analytics script (vs. hundreds of KB for GA/others)
- Desktop app runs on 2GB RAM, no infrastructure needed
**Customers:** [To be added as community grows]
**Testimonials:**
> [To be added]
**Value themes:**
| Theme | Proof |
|-------|-------|
| Unified knowledge layer | All databases under one umbrella, shared org-wide, not per-person chatbot silos |
| Dashboards as source of truth | Curated dashboards ground every answer — documentation and visualization in one stroke |
| Works where you are | Ask from Claude.ai, Claude Code, Slack — Kyomi is the intelligence layer, not another app to log into |
| Open source, runs anywhere | Hosted, self-hosted, desktop. No vendor lock-in. Audit every line. |
| Next-gen BI | Slimmed down, built for AI from the ground up. Not a legacy tool with AI bolted on. |

## Goals
**Business goal:** Establish Kyomi as the next-gen BI platform — the knowledge layer between people and their data. Grow open-source adoption, convert to hosted/paid tiers.
**Conversion action:** Download desktop app or self-host → connect databases → curate first dashboard as source of truth → share with team → upgrade to hosted for convenience
**Current metrics:** [To be filled in]

## Pricing Strategy

*Last updated: 2026-04-10*

### Principles
- Optimize for adoption and growth, revenue is a bonus
- One tier, no feature gating, no upsells, no annual contracts
- Every deployment gets every feature
- Month-to-month only

### Deployment Modes

| Mode | Price | AI | Analytics | Users |
|------|-------|-----|-----------|-------|
| **Cloud** | $5/user/month | BYOK or token bundles | 100K events/mo free, then $10/M/mo | Unlimited |
| **Self-Hosted** | Free | BYOK | Unlimited (your infrastructure) | Unlimited |
| **Desktop** | Free | BYOK | Unlimited (local) | Single user |

Cloud includes a 30-day free trial, no credit card required.

### AI — Three Options
1. **Bring Your Own Key (BYOK)** — User connects their Anthropic, OpenAI, or Google API key. No markup. If a key is provided, it's used first.
2. **Token Bundles** — Pre-paid AI credits purchased from Kyomi. 30% markup over provider cost (not disclosed publicly). Non-expiring. Fallback when no key is connected.
3. **MCP (Claude Code, Cursor, Codex)** — User's existing AI subscription handles the LLM cost. Kyomi is the MCP server providing the knowledge layer. No additional AI cost from Kyomi.

### Website Analytics
- **$10 per million events per month**
- 6 months data retention
- Cloud includes 100K events/month free
- Desktop and self-hosted: unlimited (user hosts their own storage)
- Competitive positioning: undercuts PostHog ($50/M), Plausible ($69/M), Mixpanel ($280/M)

### Competitive Pricing Position
| Competitor | Their Price | Kyomi |
|------------|-----------|-------|
| Metabase Cloud | $85/mo + $6-12/user | $5/user, no base fee |
| Looker | ~$5,000/mo+ | $5/user |
| Power BI Pro | $10/user/mo | $5/user |
| Hex | $28/user/mo | $5/user |
| PostHog analytics | $50/1M events | $10/1M events |
| Plausible | $69/1M pageviews | $10/1M events |

### What We Don't Disclose Publicly
- Token bundle markup percentage (30%)
- Token bundle tier breakdown (just "purchase bundles")
- The word "cheap" — use "affordable" or just state the price

## Site Architecture Notes

*Existing site:* VitePress at `apps/marketing/`, deployed to kyomi.ai via Vercel. Structure is solid — Homepage, Features, Pricing, Self-Host, Blog, Docs, Alternatives/Metabase. ~51 pages total.

*Problem:* Messaging has evolved over time and is confusing to new visitors. Positioning drifted from the core story. Structure is mostly fine, content needs cohesive rewrite.

*Structural tweaks to make:*
- Add GitHub link/logo to header nav (signals open source immediately)
- Add `/compare/mcp-connectors` page (timely comparison, core differentiator)
- Give integrations (Claude.ai, Claude Code, Slack) more prominent placement — "works where you are" is a core selling point now, not a feature bullet
- Consider `/download` as a standalone entry point for the desktop app

*Content rewrite priority:*
1. Homepage — lead with "knowledge layer between you and all your data"
2. Features — reorganize around unified data layer, dashboards as source of truth, shared org intelligence
3. Pricing — simplify to ~$5/user/mo + AI, free self-hosted/desktop
4. Compare/MCP connectors — new page
5. Blog/Docs — lower priority, update over time
