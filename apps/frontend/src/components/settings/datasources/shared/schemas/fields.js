// SPDX-License-Identifier: AGPL-3.0-or-later
// shared/schemas/fields.js

export const FIELD_TYPES = {
  TEXT: 'text',
  NUMBER: 'number',
  PASSWORD: 'password',
  TEXTAREA: 'textarea',
  SELECT: 'select',
  CHECKBOX: 'checkbox',
  DISCOVERY: 'discovery',
};

// === CONNECTION FIELDS ===

export const hostField = (overrides = {}) => ({
  name: 'host',
  type: FIELD_TYPES.TEXT,
  label: 'Host',
  placeholder: 'db.example.com',
  required: true,
  gridColumn: 1,
  ...overrides,
});

export const portField = (defaultValue, overrides = {}) => ({
  name: 'port',
  type: FIELD_TYPES.NUMBER,
  label: 'Port',
  defaultValue,
  gridColumn: 2,
  ...overrides,
});

export const databaseField = (overrides = {}) => ({
  name: 'database',
  type: FIELD_TYPES.TEXT,
  label: 'Database',
  required: true,
  gridColumn: 1,
  ...overrides,
});

export const sslModeField = (options = null) => ({
  name: 'ssl_mode',
  type: FIELD_TYPES.SELECT,
  label: 'SSL Mode',
  defaultValue: 'require',
  gridColumn: 'full',
  options: options || [
    { value: 'disable', label: 'Disable' },
    { value: 'require', label: 'Require' },
    { value: 'verify-ca', label: 'Verify CA' },
    { value: 'verify-full', label: 'Verify Full' },
  ],
});

// === CREDENTIAL FIELDS ===

export const usernameField = (overrides = {}) => ({
  name: 'username',
  type: FIELD_TYPES.TEXT,
  label: 'Username',
  placeholder: 'username',
  required: true,
  gridColumn: 1,
  ...overrides,
});

export const passwordField = (overrides = {}) => ({
  name: 'password',
  type: FIELD_TYPES.PASSWORD,
  label: 'Password',
  placeholder: 'Enter password',
  required: true,
  gridColumn: 2,
  ...overrides,
});

export const privateKeyField = (overrides = {}) => ({
  name: 'private_key',
  type: FIELD_TYPES.TEXTAREA,
  label: 'Private Key (PEM format)',
  placeholder: '-----BEGIN PRIVATE KEY-----\n...\n-----END PRIVATE KEY-----',
  required: true,
  rows: 6,
  gridColumn: 'full',
  ...overrides,
});

export const privateKeyPassphraseField = (overrides = {}) => ({
  name: 'private_key_passphrase',
  type: FIELD_TYPES.PASSWORD,
  label: 'Private Key Passphrase',
  placeholder: 'Optional',
  required: false,
  helpText: 'Only required if your private key is encrypted',
  gridColumn: 'full',
  ...overrides,
});

// === OAUTH CONFIG FIELDS ===

export const oauthClientIdField = (overrides = {}) => ({
  name: 'oauth_client_id',
  type: FIELD_TYPES.TEXT,
  label: 'OAuth Client ID',
  placeholder: 'OAuth client ID',
  gridColumn: 1,
  ...overrides,
});

export const oauthClientSecretField = (overrides = {}) => ({
  name: 'oauth_client_secret',
  type: FIELD_TYPES.PASSWORD,
  label: 'OAuth Client Secret',
  placeholder: 'OAuth client secret',
  gridColumn: 2,
  ...overrides,
});

// === DISCOVERY FIELDS ===

export const discoveryField = (name, discoveryKey, label, overrides = {}) => ({
  name,
  type: FIELD_TYPES.DISCOVERY,
  discoveryKey,
  label,
  placeholder: `Select ${label.toLowerCase()}...`,
  ...overrides,
});

// === SPECIAL FIELDS ===

export const billingProjectField = (overrides = {}) => ({
  name: 'billing_project',
  type: FIELD_TYPES.DISCOVERY,
  discoveryKey: 'projects',
  label: 'Billing Project',
  placeholder: 'Select billing project...',
  gridColumn: 1,
  ...overrides,
});

export const defaultProjectField = (overrides = {}) => ({
  name: 'default_project',
  type: FIELD_TYPES.DISCOVERY,
  discoveryKey: 'projects',
  label: 'Default Project',
  placeholder: 'Select default project...',
  gridColumn: 2,
  ...overrides,
});

// === PROVIDER-SPECIFIC CONNECTION FIELDS ===

/**
 * Snowflake account identifier field.
 * Format: xy12345.us-east-1 or myorg-myaccount
 */
export const accountField = (overrides = {}) => ({
  name: 'account',
  type: FIELD_TYPES.TEXT,
  label: 'Account',
  placeholder: 'xy12345.us-east-1 or myorg-myaccount',
  required: true,
  gridColumn: 'full',
  helpText: 'Your Snowflake account identifier (found in your Snowflake URL)',
  ...overrides,
});

/**
 * Databricks server hostname field.
 */
export const serverHostnameField = (overrides = {}) => ({
  name: 'server_hostname',
  type: FIELD_TYPES.TEXT,
  label: 'Server Hostname',
  placeholder: 'adb-123456789.12.azuredatabricks.net',
  required: true,
  gridColumn: 'full',
  helpText: 'Your Databricks workspace URL without https://',
  ...overrides,
});

/**
 * Databricks HTTP path field.
 */
export const httpPathField = (overrides = {}) => ({
  name: 'http_path',
  type: FIELD_TYPES.TEXT,
  label: 'HTTP Path',
  placeholder: '/sql/1.0/warehouses/abc123',
  required: true,
  gridColumn: 'full',
  helpText: 'SQL warehouse HTTP path from connection details',
  ...overrides,
});

/**
 * Snowflake role field.
 */
export const roleField = (overrides = {}) => ({
  name: 'role',
  type: FIELD_TYPES.TEXT,
  label: 'Role',
  placeholder: 'ACCOUNTADMIN',
  required: false,
  optional: true,
  gridColumn: 2,
  helpText: 'Snowflake role to use (leave empty for default)',
  ...overrides,
});

/**
 * ClickHouse HTTPS/secure mode checkbox.
 */
export const secureField = (overrides = {}) => ({
  name: 'secure',
  type: FIELD_TYPES.CHECKBOX,
  label: 'Use HTTPS',
  defaultValue: true,
  gridColumn: 1,
  ...overrides,
});

/**
 * SQL Server encrypt connection checkbox.
 */
export const encryptField = (overrides = {}) => ({
  name: 'encrypt',
  type: FIELD_TYPES.CHECKBOX,
  label: 'Encrypt Connection',
  defaultValue: true,
  gridColumn: 1,
  ...overrides,
});

/**
 * SQL Server trust server certificate checkbox.
 */
export const trustServerCertificateField = (overrides = {}) => ({
  name: 'trust_server_certificate',
  type: FIELD_TYPES.CHECKBOX,
  label: 'Trust Server Certificate',
  defaultValue: false,
  gridColumn: 2,
  helpText: 'Skip certificate validation (not recommended for production)',
  ...overrides,
});

/**
 * Personal access token field (Databricks).
 */
export const accessTokenField = (overrides = {}) => ({
  name: 'access_token',
  type: FIELD_TYPES.PASSWORD,
  label: 'Personal Access Token',
  placeholder: 'dapi...',
  required: true,
  gridColumn: 'full',
  helpText: 'Generate from your account settings',
  ...overrides,
});

// === AZURE SERVICE PRINCIPAL FIELDS ===

/**
 * Azure tenant ID field.
 */
export const tenantIdField = (overrides = {}) => ({
  name: 'tenant_id',
  type: FIELD_TYPES.TEXT,
  label: 'Tenant ID',
  placeholder: 'xxxxxxxx-xxxx-xxxx-xxxx-xxxxxxxxxxxx',
  required: true,
  gridColumn: 'full',
  helpText: 'Azure AD tenant ID (Directory ID)',
  ...overrides,
});

/**
 * Azure client ID field (Service Principal App ID).
 */
export const clientIdField = (overrides = {}) => ({
  name: 'client_id',
  type: FIELD_TYPES.TEXT,
  label: 'Client ID',
  placeholder: 'xxxxxxxx-xxxx-xxxx-xxxx-xxxxxxxxxxxx',
  required: true,
  gridColumn: 1,
  helpText: 'Service Principal Application ID',
  ...overrides,
});

/**
 * Azure client secret field.
 */
export const clientSecretField = (overrides = {}) => ({
  name: 'client_secret',
  type: FIELD_TYPES.PASSWORD,
  label: 'Client Secret',
  placeholder: 'Enter client secret',
  required: true,
  gridColumn: 2,
  helpText: 'Service Principal secret',
  ...overrides,
});

// === CONVENIENCE DISCOVERY FIELD FACTORIES ===

/**
 * Snowflake warehouse discovery field.
 */
export const warehouseField = (overrides = {}) =>
  discoveryField('warehouse', 'warehouses', 'Warehouse', {
    required: true,
    helpText: 'Default warehouse for compute',
    gridColumn: 1,
    ...overrides,
  });

/**
 * Databricks catalog discovery field.
 */
export const catalogField = (overrides = {}) =>
  discoveryField('catalog', 'catalogs', 'Catalog', {
    defaultValue: 'hive_metastore',
    gridColumn: 1,
    ...overrides,
  });

/**
 * Schema discovery field with configurable default.
 * @param {string} defaultValue - Default schema name (e.g., 'public', 'PUBLIC', 'dbo')
 */
export const schemaField = (defaultValue = 'public', overrides = {}) =>
  discoveryField('schema', 'schemas', 'Schema', {
    defaultValue,
    gridColumn: 2,
    ...overrides,
  });
