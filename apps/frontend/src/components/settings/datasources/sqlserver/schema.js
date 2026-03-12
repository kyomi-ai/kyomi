// SPDX-License-Identifier: AGPL-3.0-or-later
// sqlserver/schema.js
import {
  hostField,
  portField,
  discoveryField,
  schemaField,
  encryptField,
  trustServerCertificateField,
  passwordAuthMode,
} from '../shared/schemas';

export const schema = {
  type: 'sqlserver',
  label: 'SQL Server',

  // Step 1: Connection fields (text inputs, required for connect)
  connectionFields: [
    hostField({
      placeholder: 'sqlserver.example.com',
    }),
    portField(1433),
    encryptField({
      description: 'Use TLS encryption',
    }),
    trustServerCertificateField({
      description: 'Trust self-signed certificates',
    }),
  ],

  // Step 2: Discovery fields (become dropdowns after successful connect)
  discoveryFields: [
    discoveryField('database', 'databases', 'Database', {
      required: true,
      gridColumn: 1,
    }),
    schemaField('dbo', {
      label: 'Default Schema',
      optional: true,
      helpText: 'Default schema for queries (usually "dbo")',
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
