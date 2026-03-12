// SPDX-License-Identifier: AGPL-3.0-or-later
// snowflake/schema.js
import {
  accountField,
  roleField,
  warehouseField,
  schemaField,
  discoveryField,
  passwordAuthMode,
  oauthAuthMode,
} from '../shared/schemas';

export const schema = {
  type: 'snowflake',
  label: 'Snowflake',

  // Step 1: Connection fields (just the account identifier)
  connectionFields: [
    accountField(),
  ],

  // Step 2: Discovery fields (become dropdowns after successful connect)
  discoveryFields: [
    warehouseField(),
    discoveryField('database', 'databases', 'Default Database', {
      optional: true,
      gridColumn: 2,
    }),
    schemaField('PUBLIC', {
      optional: true,
      helpText: 'Default schema for queries (usually "PUBLIC")',
    }),
    roleField(),
  ],

  // Snowflake supports password and OAuth authentication
  authModes: [
    passwordAuthMode({ isDefault: true }),
    oauthAuthMode('Snowflake', {
      callbackPath: '/auth/oauth/snowflake/callback',
    }),
  ],

  // Catalog configuration
  catalogConfig: {
    key: 'catalog_databases',
    label: 'Databases to Index',
    discoveryKey: 'databases',
    helpText: 'Select which databases to include in the catalog.',
  },

  sshTunnelSupported: false,
};
