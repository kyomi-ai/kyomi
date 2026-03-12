// SPDX-License-Identifier: AGPL-3.0-or-later
// Import all providers
import postgres from './postgres';
import mysql from './mysql';
import clickhouse from './clickhouse';
import snowflake from './snowflake';
import bigquery from './bigquery';
import databricks from './databricks';
import redshift from './redshift';
import sqlserver from './sqlserver';
import synapse from './synapse';

// Registry of all providers
export const providers = {
  postgres,
  mysql,
  clickhouse,
  snowflake,
  bigquery,
  databricks,
  redshift,
  sqlserver,
  synapse,
};

// Get provider by type
export function getProvider(type) {
  return providers[type] || null;
}

// Get all provider types for dropdown
export function getProviderTypes() {
  return Object.entries(providers).map(([type, provider]) => ({
    value: type,
    label: provider.label,
  }));
}

// Get default auth mode for a provider
export function getDefaultAuthMode(type) {
  const provider = getProvider(type);
  if (!provider?.schema?.authModes) return 'password';
  const defaultMode = provider.schema.authModes.find(m => m.isDefault);
  return defaultMode?.value || provider.schema.authModes[0]?.value || 'password';
}

// Re-export the DatasourceModal
export { default as DatasourceModal } from './DatasourceModal';
