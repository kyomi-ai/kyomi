// SPDX-License-Identifier: AGPL-3.0-or-later
// clickhouse/schema.js
import {
  hostField,
  portField,
  discoveryField,
  secureField,
  passwordAuthMode,
} from '../shared/schemas';

export const schema = {
  type: 'clickhouse',
  label: 'ClickHouse',

  // Step 1: Connection fields (text inputs, required for connect)
  connectionFields: [
    hostField({ placeholder: 'clickhouse.example.com' }),
    portField(8123, {
      helpText: 'HTTP port (typically 8123 for HTTP, 8443 for HTTPS)',
    }),
    secureField({
      label: 'Secure (HTTPS)',
      description: 'Use HTTPS connection',
      defaultValue: false,
      gridColumn: 'full',
    }),
  ],

  // Step 2: Discovery fields (become dropdowns after successful connect)
  // ClickHouse uses databases, not schemas
  discoveryFields: [
    discoveryField('database', 'databases', 'Default Database', {
      required: true,
      gridColumn: 'full',
      helpText: 'Default database for queries',
    }),
  ],

  // Uses standard password authentication
  credentialFields: passwordAuthMode().credentialFields,
  supportsSharedCredentials: true,

  // Catalog configuration
  catalogConfig: {
    key: 'catalog_databases',
    label: 'Databases to Index',
    discoveryKey: 'databases',
    helpText: 'Select which databases to include in the catalog. Leave empty to index all accessible databases.',
  },

  sshTunnelSupported: true,
  connectSupported: true,
};
