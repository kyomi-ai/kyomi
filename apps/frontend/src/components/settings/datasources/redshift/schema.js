// SPDX-License-Identifier: AGPL-3.0-or-later
// redshift/schema.js
import {
  FIELD_TYPES,
  schemaField,
  passwordAuthMode,
} from '../shared/schemas';

export const schema = {
  type: 'redshift',
  label: 'Amazon Redshift',

  // Single endpoint field - user pastes exactly what AWS gives them
  connectionFields: [
    {
      name: 'endpoint',
      type: FIELD_TYPES.TEXT,
      label: 'Endpoint',
      placeholder: 'cluster.xxxx.region.redshift-serverless.amazonaws.com:5439/database',
      required: true,
      gridColumn: 'full',
      helpText: 'Copy the full endpoint from AWS Redshift console (includes host:port/database)',
    },
  ],

  // Schema discovery after connection
  discoveryFields: [
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
