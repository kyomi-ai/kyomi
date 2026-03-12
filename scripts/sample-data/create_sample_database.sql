-- Sample ClickHouse Database DDL for "Acme Analytics"
-- A fictional SaaS company's analytics data for the Try Before Signup feature

-- Create database
CREATE DATABASE IF NOT EXISTS acme_analytics;

-- Subscriptions table: Customer subscription data
CREATE TABLE IF NOT EXISTS acme_analytics.subscriptions
(
    subscription_id String,
    customer_id String,
    plan_name String,  -- 'free', 'starter', 'professional', 'enterprise'
    status String,     -- 'active', 'churned', 'paused'
    mrr Float64,       -- Monthly recurring revenue in USD
    billing_cycle String,  -- 'monthly', 'annual'
    start_date Date,
    end_date Nullable(Date),
    created_at DateTime DEFAULT now(),
    updated_at DateTime DEFAULT now()
)
ENGINE = MergeTree()
ORDER BY (customer_id, start_date)
SETTINGS index_granularity = 8192;

-- Users table: User accounts
CREATE TABLE IF NOT EXISTS acme_analytics.users
(
    user_id String,
    customer_id String,
    email String,
    name String,
    role String,  -- 'admin', 'member', 'viewer'
    signup_date Date,
    last_activity DateTime,
    created_at DateTime DEFAULT now()
)
ENGINE = MergeTree()
ORDER BY (customer_id, signup_date)
SETTINGS index_granularity = 8192;

-- Events table: Product usage events
CREATE TABLE IF NOT EXISTS acme_analytics.events
(
    event_id String,
    user_id String,
    event_type String,  -- 'login', 'export', 'dashboard_view', 'report_run', 'chart_created', 'invite_sent'
    timestamp DateTime,
    properties String,  -- JSON-encoded properties
    session_id String
)
ENGINE = MergeTree()
ORDER BY (user_id, timestamp)
SETTINGS index_granularity = 8192;

-- Website Sessions table: Marketing funnel data
CREATE TABLE IF NOT EXISTS acme_analytics.website_sessions
(
    session_id String,
    landing_page String,  -- '/pricing', '/features', '/demo', '/blog', '/'
    referrer String,      -- 'google', 'linkedin', 'twitter', 'direct', 'producthunt', 'other'
    utm_source Nullable(String),
    duration_seconds Int32,
    converted UInt8,      -- 1 = signed up, 0 = didn't
    timestamp DateTime
)
ENGINE = MergeTree()
ORDER BY (timestamp, landing_page)
SETTINGS index_granularity = 8192;

-- Create read-only user for trial access
-- Note: This should be run as admin user
-- CREATE USER IF NOT EXISTS sample_readonly IDENTIFIED BY 'readonly_password_here';
-- GRANT SELECT ON acme_analytics.* TO sample_readonly;
