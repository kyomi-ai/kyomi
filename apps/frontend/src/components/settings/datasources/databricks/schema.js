// SPDX-License-Identifier: AGPL-3.0-or-later
// databricks/schema.js
import {
  serverHostnameField,
  httpPathField,
  catalogField,
  schemaField,
  tokenAuthMode,
  oauthAuthMode,
} from '../shared/schemas';

export const schema = {
  type: 'databricks',
  label: 'Databricks',

  // Step 1: Connection fields (text inputs, required for connect)
  connectionFields: [
    serverHostnameField({
      placeholder: 'dbc-xxxxxxxx-xxxx.cloud.databricks.com',
    }),
    httpPathField({
      placeholder: '/sql/1.0/warehouses/xxxx',
    }),
  ],

  // Step 2: Discovery fields (become dropdowns after successful connect)
  discoveryFields: [
    catalogField({
      helpText: 'Unity Catalog or hive_metastore',
    }),
    schemaField('default', {
      label: 'Default Schema',
      optional: true,
    }),
  ],

  // Databricks supports personal access token and OAuth authentication
  authModes: [
    tokenAuthMode({ isDefault: true }),
    oauthAuthMode('Databricks', {
      callbackPath: '/auth/oauth/databricks/callback',
    }),
  ],

  // Catalog configuration
  catalogConfig: {
    key: 'catalog_catalogs',
    label: 'Catalogs to Index',
    discoveryKey: 'catalogs',
    helpText: 'Select which catalogs to include in the catalog. Leave empty to index all accessible catalogs.',
  },

  sshTunnelSupported: false,
};
