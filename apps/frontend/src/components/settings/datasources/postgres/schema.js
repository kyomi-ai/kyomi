// SPDX-License-Identifier: AGPL-3.0-or-later
// postgres/schema.js
import {
  hostField,
  portField,
  sslModeField,
  discoveryField,
  schemaField,
  passwordAuthMode,
} from '../shared/schemas';

export const schema = {
  type: 'postgres',
  label: 'PostgreSQL',

  // Step 1: Connection fields (text inputs, required for connect)
  connectionFields: [
    hostField({ placeholder: 'db.example.com' }),
    portField(5432),
    sslModeField(),
  ],

  // Step 2: Discovery fields (become dropdowns after successful connect)
  discoveryFields: [
    discoveryField('database', 'databases', 'Database', {
      required: true,
      gridColumn: 1,
    }),
    schemaField('public', {
      label: 'Default Schema',
      optional: true,
      helpText: 'Default schema for queries (usually "public")',
    }),
  ],

  // Uses standard password authentication
  credentialFields: passwordAuthMode().credentialFields,
  supportsSharedCredentials: true,

  // Catalog configuration
  catalogConfig: {
    key: 'catalog_schemas',
    label: 'Schemas to Index',
    discoveryKey: 'schemas',
    helpText: 'Select which schemas to include in the catalog. Leave empty to index all accessible schemas.',
  },

  sshTunnelSupported: true,
  connectSupported: true,
};
