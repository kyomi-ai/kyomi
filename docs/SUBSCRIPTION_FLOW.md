# Subscription Flow

## Overview

Kyomi uses a single Cloud plan at $5/user/month. Billing is per-workspace, not per-user. Stripe is the single source of truth for subscription state.

## Signup

1. User signs up (Google OAuth, passkey, or password)
2. Backend creates workspace with `tier = "cloud"`
3. Backend creates Stripe customer (using the user's email)
4. Backend creates Stripe subscription with `trial_period_days: 30`, `quantity: 1`
   - No payment method required during trial
   - Stripe sets `subscription.status = "trialing"`
5. Backend stores `stripe_customer_id` and `stripe_subscription_id` on workspace
6. Backend sets `subscription_status = "trialing"` from Stripe response
7. User gets $5 one-time AI credit (`ai_bundle_balance_usd = 5.0`)
8. User enters the app with full access

## During Trial (Days 0–30)

- Full access to all features
- AI usage deducted from the $5 credit (with 10% markup for payment processing fees)
- Owner can invite users — each invite accept calls `update_billing_users` which updates the Stripe subscription quantity
- **Day 7**: Nudge banner appears — "Add a payment method to continue after your trial"
- Owner can add payment method via Stripe Customer Portal at any time

## Trial End (Day 30)

Stripe handles this automatically:

- **If payment method on file**: Stripe charges `$5 × active_members`, subscription moves to `active`. Webhook `customer.subscription.updated` fires, we update `subscription_status = "active"` in DB.
- **If no payment method**: Stripe fires `invoice.payment_failed`. Subscription goes to `past_due`. We update DB. User is blocked and redirected to billing page.

## Active Subscription

- Owner is charged monthly: `$5 × active_member_count`
- Inviting a user → accept → `update_billing_users` increments Stripe quantity → prorated charge
- Removing a user → `update_billing_users` decrements quantity → prorated credit
- Owner can set a `user_limit` (budget cap) — invites are blocked when `active_members + pending_invites >= user_limit`
- The `user_limit` is the owner's spending control, NOT the seat count

## Subscription Gate

When a user signs in:
- Middleware loads `subscription_status` from workspace
- If `active` or `trialing` → full access
- If `past_due` or `cancelled` → redirected to `/settings/billing`
  - **Admins** see: "Subscribe to continue" with payment button
  - **Non-admins** see: "Your workspace subscription has expired. Contact your workspace admin."

## Stripe Entities

- **Customer** = workspace (one per workspace, created at signup with owner's email)
- **Subscription** = Cloud plan (one per workspace, created at signup with 30-day trial)
- **Subscription quantity** = active member count (synced automatically)
- **Invited users** have NO Stripe relationship — they are workspace_users rows

## Webhook Events Handled

| Event | Action |
|-------|--------|
| `customer.subscription.created` | Update workspace tier, status, period dates |
| `customer.subscription.updated` | Update workspace tier, status, period dates |
| `customer.subscription.deleted` | Revert workspace to free, clear Stripe IDs |
| `invoice.payment_succeeded` | Reset AI credits for new billing period |
| `invoice.payment_failed` | Set workspace to `past_due` |
| `checkout.session.completed` (payment mode) | Credit AI/analytics bundle balance |

## Bundle Purchases

Separate from the subscription:
- **AI Token Bundle** ($10) — one-time purchase, credits `ai_bundle_balance_usd`
- **Analytics Event Bundle** ($10) — one-time purchase, credits `analytics_bundle_events`
- Purchased via Stripe Checkout (payment mode, not subscription)
- Fulfilled by `checkout.session.completed` webhook handler
- Non-expiring — balance persists across billing periods

## Billing Page States

| State | What the user sees |
|-------|-------------------|
| Trialing (days 0–7) | "Cloud Plan — Trial (X days remaining)" |
| Trialing (days 7–30) | Same + nudge banner to add payment method |
| Active | "Cloud Plan — Active", billing details, manage payment |
| Past Due | "Payment failed — update your payment method" |
| Cancelled | "Subscription cancelled — subscribe to continue" |
| Non-admin on expired workspace | "Contact your workspace admin" |

## Environment Variables

```
STRIPE_CLOUD_MONTHLY=price_xxx    # Cloud plan price ID
STRIPE_AI_BUNDLE=price_yyy        # AI token bundle price ID
STRIPE_ANALYTICS_BUNDLE=price_zzz # Analytics event bundle price ID
AI_BUDGET_CLOUD=0.0               # Monthly included AI budget (0 = bundle/BYOK only)
AI_BUNDLE_CREDIT_USD=10.0         # USD credited per AI bundle purchase
ANALYTICS_BUNDLE_CREDIT_EVENTS=1000000  # Events credited per analytics bundle purchase
```
