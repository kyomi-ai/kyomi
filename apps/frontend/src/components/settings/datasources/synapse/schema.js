// SPDX-License-Identifier: AGPL-3.0-or-later
// synapse/schema.js
import {
  discoveryField,
  usernameField,
  passwordField,
  clientIdField,
  clientSecretField,
  oauthClientIdField,
  oauthClientSecretField,
  enterpriseOAuthAuthMode,
} from '../shared/schemas';

export const schema = {
  type: 'synapse',
  label: 'Azure Synapse',

  // Connection fields - server endpoint and tenant ID for OAuth
  connectionFields: [
    {
      name: 'server',
      type: 'text',
      label: 'Server',
      placeholder: 'my-workspace.sql.azuresynapse.net',
      required: true,
      helpText: 'Synapse workspace SQL endpoint',
    },
    {
      name: 'tenant_id',
      type: 'text',
      label: 'Azure AD Tenant ID',
      placeholder: 'xxxxxxxx-xxxx-xxxx-xxxx-xxxxxxxxxxxx',
      required: false,
      helpText: 'Required for Microsoft OAuth. Find in Azure Portal → Directory ID.',
    },
  ],

  // Discovery fields - just database selection
  discoveryFields: [
    discoveryField('database', 'databases', 'Database', {
      required: true,
      gridColumn: 'full',
    }),
  ],

  // Auth modes - SQL auth (default), Service Principal, Enterprise OAuth
  // NOTE: Kyomi global Microsoft OAuth mode removed — re-enable when Microsoft approves scopes
  authModes: [
    {
      value: 'sql',
      label: 'SQL Authentication',
      description: 'Authenticate with username and password.',
      isDefault: true,
      supportsSharedCredentials: true,
      credentialFields: [
        usernameField(),
        passwordField(),
      ],
    },
    {
      value: 'service_principal',
      label: 'Service Principal',
      description: 'Authenticate using an Azure AD service principal.',
      supportsSharedCredentials: true,
      credentialFields: [
        // tenant_id is now in connectionFields (shared with OAuth mode)
        clientIdField(),
        clientSecretField(),
      ],
    },
    {
      ...enterpriseOAuthAuthMode('Microsoft'),
      value: 'enterprise_oauth',
      description: "Users authenticate with your organization's Azure AD app.",
      oauth: {
        provider: 'microsoft-enterprise',
        configFields: [oauthClientIdField(), oauthClientSecretField()],
      },
      callbackPath: '/auth/oauth/microsoft-enterprise/callback',
      credentialFields: [],
    },
  ],

  // Catalog configuration
  catalogConfig: {
    key: 'catalog_schemas',
    label: 'Schemas to Index',
    discoveryKey: 'schemas',
    helpText: 'Select which schemas to include in the catalog.',
  },

  sshTunnelSupported: false,
};
