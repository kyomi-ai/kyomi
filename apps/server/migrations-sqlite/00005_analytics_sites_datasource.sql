-- Link analytics sites to their auto-provisioned datasource
ALTER TABLE analytics_sites
  ADD COLUMN datasource_id TEXT REFERENCES datasource_configs(id) ON DELETE SET NULL;
