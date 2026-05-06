-- Initialize Kyomi database
-- This script runs automatically when PostgreSQL container starts for the first time

-- Enable necessary extensions
CREATE EXTENSION IF NOT EXISTS "uuid-ossp";
CREATE EXTENSION IF NOT EXISTS "pg_trgm";
CREATE EXTENSION IF NOT EXISTS "vector";

-- Create application user (if needed for production)
-- For development, we use the default 'kyomi' user created by Docker

-- Grant necessary permissions
GRANT USAGE, CREATE ON SCHEMA public TO kyomi;
GRANT ALL PRIVILEGES ON ALL TABLES IN SCHEMA public TO kyomi;
GRANT ALL PRIVILEGES ON ALL SEQUENCES IN SCHEMA public TO kyomi;

-- Set default privileges for future objects
ALTER DEFAULT PRIVILEGES IN SCHEMA public GRANT ALL ON TABLES TO kyomi;
ALTER DEFAULT PRIVILEGES IN SCHEMA public GRANT ALL ON SEQUENCES TO kyomi;

-- Create basic database structure will be handled by SQLAlchemy
-- This script just ensures the database is ready for the application

COMMENT ON DATABASE kyomi IS 'Kyomi AI-powered data analysis platform database';