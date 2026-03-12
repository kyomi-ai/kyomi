// SPDX-License-Identifier: AGPL-3.0-or-later
/**
 * Connection Form Schemas
 *
 * Declarative field definitions for datasource connection forms.
 * This eliminates ~300 lines of repetitive JSX by defining forms as data.
 *
 * Each schema defines the fields needed for a datasource type's connection config.
 * The ConnectionFormRenderer component uses these schemas to render forms dynamically.
 *
 * Schema Structure (Phase 3 - Universal Datasource Setup):
 * - connectionFields: Required for initial connect (always text/number inputs)
 * - discoveryFields: Populated after connect (become dropdowns from discovered resources)
 * - credentialFields: Authentication fields (username/password)
 * - catalogConfig: Configuration for what to index
 *
 * @deprecated fields - The old `fields` array is kept for backward compatibility
 *                      but new code should use connectionFields/discoveryFields
 */

/**
 * Field types supported by the form renderer
 * @type {Object}
 */
export const FIELD_TYPES = {
  TEXT: 'text',
  NUMBER: 'number',
  PASSWORD: 'password',
  SELECT: 'select',
  CHECKBOX: 'checkbox',
  DISCOVERY: 'discovery', // NEW: Populated from discovery endpoint
};

/**
 * Base field definitions that are common across multiple datasource types.
 * These can be imported and customized per datasource.
 */
const BASE_FIELDS = {
  host: {
    name: 'host',
    type: FIELD_TYPES.TEXT,
    label: 'Host',
    placeholder: 'db.example.com',
    required: true,
    gridColumn: 1,
  },
  port: {
    name: 'port',
    type: FIELD_TYPES.NUMBER,
    label: 'Port',
    gridColumn: 2,
  },
  database: {
    name: 'database',
    type: FIELD_TYPES.TEXT,
    label: 'Database',
    required: true,
    gridColumn: 1,
  },
};

/**
 * SSH Tunnel fields - reusable across any datasource that supports SSH tunneling
 */
export const SSH_TUNNEL_FIELDS = [
  {
    name: 'ssh_host',
    type: FIELD_TYPES.TEXT,
    label: 'SSH Host',
    placeholder: 'bastion.example.com',
    required: true,
    gridColumn: 1,
  },
  {
    name: 'ssh_port',
    type: FIELD_TYPES.NUMBER,
    label: 'SSH Port',
    defaultValue: 22,
    gridColumn: 2,
  },
  {
    name: 'ssh_username',
    type: FIELD_TYPES.TEXT,
    label: 'SSH Username',
    placeholder: 'ssh_user',
    required: true,
    gridColumn: 'full',
  },
];

/**
 * PostgreSQL connection form schema
 *
 * Discovery returns: { databases: [...], schemas: [...] }
 */
export const POSTGRES_SCHEMA = {
  type: 'postgres',

  // Step 1: Connection fields (text inputs, required for connect)
  connectionFields: [
    {
      ...BASE_FIELDS.host,
      placeholder: 'db.example.com',
    },
    {
      ...BASE_FIELDS.port,
      defaultValue: 5432,
    },
    {
      name: 'ssl_mode',
      type: FIELD_TYPES.SELECT,
      label: 'SSL Mode',
      defaultValue: 'require',
      gridColumn: 'full',
      options: [
        { value: 'disable', label: 'Disable' },
        { value: 'require', label: 'Require' },
        { value: 'verify-ca', label: 'Verify CA' },
        { value: 'verify-full', label: 'Verify Full' },
      ],
    },
  ],

  // Step 2: Discovery fields (become dropdowns after successful connect)
  discoveryFields: [
    {
      name: 'database',
      type: FIELD_TYPES.DISCOVERY,
      discoveryKey: 'databases',
      label: 'Database',
      placeholder: 'Select database...',
      required: true,
      gridColumn: 1,
    },
    {
      name: 'schema',
      type: FIELD_TYPES.DISCOVERY,
      discoveryKey: 'schemas',
      label: 'Default Schema',
      placeholder: 'Select schema...',
      defaultValue: 'public',
      optional: true,
      gridColumn: 2,
      helpText: 'Default schema for queries (usually "public")',
    },
  ],

  // Credential fields (username/password)
  credentialFields: [
    {
      name: 'username',
      type: FIELD_TYPES.TEXT,
      label: 'Username',
      placeholder: 'postgres',
      required: true,
      gridColumn: 1,
    },
    {
      name: 'password',
      type: FIELD_TYPES.PASSWORD,
      label: 'Password',
      placeholder: 'Enter password',
      required: true,
      gridColumn: 2,
    },
  ],

  // Catalog configuration
  catalogConfig: {
    key: 'catalog_schemas',
    label: 'Schemas to Index',
    discoveryKey: 'schemas',
    helpText: 'Select which schemas to include in the catalog. Leave empty to index all accessible schemas.',
  },

  sshTunnelSupported: true,

  // @deprecated - Use connectionFields/discoveryFields instead
  fields: [
    {
      ...BASE_FIELDS.host,
      placeholder: 'db.example.com',
    },
    {
      ...BASE_FIELDS.port,
      defaultValue: 5432,
    },
    {
      ...BASE_FIELDS.database,
      placeholder: 'mydb',
    },
    {
      name: 'ssl_mode',
      type: FIELD_TYPES.SELECT,
      label: 'SSL Mode',
      defaultValue: 'require',
      gridColumn: 2,
      options: [
        { value: 'disable', label: 'Disable' },
        { value: 'require', label: 'Require' },
        { value: 'verify-ca', label: 'Verify CA' },
        { value: 'verify-full', label: 'Verify Full' },
      ],
    },
  ],
};

/**
 * ClickHouse connection form schema
 *
 * Discovery returns: { databases: [...] }
 */
export const CLICKHOUSE_SCHEMA = {
  type: 'clickhouse',

  // Step 1: Connection fields (text inputs, required for connect)
  connectionFields: [
    {
      ...BASE_FIELDS.host,
      placeholder: 'clickhouse.example.com',
    },
    {
      ...BASE_FIELDS.port,
      defaultValue: 8123,
    },
    {
      name: 'secure',
      type: FIELD_TYPES.CHECKBOX,
      label: 'Secure (HTTPS)',
      description: 'Use HTTPS connection',
      defaultValue: false,
      gridColumn: 'full',
    },
  ],

  // Step 2: Discovery fields (become dropdowns after successful connect)
  discoveryFields: [
    {
      name: 'database',
      type: FIELD_TYPES.DISCOVERY,
      discoveryKey: 'databases',
      label: 'Default Database',
      placeholder: 'Select database...',
      required: true,
      gridColumn: 'full',
      helpText: 'Default database for queries',
    },
  ],

  // Credential fields (username/password)
  credentialFields: [
    {
      name: 'username',
      type: FIELD_TYPES.TEXT,
      label: 'Username',
      placeholder: 'default',
      required: true,
      gridColumn: 1,
    },
    {
      name: 'password',
      type: FIELD_TYPES.PASSWORD,
      label: 'Password',
      placeholder: 'Enter password',
      required: true,
      gridColumn: 2,
    },
  ],

  // Catalog configuration
  catalogConfig: {
    key: 'catalog_databases',
    label: 'Databases to Index',
    discoveryKey: 'databases',
    helpText: 'Select which databases to include in the catalog. Leave empty to index all accessible databases.',
  },

  sshTunnelSupported: true,

  // @deprecated - Use connectionFields/discoveryFields instead
  fields: [
    {
      ...BASE_FIELDS.host,
      placeholder: 'clickhouse.example.com',
    },
    {
      ...BASE_FIELDS.port,
      defaultValue: 8123,
    },
    {
      ...BASE_FIELDS.database,
      placeholder: 'default',
    },
    {
      name: 'secure',
      type: FIELD_TYPES.CHECKBOX,
      label: 'Secure (HTTPS)',
      description: 'Use HTTPS connection',
      defaultValue: false,
      gridColumn: 2,
    },
  ],
};

/**
 * Snowflake connection form schema
 *
 * Discovery returns: { warehouses: [...], databases: [...], schemas: [...] }
 */
export const SNOWFLAKE_SCHEMA = {
  type: 'snowflake',

  // Step 1: Connection fields (text inputs, required for connect)
  connectionFields: [
    {
      name: 'account',
      type: FIELD_TYPES.TEXT,
      label: 'Account',
      placeholder: 'xy12345.us-east-1 or myorg-myaccount',
      required: true,
      gridColumn: 'full',
      helpText: 'Your Snowflake account identifier (found in your Snowflake URL)',
    },
  ],

  // Step 2: Discovery fields (become dropdowns after successful connect)
  discoveryFields: [
    {
      name: 'warehouse',
      type: FIELD_TYPES.DISCOVERY,
      discoveryKey: 'warehouses',
      label: 'Warehouse',
      placeholder: 'Select warehouse...',
      required: true,
      gridColumn: 1,
      helpText: 'Default warehouse for compute',
    },
    {
      name: 'database',
      type: FIELD_TYPES.DISCOVERY,
      discoveryKey: 'databases',
      label: 'Default Database',
      placeholder: 'Select database...',
      optional: true,
      gridColumn: 2,
    },
    {
      name: 'schema',
      type: FIELD_TYPES.DISCOVERY,
      discoveryKey: 'schemas',
      label: 'Default Schema',
      placeholder: 'Select schema...',
      defaultValue: 'PUBLIC',
      optional: true,
      gridColumn: 1,
      helpText: 'Default schema for queries (usually "PUBLIC")',
    },
    {
      name: 'role',
      type: FIELD_TYPES.TEXT, // Role stays as text (optional)
      label: 'Role',
      placeholder: 'ACCOUNTADMIN',
      optional: true,
      gridColumn: 2,
      helpText: 'Snowflake role to use (leave empty for default)',
    },
  ],

  // Credential fields (username/password) - used when OAuth is not configured
  credentialFields: [
    {
      name: 'username',
      type: FIELD_TYPES.TEXT,
      label: 'Username',
      placeholder: 'your_username',
      required: true,
      gridColumn: 1,
    },
    {
      name: 'password',
      type: FIELD_TYPES.PASSWORD,
      label: 'Password',
      placeholder: 'Enter password',
      required: true,
      gridColumn: 2,
    },
  ],

  // OAuth configuration fields (admin-only, stored in connection_config)
  // When these are configured, users can authenticate via OAuth instead of password
  oauthConfigFields: [
    {
      name: 'oauth_client_id',
      type: FIELD_TYPES.TEXT,
      label: 'OAuth Client ID',
      placeholder: 'From your Snowflake OAuth integration',
      gridColumn: 1,
      helpText: 'Client ID from your Snowflake OAuth security integration',
    },
    {
      name: 'oauth_client_secret',
      type: FIELD_TYPES.PASSWORD,
      label: 'OAuth Client Secret',
      placeholder: 'OAuth client secret',
      gridColumn: 2,
      helpText: 'Client secret from your Snowflake OAuth security integration',
    },
  ],

  // Catalog configuration
  catalogConfig: {
    key: 'catalog_databases',
    label: 'Databases to Index',
    discoveryKey: 'databases',
    helpText: 'Select which databases to include in the catalog. Leave empty to index all accessible databases.',
  },

  // Snowflake supports OAuth (per-datasource, not global like BigQuery)
  supportsOAuth: true,
  oauthProvider: 'snowflake',
  oauthMessage: 'Connect with your Snowflake account using OAuth. Requires OAuth integration configured by your Snowflake admin.',

  sshTunnelSupported: false,

  // @deprecated - Use connectionFields/discoveryFields instead
  fields: [
    {
      name: 'account',
      type: FIELD_TYPES.TEXT,
      label: 'Account',
      placeholder: 'xy12345.us-east-1 or myorg-myaccount',
      required: true,
      gridColumn: 'full',
      helpText: 'Your Snowflake account identifier (found in your Snowflake URL)',
    },
    {
      name: 'warehouse',
      type: FIELD_TYPES.TEXT,
      label: 'Warehouse',
      placeholder: 'COMPUTE_WH',
      gridColumn: 1,
      helpText: 'Default warehouse for compute',
    },
    {
      ...BASE_FIELDS.database,
      placeholder: 'MY_DATABASE',
      gridColumn: 2,
      required: false,
    },
    {
      name: 'schema',
      type: FIELD_TYPES.TEXT,
      label: 'Schema',
      placeholder: 'PUBLIC',
      defaultValue: 'PUBLIC',
      gridColumn: 1,
    },
    {
      name: 'role',
      type: FIELD_TYPES.TEXT,
      label: 'Role',
      placeholder: 'ACCOUNTADMIN',
      gridColumn: 2,
      helpText: 'Snowflake role to use (leave empty for default)',
    },
  ],
};

/**
 * MySQL connection form schema
 *
 * SSL mode values match backend MySQLProvider expectations:
 * - disable: No SSL
 * - require: SSL required (default)
 * - verify-ca: Verify server certificate against CA
 * - verify-full: Verify server certificate and hostname
 *
 * Discovery returns: { databases: [...] }
 */
export const MYSQL_SCHEMA = {
  type: 'mysql',

  // Step 1: Connection fields (text inputs, required for connect)
  connectionFields: [
    {
      ...BASE_FIELDS.host,
      placeholder: 'mysql.example.com',
    },
    {
      ...BASE_FIELDS.port,
      defaultValue: 3306,
    },
    {
      name: 'ssl_mode',
      type: FIELD_TYPES.SELECT,
      label: 'SSL Mode',
      defaultValue: 'require',
      gridColumn: 'full',
      options: [
        { value: 'disable', label: 'Disable' },
        { value: 'require', label: 'Require (Recommended)' },
        { value: 'verify-ca', label: 'Verify CA' },
        { value: 'verify-full', label: 'Verify Full' },
      ],
    },
  ],

  // Step 2: Discovery fields (become dropdowns after successful connect)
  discoveryFields: [
    {
      name: 'database',
      type: FIELD_TYPES.DISCOVERY,
      discoveryKey: 'databases',
      label: 'Default Database',
      placeholder: 'Select database...',
      required: true,
      gridColumn: 'full',
      helpText: 'Default database for queries',
    },
  ],

  // Credential fields (username/password)
  credentialFields: [
    {
      name: 'username',
      type: FIELD_TYPES.TEXT,
      label: 'Username',
      placeholder: 'root',
      required: true,
      gridColumn: 1,
    },
    {
      name: 'password',
      type: FIELD_TYPES.PASSWORD,
      label: 'Password',
      placeholder: 'Enter password',
      required: true,
      gridColumn: 2,
    },
  ],

  // Catalog configuration
  catalogConfig: {
    key: 'catalog_databases',
    label: 'Databases to Index',
    discoveryKey: 'databases',
    helpText: 'Select which databases to include in the catalog. Leave empty to index all accessible databases.',
  },

  sshTunnelSupported: true,

  // @deprecated - Use connectionFields/discoveryFields instead
  fields: [
    {
      ...BASE_FIELDS.host,
      placeholder: 'mysql.example.com',
    },
    {
      ...BASE_FIELDS.port,
      defaultValue: 3306,
    },
    {
      ...BASE_FIELDS.database,
      placeholder: 'mydb',
    },
    {
      name: 'ssl_mode',
      type: FIELD_TYPES.SELECT,
      label: 'SSL Mode',
      defaultValue: 'require',
      gridColumn: 2,
      options: [
        { value: 'disable', label: 'Disable' },
        { value: 'require', label: 'Require (Recommended)' },
        { value: 'verify-ca', label: 'Verify CA' },
        { value: 'verify-full', label: 'Verify Full' },
      ],
    },
  ],
};

/**
 * Databricks connection form schema
 *
 * Discovery returns: { catalogs: [...], schemas: [...] }
 */
export const DATABRICKS_SCHEMA = {
  type: 'databricks',

  // Step 1: Connection fields (text inputs, required for connect)
  connectionFields: [
    {
      name: 'server_hostname',
      type: FIELD_TYPES.TEXT,
      label: 'Server Hostname',
      placeholder: 'dbc-xxxxxxxx-xxxx.cloud.databricks.com',
      required: true,
      gridColumn: 'full',
    },
    {
      name: 'http_path',
      type: FIELD_TYPES.TEXT,
      label: 'HTTP Path',
      placeholder: '/sql/1.0/warehouses/xxxx',
      required: true,
      gridColumn: 'full',
    },
  ],

  // Step 2: Discovery fields (become dropdowns after successful connect)
  discoveryFields: [
    {
      name: 'catalog',
      type: FIELD_TYPES.DISCOVERY,
      discoveryKey: 'catalogs',
      label: 'Catalog',
      placeholder: 'Select catalog...',
      defaultValue: 'hive_metastore',
      gridColumn: 1,
      helpText: 'Unity Catalog or hive_metastore',
    },
    {
      name: 'schema',
      type: FIELD_TYPES.DISCOVERY,
      discoveryKey: 'schemas',
      label: 'Default Schema',
      placeholder: 'Select schema...',
      defaultValue: 'default',
      optional: true,
      gridColumn: 2,
    },
  ],

  // Credential fields (token-based authentication)
  credentialFields: [
    {
      name: 'access_token',
      type: FIELD_TYPES.PASSWORD,
      label: 'Personal Access Token',
      placeholder: 'dapi...',
      required: true,
      gridColumn: 'full',
      helpText: 'Databricks personal access token',
    },
  ],

  // Catalog configuration
  catalogConfig: {
    key: 'catalog_schemas',
    label: 'Schemas to Index',
    discoveryKey: 'schemas',
    helpText: 'Select which schemas to include in the catalog. Leave empty to index all accessible schemas.',
  },

  sshTunnelSupported: false,

  // @deprecated - Use connectionFields/discoveryFields instead
  fields: [
    {
      name: 'server_hostname',
      type: FIELD_TYPES.TEXT,
      label: 'Server Hostname',
      placeholder: 'dbc-xxxxxxxx-xxxx.cloud.databricks.com',
      required: true,
      gridColumn: 'full',
    },
    {
      name: 'http_path',
      type: FIELD_TYPES.TEXT,
      label: 'HTTP Path',
      placeholder: '/sql/1.0/warehouses/xxxx',
      required: true,
      gridColumn: 'full',
    },
    {
      name: 'catalog',
      type: FIELD_TYPES.TEXT,
      label: 'Catalog',
      placeholder: 'hive_metastore',
      defaultValue: 'hive_metastore',
      gridColumn: 1,
    },
    {
      name: 'schema',
      type: FIELD_TYPES.TEXT,
      label: 'Schema',
      placeholder: 'default',
      defaultValue: 'default',
      gridColumn: 2,
    },
  ],
};

/**
 * SQL Server connection form schema
 *
 * Discovery returns: { databases: [...], schemas: [...] }
 */
export const SQLSERVER_SCHEMA = {
  type: 'sqlserver',

  // Step 1: Connection fields (text inputs, required for connect)
  connectionFields: [
    {
      ...BASE_FIELDS.host,
      placeholder: 'sqlserver.example.com',
    },
    {
      ...BASE_FIELDS.port,
      defaultValue: 1433,
    },
    {
      name: 'encrypt',
      type: FIELD_TYPES.CHECKBOX,
      label: 'Encrypt Connection',
      description: 'Use TLS encryption',
      defaultValue: true,
      gridColumn: 1,
    },
    {
      name: 'trust_server_certificate',
      type: FIELD_TYPES.CHECKBOX,
      label: 'Trust Server Certificate',
      description: 'Trust self-signed certificates',
      defaultValue: false,
      gridColumn: 2,
    },
  ],

  // Step 2: Discovery fields (become dropdowns after successful connect)
  discoveryFields: [
    {
      name: 'database',
      type: FIELD_TYPES.DISCOVERY,
      discoveryKey: 'databases',
      label: 'Database',
      placeholder: 'Select database...',
      required: true,
      gridColumn: 1,
    },
    {
      name: 'schema',
      type: FIELD_TYPES.DISCOVERY,
      discoveryKey: 'schemas',
      label: 'Default Schema',
      placeholder: 'Select schema...',
      defaultValue: 'dbo',
      optional: true,
      gridColumn: 2,
      helpText: 'Default schema for queries (usually "dbo")',
    },
  ],

  // Credential fields (username/password)
  credentialFields: [
    {
      name: 'username',
      type: FIELD_TYPES.TEXT,
      label: 'Username',
      placeholder: 'sa',
      required: true,
      gridColumn: 1,
    },
    {
      name: 'password',
      type: FIELD_TYPES.PASSWORD,
      label: 'Password',
      placeholder: 'Enter password',
      required: true,
      gridColumn: 2,
    },
  ],

  // Catalog configuration
  catalogConfig: {
    key: 'catalog_schemas',
    label: 'Schemas to Index',
    discoveryKey: 'schemas',
    helpText: 'Select which schemas to include in the catalog. Leave empty to index all accessible schemas.',
  },

  sshTunnelSupported: true,

  // @deprecated - Use connectionFields/discoveryFields instead
  fields: [
    {
      ...BASE_FIELDS.host,
      placeholder: 'sqlserver.example.com',
    },
    {
      ...BASE_FIELDS.port,
      defaultValue: 1433,
    },
    {
      ...BASE_FIELDS.database,
      placeholder: 'master',
    },
    {
      name: 'schema',
      type: FIELD_TYPES.TEXT,
      label: 'Schema',
      placeholder: 'dbo',
      defaultValue: 'dbo',
      gridColumn: 2,
    },
  ],
};

/**
 * BigQuery schema - special case, uses OAuth
 *
 * Discovery returns: { projects: [...] }
 */
export const BIGQUERY_SCHEMA = {
  type: 'bigquery',

  // No connection fields - uses OAuth
  connectionFields: [],

  // No discovery fields - billing/default project are handled in credentials section
  discoveryFields: [],

  // No credential fields - uses OAuth (billing/default project handled in custom UI)
  credentialFields: [],

  // Catalog configuration
  catalogConfig: {
    key: 'catalog_projects',
    label: 'Projects to Index',
    discoveryKey: 'projects',
    helpText: 'Select which projects to include in the catalog. Leave empty to index all accessible projects.',
  },

  usesOAuth: true,
  oauthMessage: 'BigQuery uses OAuth for authentication. Connect your Google account below.',
  sshTunnelSupported: false,

  // @deprecated - Use connectionFields/discoveryFields instead
  fields: [],
};

/**
 * Generic fallback schema for unknown types
 */
export const GENERIC_SCHEMA = {
  type: 'generic',

  // Connection fields
  connectionFields: [
    {
      ...BASE_FIELDS.host,
      placeholder: 'host.example.com',
      gridColumn: 'full',
    },
    {
      ...BASE_FIELDS.port,
      gridColumn: 1,
    },
  ],

  // Discovery fields
  discoveryFields: [
    {
      name: 'database',
      type: FIELD_TYPES.DISCOVERY,
      discoveryKey: 'databases',
      label: 'Database',
      placeholder: 'Select database...',
      required: true,
      gridColumn: 'full',
    },
  ],

  // Credential fields
  credentialFields: [
    {
      name: 'username',
      type: FIELD_TYPES.TEXT,
      label: 'Username',
      placeholder: 'username',
      required: true,
      gridColumn: 1,
    },
    {
      name: 'password',
      type: FIELD_TYPES.PASSWORD,
      label: 'Password',
      placeholder: 'Enter password',
      required: true,
      gridColumn: 2,
    },
  ],

  // Catalog configuration
  catalogConfig: {
    key: 'catalog_items',
    label: 'Items to Index',
    discoveryKey: 'databases',
    helpText: 'Select which items to include in the catalog.',
  },

  sshTunnelSupported: false,

  // @deprecated - Use connectionFields/discoveryFields instead
  fields: [
    {
      ...BASE_FIELDS.host,
      placeholder: 'host.example.com',
      gridColumn: 'full',
    },
    {
      ...BASE_FIELDS.database,
      placeholder: 'default',
      gridColumn: 'full',
    },
  ],
};

/**
 * Schema registry - maps datasource types to their schemas
 */
export const CONNECTION_SCHEMAS = {
  postgres: POSTGRES_SCHEMA,
  clickhouse: CLICKHOUSE_SCHEMA,
  snowflake: SNOWFLAKE_SCHEMA,
  mysql: MYSQL_SCHEMA,
  databricks: DATABRICKS_SCHEMA,
  sqlserver: SQLSERVER_SCHEMA,
  bigquery: BIGQUERY_SCHEMA,
};

/**
 * Get the schema for a datasource type
 * @param {string} type - Datasource type (e.g., 'postgres', 'clickhouse')
 * @returns {Object} Schema definition
 */
export function getConnectionSchema(type) {
  return CONNECTION_SCHEMAS[type] || GENERIC_SCHEMA;
}

/**
 * Get default values for a schema's fields
 * Supports both legacy `fields` array and new `connectionFields`/`discoveryFields` arrays
 *
 * @param {Object} schema - Schema definition
 * @returns {Object} Default values keyed by field name
 */
export function getSchemaDefaults(schema) {
  const defaults = {};

  // Helper to extract defaults from a field array
  const extractDefaults = (fields) => {
    if (!fields) return;
    for (const field of fields) {
      if (field.defaultValue !== undefined) {
        defaults[field.name] = field.defaultValue;
      }
    }
  };

  // Extract from new field arrays (Phase 3+)
  extractDefaults(schema.connectionFields);
  extractDefaults(schema.discoveryFields);
  extractDefaults(schema.credentialFields);

  // Also extract from legacy fields array for backward compatibility
  extractDefaults(schema.fields);

  return defaults;
}

/**
 * Get all fields from a schema (for backward compatibility)
 * Combines connectionFields, discoveryFields in order
 *
 * @param {Object} schema - Schema definition
 * @returns {Array} Combined array of all fields
 */
export function getAllFields(schema) {
  // If schema has the new field arrays, combine them
  if (schema.connectionFields || schema.discoveryFields) {
    return [
      ...(schema.connectionFields || []),
      ...(schema.discoveryFields || []),
    ];
  }

  // Fall back to legacy fields array
  return schema.fields || [];
}

/**
 * Check if a field should be rendered as a discovery dropdown
 *
 * @param {Object} field - Field definition
 * @returns {boolean} True if field is a discovery field
 */
export function isDiscoveryField(field) {
  return field.type === FIELD_TYPES.DISCOVERY;
}
