---
layout: page
title: "How to Talk to Your Data Agent"
description: "The biggest improvement to your AI data experience isn't a better model—it's asking better questions."
---

<div class="blog-post">

<div class="blog-post-header">
  <h1>How to Talk to Your Data Agent</h1>
  <p class="blog-post-meta">February 3, 2026 · Jason Adams</p>
</div>

<div class="blog-post-content">

We've all been trained by search engines. Type some keywords, hit enter, hope for the best. That habit follows us everywhere—including into AI data tools.

Here's a real prompt someone sent to a data assistant recently:

> several requests for data on October 25 and now
>
> &bull; MAU who completed an action<br>
> &bull; Total weekly activities<br>
> &bull; Login MAU<br>
> &bull; Transaction MAU<br>
> &bull; Daily transactions (average of month)<br>
> &bull; Monthly active transactors

This isn't unusual. It's how most people interact with data tools. And it's exactly why most people don't get great results.

## What's Actually Wrong Here

That prompt has six different questions crammed into one message with almost no context. Let's break down what's missing:

**No clear time range.** "October 25 and now" — October 25 of which year? It turned out they meant October 2025, not a specific date. And "now" when? The day you typed this? The most recent complete month? The agent has no way to know.

**No metric definitions.** What counts as "completed an action"? What's a "Login MAU" versus a "Transaction MAU"? These might seem obvious to you, but they're ambiguous to an agent that has access to dozens of tables with overlapping definitions.

**No output format.** Do you want a summary table? A trend chart? Just raw numbers? Each one requires a different query approach.

**No business context.** Why do you need this? A board report needs precision and formatting. A quick sanity check needs speed. The purpose shapes the answer.

**Six questions at once.** This is the big one.

## One Question at a Time

The single most effective thing you can do is ask one question per message.

When you throw six metrics at an agent in one shot, several things go wrong:

- The agent has to juggle multiple queries simultaneously, increasing the chance of errors in each one
- If one metric is wrong, it's buried in a wall of results and harder to spot
- You can't course-correct along the way—by the time you see all six answers, you've already committed to one interpretation of each metric
- The agent can't ask clarifying questions without creating confusion about which metric it's asking about

Compare that to a focused conversation:

> **You:** What was our MAU for users who completed at least one activity, from October 25 through today?
>
> **Agent:** For Oct 25 – Feb 2, MAU who completed at least one activity was 42,300. I'm using the `user_events` table filtered to `event_type = 'activity_complete'`, counting distinct users per month. Does that match the definition you're looking for?
>
> **You:** That's right. Now show me total weekly activities for the same period.
>
> **Agent:** Here's the weekly breakdown...

Each answer builds context for the next question. The agent remembers what time range you're looking at, which tables it used, what definitions it applied. By the third or fourth question, it's operating with a shared understanding of exactly what you need.

This isn't slower. It's faster. Because you're not spending three follow-up messages untangling which of the six results was wrong and why.

## Add Context, Not Keywords

The difference between a search query and a good question is context.

A search query:

> "Login MAU"

A good question:

> "How many unique users logged in at least once per month, from October through January? I'm comparing this against transaction MAU for a board report, so monthly granularity is fine."

That extra sentence—"I'm comparing against transaction MAU for a board report"—changes how the agent responds. It knows to:

- Use consistent date boundaries across both metrics
- Format the output in a way that's board-presentation-ready
- Flag if the definitions might not be directly comparable

You don't need to write a paragraph. A single sentence of context transforms the result.

## The Three Things That Matter

When you ask a data question, include three things:

**1. What you want to measure** — not just the metric name, but enough detail to remove ambiguity. "Monthly active users" could mean five different things depending on how you define "active."

**2. The time range** — explicit dates or relative ranges. "Last quarter" is fine. "Recently" is not.

**3. Why you need it** — one sentence about the purpose. This helps the agent choose the right level of detail, formatting, and precision.

That's it. You don't need to specify tables, write SQL fragments, or explain your data model. That's the agent's job. Your job is to be clear about what you actually want to know.

## The Conversation, Not the Query

The mental model shift is simple: you're not typing a query into a search bar. You're having a conversation with an analyst.

If a new analyst joined your team and you walked over to their desk and said "several requests for data on October 25 and now — MAU, weekly activities, login MAU, transaction MAU, daily transactions, monthly transactors" — they'd stare at you. They'd need to ask a dozen questions before they could start.

Your data agent is the same. It *can* handle ambiguity—it'll make assumptions and give you something. But those assumptions might be wrong, and now you're debugging instead of analyzing.

Give it the same context you'd give a person. Ask one thing at a time. Let the conversation build.

You'll get better answers, faster.

## The Terminology Problem Doesn't Go Away

Even with perfect prompting habits, there's a deeper issue: every organization has its own language. "MAU" means one thing to your product team and something slightly different to finance. "Revenue" might exclude refunds, or include them, depending on who you ask. These definitions live in people's heads, and they're never written down as clearly as anyone thinks.

This is where a knowledge layer becomes essential. In Kyomi, when you correct the agent—"no, Login MAU only counts users who authenticated, not session refreshes"—it saves that as a permanent learning. The next time anyone on your team asks about Login MAU, the agent already knows the definition. It doesn't need to be told again.

But this doesn't happen on day one. The knowledge layer is built through use. Every correction, every clarification, every "actually, use this table instead" becomes institutional knowledge that compounds over time. After a few weeks, the agent knows which tables matter, what your metrics actually mean, and the quirks of your data that only your most senior analysts used to carry around in their heads.

The prompting techniques in this post will get you better results immediately. But the real unlock is an agent that learns your business well enough that you eventually *don't* need to spell everything out—because it already knows.

---

*[Kyomi](https://kyomi.ai) is a data intelligence platform that connects to your warehouse and answers questions in plain English. It learns your business over time, so the context you provide today becomes knowledge it carries forward tomorrow.*

</div>
</div>
