#!/usr/bin/env python3
"""
Restore subscription data from Stripe for a given customer ID.

This script:
1. Queries Stripe for the customer's active subscription
2. Finds the workspace in Kyomi with that customer ID
3. Updates the workspace with the correct subscription data
"""

import sys
import os
from pathlib import Path

# Add backend to Python path
backend_path = Path(__file__).parent.parent.parent / "apps" / "backend"
sys.path.insert(0, str(backend_path / "src" / "api"))

from sqlalchemy.orm import Session
from database.connection import init_database, get_session_sync
from database.models import Workspace
from usage.stripe_service import StripeService
import asyncio
import logging

logging.basicConfig(level=logging.INFO)
logger = logging.getLogger(__name__)


async def restore_subscription(customer_id: str):
    """Restore subscription data from Stripe."""

    # Initialize Stripe service
    stripe_service = StripeService()

    try:
        # Import stripe to query directly
        import stripe

        # Get customer from Stripe
        logger.info(f"🔍 Fetching customer {customer_id} from Stripe...")
        customer = stripe.Customer.retrieve(customer_id)
        logger.info(f"✅ Found customer: {customer.email}")
        logger.info(f"   Metadata: {customer.metadata}")

        # Get subscriptions for this customer
        logger.info(f"🔍 Fetching subscriptions...")
        subscriptions = stripe.Subscription.list(customer=customer_id, limit=10)

        if not subscriptions.data:
            logger.error(f"❌ No subscriptions found for customer {customer_id}")
            return

        # Find active subscription
        active_sub = None
        for sub in subscriptions.data:
            logger.info(f"   Subscription {sub.id}: status={sub.status}")
            if sub.status in ["active", "trialing"]:
                active_sub = sub
                break

        if not active_sub:
            logger.error(f"❌ No active subscription found")
            logger.info(f"   All subscriptions: {[f'{s.id} ({s.status})' for s in subscriptions.data]}")
            return

        logger.info(f"✅ Found active subscription: {active_sub.id}")

        # Parse subscription data
        sub_data = stripe_service.parse_subscription_data(active_sub)
        logger.info(f"📊 Subscription data:")
        logger.info(f"   Tier: {sub_data['tier']}")
        logger.info(f"   Status: {sub_data['status']}")
        logger.info(f"   Billing cycle: {sub_data['billing_cycle']}")
        logger.info(f"   User limit: {sub_data['user_limit']}")
        logger.info(f"   Period: {sub_data['period_start']} to {sub_data['period_end']}")

        # Initialize database
        init_database()

        # Find workspace in database
        logger.info(f"🔍 Finding workspace with customer_id {customer_id}...")
        db: Session = get_session_sync()
        try:
            workspace = db.query(Workspace).filter(
                Workspace.stripe_customer_id == customer_id
            ).first()

            if not workspace:
                logger.error(f"❌ No workspace found with stripe_customer_id={customer_id}")
                logger.info(f"   Checking metadata for workspace_id...")
                workspace_id = customer.metadata.get("workspace_id")
                if workspace_id:
                    logger.info(f"   Found workspace_id in metadata: {workspace_id}")
                    workspace = db.query(Workspace).filter(
                        Workspace.workspace_id == workspace_id
                    ).first()
                    if workspace:
                        logger.info(f"✅ Found workspace by ID: {workspace.workspace_id} ({workspace.name})")
                    else:
                        logger.error(f"❌ Workspace {workspace_id} not found in database")
                        return
                else:
                    logger.error(f"   No workspace_id in customer metadata")
                    return
            else:
                logger.info(f"✅ Found workspace: {workspace.workspace_id} ({workspace.name})")

            # Show current state
            logger.info(f"📊 Current workspace state:")
            logger.info(f"   subscription_tier: {workspace.subscription_tier}")
            logger.info(f"   subscription_status: {workspace.subscription_status}")
            logger.info(f"   stripe_subscription_id: {workspace.stripe_subscription_id}")
            logger.info(f"   stripe_customer_id: {workspace.stripe_customer_id}")

            # Update workspace
            logger.info(f"💾 Updating workspace with Stripe data...")
            workspace.subscription_tier = sub_data["tier"]
            workspace.subscription_status = sub_data["status"]
            workspace.billing_cycle = sub_data["billing_cycle"]
            workspace.subscription_period_start = sub_data["period_start"]
            workspace.subscription_period_end = sub_data["period_end"]
            workspace.stripe_subscription_id = sub_data["stripe_subscription_id"]
            workspace.stripe_customer_id = sub_data["stripe_customer_id"]
            workspace.stripe_additional_users_item_id = sub_data.get("additional_users_item_id")
            workspace.user_limit = sub_data["user_limit"]

            db.commit()
            logger.info(f"✅ Successfully restored subscription data!")

            # Show final state
            logger.info(f"📊 Updated workspace state:")
            logger.info(f"   subscription_tier: {workspace.subscription_tier}")
            logger.info(f"   subscription_status: {workspace.subscription_status}")
            logger.info(f"   billing_cycle: {workspace.billing_cycle}")
            logger.info(f"   stripe_subscription_id: {workspace.stripe_subscription_id}")
            logger.info(f"   stripe_customer_id: {workspace.stripe_customer_id}")
            logger.info(f"   user_limit: {workspace.user_limit}")

        finally:
            db.close()

    except Exception as e:
        logger.error(f"❌ Error: {e}")
        import traceback
        traceback.print_exc()


if __name__ == "__main__":
    if len(sys.argv) != 2:
        print("Usage: python restore_subscription.py <stripe_customer_id>")
        print("Example: python restore_subscription.py cus_TSjvJq5bWVtM6e")
        sys.exit(1)

    customer_id = sys.argv[1]
    asyncio.run(restore_subscription(customer_id))
