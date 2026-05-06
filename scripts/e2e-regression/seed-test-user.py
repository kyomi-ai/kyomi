#!/usr/bin/env python3
"""
Seed E2E test users into the Kyomi database.

Creates two users:
  - e2e-test@kyomi.dev    (regular workspace admin) — used by most tests
  - e2e-admin@kyomi.dev   (workspace owner/admin)   — used for admin-only flows

Both users are placed in the same shared workspace.

Requirements:
  pip3 install argon2-cffi psycopg2-binary

Usage:
  python3 scripts/e2e-regression/seed-test-user.py
  # Or with custom DB URL:
  DATABASE_URL=postgresql://... python3 scripts/e2e-regression/seed-test-user.py
"""

import os
import sys
import uuid
import psycopg2
from argon2 import PasswordHasher
import json
from datetime import datetime, timedelta, timezone

# ── Config ─────────────────────────────────────────────────────────────────────

DATABASE_URL = os.environ.get(
    "DATABASE_URL",
    "postgresql://kyomi:password@localhost:5433/kyomi"
)

TEST_USERS = [
    {
        "email": "e2e-test@kyomi.dev",
        "password": "E2eTestPass123!",
        "name": "E2E Test User",
        "role": "workspace_admin",
        "is_owner": False,
    },
    {
        "email": "e2e-admin@kyomi.dev",
        "password": "E2eAdminPass123!",
        "name": "E2E Admin User",
        "role": "workspace_admin",
        "is_owner": True,
    },
]

WORKSPACE_ID = "e2e-test-workspace-0001"
WORKSPACE_NAME = "E2E Test Workspace"

# ── Helpers ─────────────────────────────────────────────────────────────────────

def make_user_id():
    return "usr_" + uuid.uuid4().hex[:20]

def make_message_id():
    return "msg_" + uuid.uuid4().hex[:20]

def make_session_id():
    return "ses_" + uuid.uuid4().hex[:20]

def hash_password(pw: str) -> str:
    ph = PasswordHasher(time_cost=2, memory_cost=19456, parallelism=1)
    return ph.hash(pw)

# ChartML content for seed message — inline data, no datasource required.
CHART_SESSION_TITLE = "E2E Chart Info Test Session"
CHART_MESSAGE_CONTENT = """\
Here is a chart showing monthly revenue:

```chartml
type: chart
version: 1
title: "Monthly Revenue"

data:
  provider: inline
  rows:
    - month: "Jan"
      revenue: 125000
    - month: "Feb"
      revenue: 138000
    - month: "Mar"
      revenue: 152000

visualize:
  type: bar
  columns: month
  rows: revenue
```

The data shows steady growth over the first quarter.
"""

# ── Main ────────────────────────────────────────────────────────────────────────

def seed():
    ph = PasswordHasher(time_cost=2, memory_cost=19456, parallelism=1)
    conn = psycopg2.connect(DATABASE_URL)
    cur = conn.cursor()
    now = datetime.now(timezone.utc)

    # Determine workspace owner (admin user)
    owner_user = next(u for u in TEST_USERS if u["is_owner"])

    # Upsert each user
    owner_user_id = None
    user_ids = {}

    for user in TEST_USERS:
        # Check if user exists
        cur.execute("SELECT user_id FROM users WHERE email = %s", (user["email"],))
        row = cur.fetchone()
        if row:
            user_id = row[0]
            print(f"  ✓ User already exists: {user['email']} ({user_id})")
        else:
            user_id = make_user_id()
            cur.execute("""
                INSERT INTO users (
                    user_id, email, name, active, verified,
                    terms_accepted_at, terms_accepted_version, created_at, updated_at
                ) VALUES (%s, %s, %s, true, true, %s, '1.0', %s, %s)
            """, (user_id, user["email"], user["name"], now, now, now))
            print(f"  ✓ Created user: {user['email']} ({user_id})")

        user_ids[user["email"]] = user_id
        if user["is_owner"]:
            owner_user_id = user_id

        # Upsert password auth method
        pw_hash = ph.hash(user["password"])
        auth_data = json.dumps({"hash": pw_hash})
        cur.execute("""
            INSERT INTO user_auth_methods (user_id, auth_type, auth_data, active, created_at)
            VALUES (%s, 'password', %s, true, %s)
            ON CONFLICT (user_id, auth_type) DO UPDATE
                SET auth_data = EXCLUDED.auth_data,
                    active = true
        """, (user_id, auth_data, now))
        print(f"  ✓ Password auth set for: {user['email']}")

    # Upsert workspace
    cur.execute("SELECT workspace_id FROM workspaces WHERE workspace_id = %s", (WORKSPACE_ID,))
    if cur.fetchone():
        print(f"  ✓ Workspace already exists: {WORKSPACE_NAME}")
    else:
        trial_ends_at = now + timedelta(days=30)
        cur.execute("""
            INSERT INTO workspaces (
                workspace_id, name, admin_email, owner_user_id, status,
                subscription_tier, subscription_status, trial_ends_at,
                user_limit, ai_bundle_balance_usd, created_at, updated_at
            ) VALUES (%s, NULL, %s, %s, 'trial', 'cloud', 'trialing', %s, NULL, %s, %s, %s)
        """, (WORKSPACE_ID, owner_user["email"], owner_user_id, trial_ends_at, 5.0, now, now))
        print(f"  ✓ Created workspace: {WORKSPACE_NAME} ({WORKSPACE_ID})")

    # Set last_workspace_id on users
    for user in TEST_USERS:
        uid = user_ids[user["email"]]
        cur.execute(
            "UPDATE users SET last_workspace_id = %s WHERE user_id = %s",
            (WORKSPACE_ID, uid)
        )

    # Upsert workspace_users entries
    for user in TEST_USERS:
        uid = user_ids[user["email"]]
        cur.execute("""
            INSERT INTO workspace_users (workspace_id, user_id, role, active, created_at)
            VALUES (%s, %s, %s, true, %s)
            ON CONFLICT (workspace_id, user_id) DO UPDATE
                SET role = EXCLUDED.role,
                    active = true
        """, (WORKSPACE_ID, uid, user["role"], now))
        print(f"  ✓ Added {user['email']} to workspace as {user['role']}")

    # Seed chart info test session (chat-adv-08)
    # Creates a session with a ChartML assistant message so the chart info
    # button is visible without needing a live LLM response.
    e2e_user_id = user_ids["e2e-test@kyomi.dev"]
    cur.execute(
        "SELECT session_id FROM chat_sessions WHERE title = %s AND workspace_id = %s",
        (CHART_SESSION_TITLE, WORKSPACE_ID)
    )
    chart_session_row = cur.fetchone()
    if chart_session_row:
        chart_session_id = chart_session_row[0]
        print(f"  ✓ Chart session already exists: {CHART_SESSION_TITLE} ({chart_session_id})")
    else:
        chart_session_id = make_session_id()
        cur.execute("""
            INSERT INTO chat_sessions (
                session_id, user_id, workspace_id, title,
                session_type, created_at, updated_at
            ) VALUES (%s, %s, %s, %s, 'chat', %s, %s)
        """, (chart_session_id, e2e_user_id, WORKSPACE_ID, CHART_SESSION_TITLE, now, now))
        print(f"  ✓ Created chart session: {CHART_SESSION_TITLE} ({chart_session_id})")

        # Seed user message
        user_msg_id = make_message_id()
        cur.execute("""
            INSERT INTO chat_messages (
                message_id, session_id, role, content,
                pinned, created_at
            ) VALUES (%s, %s, 'user', %s, false, %s)
        """, (user_msg_id, chart_session_id, "Show me a revenue chart", now))

        # Seed assistant message with ChartML block
        asst_msg_id = make_message_id()
        cur.execute("""
            INSERT INTO chat_messages (
                message_id, session_id, role, content,
                pinned, created_at
            ) VALUES (%s, %s, 'assistant', %s, false, %s)
        """, (asst_msg_id, chart_session_id, CHART_MESSAGE_CONTENT, now))
        print(f"  ✓ Seeded ChartML assistant message into chart session")

    conn.commit()
    cur.close()
    conn.close()
    print()
    print("✅ E2E test users seeded successfully.")
    print()
    print("Credentials:")
    for user in TEST_USERS:
        print(f"  {user['email']}  /  {user['password']}")

if __name__ == "__main__":
    print("Seeding E2E test users...")
    try:
        seed()
    except Exception as e:
        print(f"\n❌ Seed failed: {e}", file=sys.stderr)
        sys.exit(1)
