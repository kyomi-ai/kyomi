-- Add clickhouse_database column to analytics_sites
ALTER TABLE analytics_sites
  ADD COLUMN clickhouse_database TEXT;
