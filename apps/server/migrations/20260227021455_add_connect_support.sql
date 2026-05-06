-- Add connection_type to datasource_configs
-- 'direct' = credentials in Kyomi (default, includes SSH tunnel)
-- 'connect' = credentials on customer side via Kyomi Connect
ALTER TABLE datasource_configs
    ADD COLUMN connection_type VARCHAR(20) NOT NULL DEFAULT 'direct';

-- Store the current valid JWT token ID for Connect datasources
-- NULL for direct connections
ALTER TABLE datasource_configs
    ADD COLUMN connect_token_jti VARCHAR(64);

-- Index for fast token lookup during WebSocket handshake
CREATE INDEX idx_datasource_configs_connect_jti
    ON datasource_configs(connect_token_jti)
    WHERE connect_token_jti IS NOT NULL;
