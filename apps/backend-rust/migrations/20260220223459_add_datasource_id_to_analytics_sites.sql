-- Link analytics sites to their auto-provisioned datasource
ALTER TABLE analytics_sites
  ADD COLUMN datasource_id VARCHAR(50) REFERENCES datasource_configs(id) ON DELETE SET NULL;
