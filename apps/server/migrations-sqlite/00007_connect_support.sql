-- Add connection_type to datasource_configs
-- 'direct' = credentials in Kyomi (default, includes SSH tunnel)
-- 'connect' = credentials on customer side via Kyomi Connect
ALTER TABLE datasource_configs
    ADD COLUMN connection_type TEXT NOT NULL DEFAULT 'direct';

-- Store the current valid JWT token ID for Connect datasources
-- NULL for direct connections
ALTER TABLE datasource_configs
    ADD COLUMN connect_token_jti TEXT;

-- Index for fast token lookup during WebSocket handshake
-- Note: SQLite partial indexes use WHERE clause
CREATE INDEX IF NOT EXISTS idx_datasource_configs_connect_jti
    ON datasource_configs(connect_token_jti)
    WHERE connect_token_jti IS NOT NULL;
