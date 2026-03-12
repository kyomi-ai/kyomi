// SPDX-License-Identifier: AGPL-3.0-or-later
// mysql/schema.js
import {
  hostField,
  portField,
  discoveryField,
  passwordAuthMode,
  FIELD_TYPES,
} from '../shared/schemas';

/**
 * MySQL SSL mode options.
 * These map to MySQL/MariaDB ssl-mode connection parameter.
 */
const MYSQL_SSL_OPTIONS = [
  { value: 'disable', label: 'Disable' },
  { value: 'require', label: 'Require (Recommended)' },
  { value: 'verify-ca', label: 'Verify CA' },
  { value: 'verify-full', label: 'Verify Full' },
];

export const schema = {
  type: 'mysql',
  label: 'MySQL',

  // Step 1: Connection fields (text inputs, required for connect)
  connectionFields: [
    hostField({ placeholder: 'mysql.example.com' }),
    portField(3306),
    {
      name: 'ssl_mode',
      type: FIELD_TYPES.SELECT,
      label: 'SSL Mode',
      defaultValue: 'require',
      gridColumn: 'full',
      options: MYSQL_SSL_OPTIONS,
    },
  ],

  // Step 2: Discovery fields (become dropdowns after successful connect)
  // Note: MySQL uses databases directly (no separate schema concept like PostgreSQL)
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
