# Team & Workspaces

Kyomi is built around **workspaces**. A workspace holds your connected data sources, dashboards, saved knowledge, and chat history — and everything in it is shared with the people you invite. This page explains how to add teammates, how roles and seats work, and how to avoid the most common mistake.

::: warning Adding a coworker? Invite them — don't have them sign up separately.
If a teammate creates their own Kyomi account instead of accepting your invitation, they get their **own empty workspace** with no connection to yours. They won't see your data sources, dashboards, or knowledge, because those belong to *your* workspace.

The fix is always the same: a workspace admin **invites** them by email, and they **accept** that invitation. See [Inviting teammates](#inviting-teammates) below.
:::

## How workspaces work

Everything you create in Kyomi lives inside one workspace:

- **Data sources** — connections to BigQuery, Snowflake, Postgres, ClickHouse, and the rest
- **Dashboards & documents** — the reports you build
- **Knowledge** — the business context Kyomi accumulates about your data
- **Chats** — your conversation history with the agent

Every member of a workspace shares all of it. That's the whole point of inviting people rather than having them sign up on their own: an invited member instantly sees the same data sources and dashboards you do, governed by their role.

## Inviting teammates

Any **admin** (or the owner) can invite teammates:

1. Go to **Settings → Team**.
2. Select **Invite Member**.
3. Enter the teammate's **email address** and pick a **role** (User or Admin).
4. Select **Send Invitation**.

Kyomi emails them an invitation link. Until they accept, they appear under **Pending Invitations**, where you can cancel a pending invite at any time.

Workspaces are **uncapped by default**, so you can invite freely. If the owner has set a **seat cap** to control billing (see [Seats & billing](#seats-billing)), admins can invite up to that cap.

## Roles

| Role | Can do | Notes |
|------|--------|-------|
| **User** | Full access to data sources, dashboards, knowledge, and chat | The default for most teammates |
| **Admin** | Everything a User can, **plus** invite members, change member roles, and remove members | Manages the team |
| **Owner** | Everything an Admin can, **plus** billing, seat cap, and ownership transfer | One per workspace; set when the workspace is created |

All members cost the same regardless of role — see [Seats & billing](#seats-billing).

## Accepting an invitation

When someone is invited, they accept from either the banner at the top of the app or the link in their invitation email:

1. **Sign in with the exact email the invitation was sent to.** This matters — see below.
2. Open the invitation (top-of-app banner, or the link in the email).
3. Choose **Accept Invitation**.

**If they already have a Kyomi account**, accepting simply **adds their existing account to your workspace** — nothing is deleted or merged. They'll then be able to switch between their own workspace and yours.

::: tip The invitation email must match their login email
An invitation is tied to the exact email address it was sent to. If your teammate tries to accept while signed in as a *different* email, they'll see *"This invitation is addressed to a different account."* Invite the precise email they log in with, and they'll be able to accept.
:::

## Seats & billing

Kyomi Cloud is **$5 per user, per month**, and includes a **30-day free trial** with $1 in AI credit.

- **You're billed for active members.** Billing is based on how many active members your workspace has, at $5 each per month.
- **Seat cap (optional).** Workspaces are uncapped by default. The **owner** can set a **seat cap** in **Settings → Billing** to put a ceiling on how many members can be invited — a way to keep billing predictable. Admins can invite up to that cap.
- **During the free trial.** You aren't charged during the 30-day trial, and inviting teammates during the trial doesn't add a charge. Add a payment method before the trial ends to keep your workspace and members active.
- **Who manages billing.** Only the workspace **owner** can open the Billing tab, change the subscription, set the seat cap, or update the payment method. Admins can invite and manage members but can't touch billing.

## Transferring ownership

The owner can hand the workspace to another member from **Settings → Team → Transfer Ownership**. After the transfer completes, the previous owner becomes a regular admin, and the new owner takes over billing and ownership-transfer rights.

## Troubleshooting

**"My coworker signed up but can't see our data sources."**
They created their own separate workspace by signing up independently. Have a workspace admin invite their email from **Settings → Team**, then have them sign in with that same email and accept. Their account joins your workspace and your data sources appear immediately.

**"The invitation says it's addressed to a different account."**
The invited email doesn't match the email your teammate is signed in with. Send the invitation to the exact address they use to log in.

**"An invite failed with 'Workspace user limit reached.'"**
The workspace owner has set a seat cap and it's now full. Ask the owner to raise it in **Settings → Billing**, or remove an inactive member first.

**"Will inviting people during my trial cost me anything?"**
No. Nothing is charged during the 30-day trial, invited teammates included. Billing begins only after the trial, at $5 per active user per month.
