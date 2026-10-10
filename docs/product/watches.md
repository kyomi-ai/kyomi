---
layout: doc
title: Kyomi Watch - Autonomous Data Monitoring
description: Set up AI-powered alerts and scheduled reports that watch your data 24/7
---

# Kyomi Watch

Kyomi Watch is your autonomous data monitoring system. Instead of manually checking dashboards, let AI watch your metrics and alert you when something matters.

## Two Monitoring Modes

### Alert Mode (Conditional)
The AI analyzes your data on schedule and only notifies you when conditions are met.

**Examples:**
- "Alert me if daily revenue drops more than 10% compared to last week"
- "Notify me when error rate exceeds 5%"
- "Watch for unusual spikes in customer churn"

### Report Mode (Scheduled)
Receive a summary on every scheduled run, regardless of what the data shows.

**Examples:**
- "Send me a daily revenue breakdown by region every morning at 9 AM"
- "Weekly user engagement summary every Monday"
- "End-of-month financial report on the 1st"

## Creating a Watch

1. Click **Create Watch** in the Watches section
2. Describe what you want to monitor in plain English
3. The AI explores your data catalog to understand available metrics
4. Review the preview card showing your watch configuration
5. Confirm to activate—your watch starts running on schedule

![Creating a watch](/images/docs/watch-creation-sidebar.png)

## Schedule Timezones

Schedules use five cron fields: minute, hour, day of month, month and weekday (0 or 7 is Sunday; 1 is Monday). Choose a named IANA timezone such as `Australia/Sydney` to keep a recurring schedule at the same local time through daylight saving changes. Monday at 09:00 Sydney is `0 9 * * 1` with `Australia/Sydney`: Sunday 23:00 UTC in winter and Sunday 22:00 UTC in summer.

The editor and next execution preview show the saved timezone, including when you edit from a browser in another timezone. The preview includes the next actual local and UTC execution dates. Changing the timezone keeps the cron wall time and recalculates the next execution. Missing local times during a daylight saving jump are skipped; repeated local times run once at their first occurrence.

Existing watches and schedules without a timezone retain UTC semantics. Choose `UTC` for a schedule that must fire at a fixed UTC time. A timezone mentioned only in a report's prompt does not change its schedule or set query date boundaries.

## Notification Channels

### Slack
- Alerts posted to any channel you choose
- Per-watch channel configuration
- Rich formatting with key metrics highlighted

### Email
- Multiple recipients supported
- HTML and plain text versions
- Configure per-watch

### In-App
- Alerts appear in your Kyomi inbox
- Unread badges in sidebar
- Click to investigate further in chat

## Investigating Alerts

When an alert fires, you can:
1. View the full analysis and SQL queries used
2. Click "Investigate" to continue the conversation in chat
3. Ask follow-up questions with full context preserved

![Investigating an alert](/images/docs/alert-investigate.png)

## Watch Limits by Plan

| Plan | Watches |
|------|---------|
| Free | — |
| Starter | 5 |
| Team | 50 |
| Pro | 200 |
