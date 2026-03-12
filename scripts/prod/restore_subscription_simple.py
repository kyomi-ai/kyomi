#!/usr/bin/env python3
"""
Restore subscription data from Stripe for a given customer ID.

Run from repo root with production environment:
    source .env.production && python scripts/prod/restore_subscription_simple.py cus_xxx
"""

import sys
import os

# Set up environment
if not os.getenv("DATABASE_URL"):
    print("❌ DATABASE_URL not set. Run with: source .env.production && python ...")
    sys.exit(1)

if not os.getenv("STRIPE_SECRET_KEY"):
    print("❌ STRIPE_SECRET_KEY not set. Check .env.production")
    sys.exit(1)

import stripe
from sqlalchemy import create_engine, text
from sqlalchemy.orm import sessionmaker
from datetime import datetime, timezone

# Initialize Stripe
stripe.api_key = os.getenv("STRIPE_SECRET_KEY")

# Initialize database
engine = create_engine(os.getenv("DATABASE_URL"))
Session = sessionmaker(bind=engine)


def restore_subscription(customer_id: str):
    """Restore subscription data from Stripe."""

    print(f"🔍 Fetching customer {customer_id} from Stripe...")
    try:
        customer = stripe.Customer.retrieve(customer_id)
    except stripe.error.StripeError as e:
        print(f"❌ Stripe error: {e}")
        return

    print(f"✅ Found customer: {customer.email}")
    print(f"   Metadata: {customer.metadata}")

    # Get subscriptions
    print(f"🔍 Fetching subscriptions...")
    subscriptions = stripe.Subscription.list(customer=customer_id, limit=10)

    if not subscriptions.data:
        print(f"❌ No subscriptions found for customer {customer_id}")
        return

    # Find active subscription
    active_sub = None
    for sub in subscriptions.data:
        print(f"   Subscription {sub.id}: status={sub.status}")
        if sub.status in ["active", "trialing"]:
            active_sub = sub
            break

    if not active_sub:
        print(f"❌ No active subscription found")
        return

    print(f"✅ Found active subscription: {active_sub.id}")

    # Parse subscription data
    metadata = active_sub.get("metadata", {})
    tier = metadata.get("tier", "free")
    billing_cycle = metadata.get("billing_cycle")
    status = active_sub.status

    # Get items
    items = active_sub.get("items", {}).get("data", [])
    user_limit = 1
    if tier == "team":
        user_limit = 5

    # Get period dates
    period_start = None
    period_end = None
    if items:
        first_item = items[0]
        if first_item.get("current_period_start"):
            period_start = datetime.fromtimestamp(first_item["current_period_start"], tz=timezone.utc)
        if first_item.get("current_period_end"):
            period_end = datetime.fromtimestamp(first_item["current_period_end"], tz=timezone.utc)

    print(f"📊 Subscription data:")
    print(f"   Tier: {tier}")
    print(f"   Status: {status}")
    print(f"   Billing cycle: {billing_cycle}")
    print(f"   User limit: {user_limit}")
    print(f"   Period: {period_start} to {period_end}")

    # Find workspace
    print(f"🔍 Finding workspace with customer_id {customer_id}...")
    session = Session()

    try:
        # Direct SQL query to avoid model imports
        result = session.execute(
            text("SELECT workspace_id, name, subscription_tier, subscription_status, stripe_subscription_id "
                 "FROM workspaces WHERE stripe_customer_id = :customer_id"),
            {"customer_id": customer_id}
        ).fetchone()

        if not result:
            # Try finding by workspace_id in metadata
            workspace_id = customer.metadata.get("workspace_id")
            if workspace_id:
                print(f"   Checking metadata workspace_id: {workspace_id}")
                result = session.execute(
                    text("SELECT workspace_id, name, subscription_tier, subscription_status, stripe_subscription_id "
                         "FROM workspaces WHERE workspace_id = :workspace_id"),
                    {"workspace_id": workspace_id}
                ).fetchone()

        if not result:
            print(f"❌ No workspace found")
            return

        workspace_id_db, name, current_tier, current_status, current_sub_id = result
        print(f"✅ Found workspace: {workspace_id_db} ({name})")
        print(f"📊 Current workspace state:")
        print(f"   subscription_tier: {current_tier}")
        print(f"   subscription_status: {current_status}")
        print(f"   stripe_subscription_id: {current_sub_id}")

        # Update workspace
        print(f"💾 Updating workspace with Stripe data...")
        session.execute(
            text("""
            UPDATE workspaces SET
                subscription_tier = :tier,
                subscription_status = :status,
                billing_cycle = :billing_cycle,
                subscription_period_start = :period_start,
                subscription_period_end = :period_end,
                stripe_subscription_id = :subscription_id,
                stripe_customer_id = :customer_id,
                user_limit = :user_limit,
                updated_at = NOW()
            WHERE workspace_id = :workspace_id
            """),
            {
                "tier": tier,
                "status": status,
                "billing_cycle": billing_cycle,
                "period_start": period_start,
                "period_end": period_end,
                "subscription_id": active_sub.id,
                "customer_id": customer_id,
                "user_limit": user_limit,
                "workspace_id": workspace_id_db
            }
        )
        session.commit()

        print(f"✅ Successfully restored subscription data!")

        # Show final state
        result = session.execute(
            text("SELECT subscription_tier, subscription_status, billing_cycle, stripe_subscription_id, user_limit "
                 "FROM workspaces WHERE workspace_id = :workspace_id"),
            {"workspace_id": workspace_id_db}
        ).fetchone()

        print(f"📊 Updated workspace state:")
        print(f"   subscription_tier: {result[0]}")
        print(f"   subscription_status: {result[1]}")
        print(f"   billing_cycle: {result[2]}")
        print(f"   stripe_subscription_id: {result[3]}")
        print(f"   user_limit: {result[4]}")

    except Exception as e:
        session.rollback()
        print(f"❌ Database error: {e}")
        import traceback
        traceback.print_exc()
    finally:
        session.close()


if __name__ == "__main__":
    if len(sys.argv) != 2:
        print("Usage: python restore_subscription_simple.py <stripe_customer_id>")
        print("Example: python restore_subscription_simple.py cus_TSjvJq5bWVtM6e")
        sys.exit(1)

    customer_id = sys.argv[1]
    restore_subscription(customer_id)
