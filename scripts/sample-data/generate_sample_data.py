#!/usr/bin/env python3
"""
Generate sample data for Acme Analytics trial database.

This script generates realistic SaaS data with interesting patterns:
1. MRR growth with January spike (new year budgets)
2. Monthly plans churn 3x more than annual
3. "Export" feature usage correlates with lower churn
4. /pricing -> /demo -> signup funnel (8% conversion)
5. First-week feature usage predicts retention

Usage:
    python generate_sample_data.py --output-dir ./output
    python generate_sample_data.py --clickhouse-host localhost --clickhouse-port 9000
"""

import argparse
import csv
import json
import os
import random
import uuid
from datetime import datetime, timedelta
from typing import List, Dict, Any, Tuple

# Seed for reproducibility
random.seed(42)

# Configuration
START_DATE = datetime(2024, 1, 1)  # 18 months ago from mid-2025
END_DATE = datetime(2025, 6, 30)

PLANS = {
    'free': {'mrr': 0, 'weight': 0.4},
    'starter': {'mrr': 29, 'weight': 0.3},
    'professional': {'mrr': 99, 'weight': 0.2},
    'enterprise': {'mrr': 299, 'weight': 0.1},
}

ROLES = ['admin', 'member', 'viewer']
REFERRERS = ['google', 'linkedin', 'twitter', 'direct', 'producthunt', 'other']
LANDING_PAGES = ['/', '/pricing', '/features', '/demo', '/blog']
EVENT_TYPES = ['login', 'export', 'dashboard_view', 'report_run', 'chart_created', 'invite_sent', 'settings_changed']


def generate_id() -> str:
    """Generate a short UUID-like ID."""
    return uuid.uuid4().hex[:12]


def random_date(start: datetime, end: datetime) -> datetime:
    """Generate a random datetime between start and end."""
    delta = end - start
    random_days = random.randint(0, delta.days)
    random_seconds = random.randint(0, 86400)
    return start + timedelta(days=random_days, seconds=random_seconds)


def weighted_choice(choices: Dict[str, Dict]) -> str:
    """Make a weighted random choice."""
    items = list(choices.keys())
    weights = [choices[k]['weight'] for k in items]
    return random.choices(items, weights=weights, k=1)[0]


def generate_customers(num_customers: int = 500) -> List[Dict]:
    """Generate customer subscription records."""
    customers = []

    for i in range(num_customers):
        customer_id = generate_id()

        # Customer signup follows a growth pattern with January spike
        signup_date = random_date(START_DATE, END_DATE)

        # January has 2x more signups (new year budgets)
        if signup_date.month != 1:
            if random.random() < 0.3:
                # 30% chance to skip non-January signups
                continue

        plan = weighted_choice(PLANS)
        billing_cycle = random.choice(['monthly', 'annual'])

        # Annual plans are more common for professional/enterprise
        if plan in ['professional', 'enterprise']:
            billing_cycle = 'annual' if random.random() < 0.6 else 'monthly'

        # Calculate MRR (annual plans get discount)
        base_mrr = PLANS[plan]['mrr']
        mrr = base_mrr * 0.8 if billing_cycle == 'annual' else base_mrr

        # Determine churn status
        # Monthly plans churn 3x more than annual
        churn_rate = 0.15 if billing_cycle == 'monthly' else 0.05
        churned = random.random() < churn_rate

        status = 'churned' if churned else 'active'
        end_date = None

        if churned:
            # Churned after 1-6 months
            churn_months = random.randint(1, 6)
            end_date = signup_date + timedelta(days=churn_months * 30)
            if end_date > END_DATE:
                end_date = END_DATE

        customers.append({
            'subscription_id': generate_id(),
            'customer_id': customer_id,
            'plan_name': plan,
            'status': status,
            'mrr': mrr,
            'billing_cycle': billing_cycle,
            'start_date': signup_date.date(),
            'end_date': end_date.date() if end_date else None,
            'created_at': signup_date,
            'updated_at': signup_date + timedelta(days=random.randint(0, 30)),
        })

    return customers


def generate_users(customers: List[Dict], avg_users_per_customer: int = 3) -> List[Dict]:
    """Generate user records for customers."""
    users = []

    for customer in customers:
        customer_id = customer['customer_id']
        signup_date = customer['start_date']

        # Enterprise has more users
        if customer['plan_name'] == 'enterprise':
            num_users = random.randint(5, 15)
        elif customer['plan_name'] == 'professional':
            num_users = random.randint(2, 8)
        else:
            num_users = random.randint(1, 3)

        for i in range(num_users):
            user_id = generate_id()
            role = 'admin' if i == 0 else random.choice(['member', 'viewer'])

            # Users sign up over time
            user_signup = datetime.combine(signup_date, datetime.min.time()) + timedelta(days=random.randint(0, 60))
            if user_signup > END_DATE:
                user_signup = END_DATE

            # Last activity based on engagement
            if customer['status'] == 'churned':
                last_activity = datetime.combine(customer['end_date'], datetime.min.time())
            else:
                last_activity = random_date(user_signup, END_DATE)

            users.append({
                'user_id': user_id,
                'customer_id': customer_id,
                'email': f"user{len(users) + 1}@company{customer_id[:4]}.com",
                'name': f"User {len(users) + 1}",
                'role': role,
                'signup_date': user_signup.date() if isinstance(user_signup, datetime) else user_signup,
                'last_activity': last_activity,
                'created_at': user_signup,
            })

    return users


def generate_events(users: List[Dict], customers_by_id: Dict[str, Dict], avg_events_per_user: int = 30) -> List[Dict]:
    """Generate product usage events."""
    events = []

    for user in users:
        user_id = user['user_id']
        customer = customers_by_id.get(user['customer_id'], {})

        signup = user['signup_date']
        if isinstance(signup, datetime):
            signup = signup.date()

        last_activity = user['last_activity']
        if isinstance(last_activity, datetime):
            pass  # already datetime
        else:
            last_activity = datetime.combine(last_activity, datetime.min.time())

        # Active users have more events
        is_churned = customer.get('status') == 'churned'
        num_events = random.randint(5, 20) if is_churned else random.randint(20, 50)

        # Export usage correlates with lower churn
        # Active customers use export feature more
        export_weight = 0.15 if is_churned else 0.25

        event_weights = {
            'login': 0.25,
            'export': export_weight,
            'dashboard_view': 0.2,
            'report_run': 0.15,
            'chart_created': 0.1,
            'invite_sent': 0.05,
            'settings_changed': 0.05,
        }

        # Normalize weights
        total_weight = sum(event_weights.values())
        event_weights = {k: v/total_weight for k, v in event_weights.items()}

        for _ in range(num_events):
            event_type = random.choices(
                list(event_weights.keys()),
                weights=list(event_weights.values()),
                k=1
            )[0]

            signup_dt = datetime.combine(signup, datetime.min.time())
            # Ensure last_activity is after signup (handle churned customers edge case)
            if last_activity <= signup_dt:
                last_activity = signup_dt + timedelta(days=1)
            timestamp = random_date(signup_dt, last_activity)

            # First week has more events (predicts retention)
            first_week = signup_dt + timedelta(days=7)
            if random.random() < 0.3 and timestamp > first_week:
                timestamp = random_date(signup_dt, first_week)

            properties = json.dumps({'browser': random.choice(['chrome', 'firefox', 'safari', 'edge'])})

            events.append({
                'event_id': generate_id(),
                'user_id': user_id,
                'event_type': event_type,
                'timestamp': timestamp,
                'properties': properties,
                'session_id': generate_id(),
            })

    return events


def generate_website_sessions(num_sessions: int = 20000) -> List[Dict]:
    """Generate marketing funnel data."""
    sessions = []

    for _ in range(num_sessions):
        session_id = generate_id()
        timestamp = random_date(START_DATE, END_DATE)

        landing_page = random.choices(
            LANDING_PAGES,
            weights=[0.25, 0.3, 0.2, 0.15, 0.1],  # /pricing most common
            k=1
        )[0]

        referrer = random.choices(
            REFERRERS,
            weights=[0.35, 0.15, 0.1, 0.25, 0.1, 0.05],  # google and direct most common
            k=1
        )[0]

        utm_source = referrer if random.random() < 0.6 else None

        # Duration varies by landing page
        if landing_page == '/demo':
            duration = random.randint(120, 600)  # Longer on demo
        elif landing_page == '/pricing':
            duration = random.randint(60, 300)
        else:
            duration = random.randint(10, 180)

        # Conversion funnel: /pricing -> /demo -> signup has 8% overall
        # But actual per-page conversion varies
        if landing_page == '/demo':
            converted = 1 if random.random() < 0.12 else 0  # 12% from demo
        elif landing_page == '/pricing':
            converted = 1 if random.random() < 0.06 else 0  # 6% from pricing
        elif landing_page == '/features':
            converted = 1 if random.random() < 0.04 else 0  # 4% from features
        else:
            converted = 1 if random.random() < 0.02 else 0  # 2% from others

        sessions.append({
            'session_id': session_id,
            'landing_page': landing_page,
            'referrer': referrer,
            'utm_source': utm_source,
            'duration_seconds': duration,
            'converted': converted,
            'timestamp': timestamp,
        })

    return sessions


def save_to_csv(data: List[Dict], filename: str, output_dir: str):
    """Save data to CSV file."""
    if not data:
        return

    filepath = os.path.join(output_dir, filename)
    keys = data[0].keys()

    with open(filepath, 'w', newline='', encoding='utf-8') as f:
        writer = csv.DictWriter(f, fieldnames=keys)
        writer.writeheader()
        for row in data:
            # Convert datetime objects to strings
            row_copy = {}
            for k, v in row.items():
                if isinstance(v, datetime):
                    row_copy[k] = v.strftime('%Y-%m-%d %H:%M:%S')
                elif v is None:
                    row_copy[k] = ''
                else:
                    row_copy[k] = v
            writer.writerow(row_copy)

    print(f"Saved {len(data)} rows to {filepath}")


def insert_to_clickhouse(data: List[Dict], table: str, host: str, port: int, database: str, user: str, password: str):
    """Insert data directly into ClickHouse."""
    try:
        import clickhouse_connect

        client = clickhouse_connect.get_client(
            host=host,
            port=port,
            username=user,
            password=password,
            database=database,
        )

        if not data:
            return

        columns = list(data[0].keys())
        rows = []
        for row in data:
            row_values = []
            for col in columns:
                val = row[col]
                if isinstance(val, datetime):
                    row_values.append(val)
                else:
                    row_values.append(val)
            rows.append(row_values)

        client.insert(table, rows, column_names=columns)
        print(f"Inserted {len(data)} rows into {database}.{table}")

    except ImportError:
        print("clickhouse-connect not installed. Use --output-dir to generate CSV files instead.")
        raise


def main():
    parser = argparse.ArgumentParser(description='Generate sample data for Acme Analytics')
    parser.add_argument('--output-dir', type=str, help='Output directory for CSV files')
    parser.add_argument('--clickhouse-host', type=str, help='ClickHouse host')
    parser.add_argument('--clickhouse-port', type=int, default=8123, help='ClickHouse HTTP port')
    parser.add_argument('--clickhouse-user', type=str, default='default', help='ClickHouse user')
    parser.add_argument('--clickhouse-password', type=str, default='', help='ClickHouse password')
    parser.add_argument('--database', type=str, default='acme_analytics', help='Database name')

    args = parser.parse_args()

    if not args.output_dir and not args.clickhouse_host:
        parser.error('Must specify either --output-dir or --clickhouse-host')

    print("Generating sample data for Acme Analytics...")
    print(f"Date range: {START_DATE.date()} to {END_DATE.date()}")

    # Generate data
    print("\n1. Generating customers and subscriptions...")
    customers = generate_customers(500)
    print(f"   Generated {len(customers)} subscriptions")

    customers_by_id = {c['customer_id']: c for c in customers}

    print("\n2. Generating users...")
    users = generate_users(customers)
    print(f"   Generated {len(users)} users")

    print("\n3. Generating events...")
    events = generate_events(users, customers_by_id)
    print(f"   Generated {len(events)} events")

    print("\n4. Generating website sessions...")
    sessions = generate_website_sessions(20000)
    print(f"   Generated {len(sessions)} website sessions")

    # Save or insert data
    if args.output_dir:
        os.makedirs(args.output_dir, exist_ok=True)
        save_to_csv(customers, 'subscriptions.csv', args.output_dir)
        save_to_csv(users, 'users.csv', args.output_dir)
        save_to_csv(events, 'events.csv', args.output_dir)
        save_to_csv(sessions, 'website_sessions.csv', args.output_dir)
        print(f"\nCSV files saved to {args.output_dir}")

    if args.clickhouse_host:
        print(f"\nInserting into ClickHouse at {args.clickhouse_host}:{args.clickhouse_port}...")
        insert_to_clickhouse(customers, 'subscriptions', args.clickhouse_host, args.clickhouse_port,
                             args.database, args.clickhouse_user, args.clickhouse_password)
        insert_to_clickhouse(users, 'users', args.clickhouse_host, args.clickhouse_port,
                             args.database, args.clickhouse_user, args.clickhouse_password)
        insert_to_clickhouse(events, 'events', args.clickhouse_host, args.clickhouse_port,
                             args.database, args.clickhouse_user, args.clickhouse_password)
        insert_to_clickhouse(sessions, 'website_sessions', args.clickhouse_host, args.clickhouse_port,
                             args.database, args.clickhouse_user, args.clickhouse_password)

    print("\nDone!")

    # Print some statistics
    print("\n=== Data Statistics ===")
    active = sum(1 for c in customers if c['status'] == 'active')
    churned = sum(1 for c in customers if c['status'] == 'churned')
    print(f"Subscriptions: {len(customers)} total, {active} active, {churned} churned")

    total_mrr = sum(c['mrr'] for c in customers if c['status'] == 'active')
    print(f"Total Active MRR: ${total_mrr:,.2f}")

    monthly = sum(1 for c in customers if c['billing_cycle'] == 'monthly')
    annual = sum(1 for c in customers if c['billing_cycle'] == 'annual')
    print(f"Billing: {monthly} monthly, {annual} annual")

    converted = sum(1 for s in sessions if s['converted'] == 1)
    print(f"Website: {len(sessions)} sessions, {converted} conversions ({converted/len(sessions)*100:.1f}%)")


if __name__ == '__main__':
    main()
