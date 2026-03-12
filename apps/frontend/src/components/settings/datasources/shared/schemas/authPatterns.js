// SPDX-License-Identifier: AGPL-3.0-or-later
// shared/schemas/authPatterns.js
import {
  usernameField,
  passwordField,
  privateKeyField,
  privateKeyPassphraseField,
  oauthClientIdField,
  oauthClientSecretField,
  accessTokenField,
  tenantIdField,
  clientIdField,
  clientSecretField,
} from './fields.js';

/**
 * Standard password authentication pattern.
 * Used by: PostgreSQL, MySQL, ClickHouse, Redshift, SQL Server, Snowflake (password mode)
 */
export const passwordAuthMode = (overrides = {}) => {
  const { credentialFields, ...authModeOverrides } = overrides;

  return {
    value: 'password',
    label: 'Password',
    description: 'Users authenticate with username and password.',
    isDefault: true,
    supportsSharedCredentials: true,
    credentialFields: credentialFields || [
      usernameField(),
      passwordField(),
    ],
    ...authModeOverrides,
  };
};

/**
 * OAuth authentication pattern (per-datasource OAuth).
 * Used by: Snowflake
 */
export const oauthAuthMode = (provider, overrides = {}) => ({
  value: 'oauth',
  label: 'OAuth',
  description: `Users authenticate with their ${provider} account via OAuth.`,
  oauth: {
    provider,
    configFields: [oauthClientIdField(), oauthClientSecretField()],
  },
  supportsSharedCredentials: false,
  ...overrides,
});

/**
 * Key-pair authentication pattern.
 * Used by: Snowflake
 */
export const keypairAuthMode = (overrides = {}) => {
  const { credentialFields, ...authModeOverrides } = overrides;

  return {
    value: 'keypair',
    label: 'Key-Pair',
    description: 'Users authenticate using RSA key-pair.',
    supportsSharedCredentials: true,
    credentialFields: credentialFields || [
      usernameField(),
      privateKeyField(),
      privateKeyPassphraseField(),
    ],
    ...authModeOverrides,
  };
};

/**
 * Global OAuth pattern (uses app's OAuth, not per-datasource).
 * Used by: BigQuery (Kyomi OAuth mode)
 */
export const globalOAuthAuthMode = (provider, overrides = {}) => ({
  value: `${provider}_oauth`,
  label: `${provider} OAuth`,
  description: `Users authenticate with their ${provider} accounts via Kyomi.`,
  oauth: {
    provider,
    global: true, // Uses app's OAuth, not configured per-datasource
  },
  supportsSharedCredentials: false,
  requiresBeta: true, // Requires beta access to use Kyomi's OAuth
  ...overrides,
});

/**
 * Enterprise OAuth pattern (customer provides OAuth credentials).
 * Used by: BigQuery (Enterprise OAuth mode)
 */
export const enterpriseOAuthAuthMode = (provider, overrides = {}) => ({
  value: 'enterprise_oauth',
  label: 'Enterprise OAuth',
  description: "Users authenticate with your organization's OAuth app.",
  oauth: {
    provider: `${provider}-enterprise`,
    configFields: [oauthClientIdField(), oauthClientSecretField()],
  },
  supportsSharedCredentials: false,
  ...overrides,
});

/**
 * Service account pattern.
 * Used by: BigQuery
 */
export const serviceAccountAuthMode = (overrides = {}) => ({
  value: 'service_account',
  label: 'Service Account',
  description: 'All users share a service account for automated access.',
  serviceAccount: {
    uploadField: 'service_account_json',
    emailField: 'service_account_email',
  },
  supportsSharedCredentials: true, // Everyone uses the same credentials
  ...overrides,
});

/**
 * Token/API key authentication pattern.
 * Used by: Databricks
 */
export const tokenAuthMode = (overrides = {}) => {
  const { credentialFields, ...authModeOverrides } = overrides;

  return {
    value: 'token',
    label: 'Personal Access Token',
    description: 'Users authenticate with a personal access token.',
    supportsSharedCredentials: true,
    credentialFields: credentialFields || [
      accessTokenField(),
    ],
    ...authModeOverrides,
  };
};

/**
 * Azure Service Principal authentication pattern.
 * Used by: Azure Synapse, Azure SQL
 */
export const servicePrincipalAuthMode = (overrides = {}) => {
  const { credentialFields, ...authModeOverrides } = overrides;

  return {
    value: 'service_principal',
    label: 'Service Principal',
    description: 'Authenticate using an Azure AD service principal.',
    supportsSharedCredentials: true,
    credentialFields: credentialFields || [
      tenantIdField(),
      clientIdField(),
      clientSecretField(),
    ],
    ...authModeOverrides,
  };
};

/**
 * Azure Managed Identity authentication pattern.
 * Used by: Azure Synapse, Azure SQL (when running in Azure)
 */
export const managedIdentityAuthMode = (overrides = {}) => ({
  value: 'managed_identity',
  label: 'Managed Identity',
  description: 'Authenticate using Azure Managed Identity (requires Azure hosting).',
  supportsSharedCredentials: false,
  credentialFields: [], // No credentials needed - Azure handles it
  helpText: 'Only available when Kyomi is hosted in Azure with Managed Identity enabled.',
  ...overrides,
});
