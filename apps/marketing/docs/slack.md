---
layout: doc
title: Slack Integration - Data Insights Where You Work
description: Query Kyomi by @mentioning in Slack channels, see charts rendered inline
---

# Slack Integration

Bring data insights to where your team already talks. @mention Kyomi in any Slack channel to ask questions and see visualizations—without leaving Slack.

## Getting Started

### 1. Install the Kyomi App
Your workspace admin installs Kyomi from the Slack App Directory or from Settings > Integrations in Kyomi.

### 2. Connect Your Account
Run `/kyomi connect` in any Slack channel to link your Kyomi account. This enables:
- Queries run with your datasource credentials
- Messages posted as you (not as a bot)
- Bi-directional sync with Kyomi web

### 3. Start Asking Questions
Mention @kyomi in any channel or DM:

> @kyomi what was our revenue last month?

## Chart Rendering

When Kyomi's response includes a visualization, it renders as an image directly in Slack.

**How it works:**
- ChartML specs are rendered to PNG images
- Images upload to Slack and embed in the thread
- First 2 charts render as images; additional charts link to Kyomi web

## Bi-directional Sync

### Slack to Kyomi Web
- @kyomi conversations appear in your Kyomi conversation list
- Full context preserved: questions, responses, charts, tool calls
- Team members can see shared channel conversations

### Kyomi Web to Slack
- Add messages in Kyomi web to continue the Slack thread
- Messages sync back to the original thread
- Maintains conversation continuity across platforms

## Team Collaboration

### Channel Mentions
Questions asked in channels are visible to all channel members—great for sharing insights with your team.

### Direct Messages
DM @kyomi for private conversations that only you can see.

### Shared Conversations
Slack threads become shared conversations in Kyomi, visible to workspace members based on channel access.

## Commands

| Command | Description |
|---------|-------------|
| `/kyomi connect` | Link your Kyomi account |
| `/kyomi disconnect` | Unlink your account |
| `/kyomi status` | Check connection status |

## Security

- Slack bot tokens encrypted at rest (AES-256-GCM)
- User tokens enable posting as yourself (optional)
- All queries use your datasource credentials
- Disconnect anytime with `/kyomi disconnect`
