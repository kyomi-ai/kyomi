// SPDX-License-Identifier: AGPL-3.0-or-later
/**
 * ConnectionFormRenderer - Generic schema-driven connection form
 *
 * Renders connection forms dynamically based on schema definitions.
 * Eliminates ~300 lines of repetitive JSX by using declarative schemas.
 *
 * Features:
 * - Supports text, number, password, select, checkbox, and discovery fields
 * - Discovery fields become dropdowns populated from discovered resources
 * - Read-only mode for non-admin users
 * - Two-column grid layout with full-width option
 * - SSH tunnel configuration section
 * - Required field indicators
 */

import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from '../ui/select';
import { FIELD_TYPES, SSH_TUNNEL_FIELDS, getConnectionSchema } from './connectionFormSchemas';

/**
 * Render a single form field
 *
 * @param {Object} props
 * @param {Object} props.field - Field definition from schema
 * @param {*} props.value - Current field value
 * @param {Function} props.onChange - Change handler (fieldName, value)
 * @param {boolean} props.readOnly - Whether field is read-only
 * @param {boolean} props.showRequired - Whether to show required indicator
 * @param {Object} props.discoveredResources - Resources discovered from backend (optional)
 * @param {string} props.discoveryStatus - Status of discovery: 'idle', 'loading', 'success', 'error'
 */
function FormField({
  field,
  value,
  onChange,
  readOnly,
  showRequired,
  discoveredResources = {},
  discoveryStatus = 'idle',
}) {
  const currentValue = value ?? field.defaultValue ?? '';

  // Read-only display
  if (readOnly) {
    let displayValue = currentValue;

    if (field.type === FIELD_TYPES.CHECKBOX) {
      displayValue = currentValue ? 'Yes' : 'No';
    } else if (field.type === FIELD_TYPES.SELECT || field.type === FIELD_TYPES.DISCOVERY) {
      // For select/discovery fields, just show the value
      const option = field.options?.find(o => o.value === currentValue);
      displayValue = option?.label || currentValue;
    }

    return (
      <div>
        <label className="block text-sm font-medium mb-1">{field.label}</label>
        <p className="text-sm text-muted-foreground py-2">{displayValue || '—'}</p>
      </div>
    );
  }

  // Editable fields
  const inputClasses = "w-full px-3 py-2 border border-input rounded-md bg-background text-sm";

  switch (field.type) {
    case FIELD_TYPES.TEXT:
    case FIELD_TYPES.PASSWORD:
      return (
        <div>
          <label className="block text-sm font-medium mb-1">
            {field.label}
            {showRequired && field.required && <span className="text-error-foreground"> *</span>}
          </label>
          <input
            type={field.type === FIELD_TYPES.PASSWORD ? 'password' : 'text'}
            value={currentValue}
            onChange={(e) => onChange(field.name, e.target.value)}
            placeholder={field.placeholder}
            className={inputClasses}
          />
          {field.helpText && (
            <p className="text-xs text-muted-foreground mt-1">{field.helpText}</p>
          )}
        </div>
      );

    case FIELD_TYPES.NUMBER:
      return (
        <div>
          <label className="block text-sm font-medium mb-1">
            {field.label}
            {showRequired && field.required && <span className="text-error-foreground"> *</span>}
          </label>
          <input
            type="number"
            value={currentValue}
            onChange={(e) => {
              // Handle empty input explicitly - fall back to default value
              const val = e.target.value === ''
                ? (field.defaultValue ?? 0)
                : (parseInt(e.target.value, 10) || field.defaultValue || 0);
              onChange(field.name, val);
            }}
            className={inputClasses}
          />
        </div>
      );

    case FIELD_TYPES.SELECT:
      return (
        <div>
          <label className="block text-sm font-medium mb-1">
            {field.label}
            {showRequired && field.required && <span className="text-error-foreground"> *</span>}
          </label>
          <Select
            value={currentValue}
            onValueChange={(val) => onChange(field.name, val)}
          >
            <SelectTrigger><SelectValue /></SelectTrigger>
            <SelectContent>
              {field.options?.map(option => (
                <SelectItem key={option.value} value={option.value}>
                  {option.label}
                </SelectItem>
              ))}
            </SelectContent>
          </Select>
        </div>
      );

    case FIELD_TYPES.CHECKBOX:
      return (
        <div>
          <label className="block text-sm font-medium mb-1">{field.label}</label>
          <label className="flex items-center gap-2 py-2">
            <input
              type="checkbox"
              checked={currentValue || false}
              onChange={(e) => onChange(field.name, e.target.checked)}
              className="h-4 w-4 rounded border-input"
            />
            <span className="text-sm text-muted-foreground">{field.description}</span>
          </label>
        </div>
      );

    case FIELD_TYPES.DISCOVERY: {
      // Discovery fields are dropdowns populated from discovered resources
      const discoveryKey = field.discoveryKey; // e.g., 'databases', 'schemas', 'warehouses'
      const options = discoveredResources[discoveryKey] || [];
      const hasOptions = options.length > 0;
      const isDiscoveryComplete = discoveryStatus === 'success';
      const isDiscoveryLoading = discoveryStatus === 'loading';

      // Determine the placeholder based on status
      let placeholder = field.placeholder || `Select ${field.label}...`;
      if (!isDiscoveryComplete) {
        placeholder = 'Run Test & Discover first';
      } else if (isDiscoveryLoading) {
        placeholder = 'Discovering...';
      } else if (!hasOptions) {
        placeholder = `No ${discoveryKey} found`;
      }

      return (
        <div>
          <label className="block text-sm font-medium mb-1">
            {field.label}
            {showRequired && field.required && !field.optional && (
              <span className="text-error-foreground"> *</span>
            )}
          </label>
          <Select
            value={currentValue}
            onValueChange={(val) => onChange(field.name, val)}
            disabled={!isDiscoveryComplete || isDiscoveryLoading || !hasOptions}
          >
            <SelectTrigger className={!isDiscoveryComplete ? 'opacity-60' : ''}>
              <SelectValue placeholder={placeholder} />
            </SelectTrigger>
            <SelectContent>
              {options.map(option => (
                <SelectItem key={option} value={option}>
                  {option}
                </SelectItem>
              ))}
            </SelectContent>
          </Select>
          {field.helpText && (
            <p className="text-xs text-muted-foreground mt-1">{field.helpText}</p>
          )}
        </div>
      );
    }

    default:
      return null;
  }
}

/**
 * Group fields into rows based on their gridColumn setting.
 * Returns an array of rows, where each row is an array of fields.
 *
 * State machine for 2-column grid layout:
 * - currentColumn = 0: Start of new row, no fields placed yet
 * - currentColumn = 1: Column 1 is filled, waiting for column 2 or new row
 * - currentColumn = 2: Both columns filled, row complete (triggers new row)
 *
 * gridColumn values:
 * - 'full': Field spans both columns, gets its own row
 * - 1: Field goes in column 1 (left)
 * - 2: Field goes in column 2 (right)
 */
function groupFieldsIntoRows(fields) {
  const rows = [];
  let currentRow = [];
  let currentColumn = 0; // 0 = start of row, 1 = col1 filled, 2 = row complete

  for (const field of fields) {
    const gridColumn = field.gridColumn || 1;

    if (gridColumn === 'full') {
      // Full-width field: flush current row, then add field on its own row
      if (currentRow.length > 0) {
        rows.push(currentRow);
        currentRow = [];
        currentColumn = 0;
      }
      rows.push([{ ...field, span: 2 }]);
    } else if (gridColumn === 1) {
      // Column 1 field: if row is complete, start new row
      if (currentColumn >= 2) {
        rows.push(currentRow);
        currentRow = [];
        currentColumn = 0;
      }
      currentRow.push({ ...field, span: 1 });
      currentColumn = 1;
    } else if (gridColumn === 2) {
      // Column 2 field: add placeholder if col1 is empty, then complete row
      if (currentColumn === 0) {
        currentRow.push(null); // Placeholder for empty column 1
      }
      currentRow.push({ ...field, span: 1 });
      rows.push(currentRow);
      currentRow = [];
      currentColumn = 0;
    }
  }

  // Push any remaining incomplete row
  if (currentRow.length > 0) {
    rows.push(currentRow);
  }

  return rows;
}

/**
 * SSH Tunnel Section Component
 */
function SSHTunnelSection({
  config,
  onChange,
  readOnly,
}) {
  const sshEnabled = config.ssh_enabled || false;

  const handleSSHToggle = (enabled) => {
    if (enabled) {
      onChange('ssh_enabled', true);
    } else {
      // Clear SSH config when disabling
      onChange('ssh_enabled', false);
      onChange('ssh_host', '');
      onChange('ssh_port', 22);
      onChange('ssh_username', '');
    }
  };

  if (readOnly) {
    if (!sshEnabled) return null;
    return (
      <div className="border-t border-border pt-4 mt-4">
        <h5 className="text-sm font-medium mb-3">SSH Tunnel</h5>
        <div className="grid grid-cols-2 gap-4">
          {SSH_TUNNEL_FIELDS.map(field => (
            <div key={field.name} className={field.gridColumn === 'full' ? 'col-span-2' : ''}>
              <FormField
                field={field}
                value={config[field.name]}
                onChange={() => {}}
                readOnly={true}
                showRequired={false}
              />
            </div>
          ))}
        </div>
      </div>
    );
  }

  return (
    <div className="border-t border-border pt-4 mt-4">
      <label className="flex items-center gap-3 p-3 bg-muted/30 rounded-lg cursor-pointer mb-4">
        <input
          type="checkbox"
          checked={sshEnabled}
          onChange={(e) => handleSSHToggle(e.target.checked)}
          className="h-4 w-4 rounded border-input"
        />
        <div>
          <p className="text-sm font-medium">Connect via SSH Tunnel</p>
          <p className="text-xs text-muted-foreground">
            Use a bastion host to reach the database behind a firewall
          </p>
        </div>
      </label>

      {sshEnabled && (
        <div className="space-y-4 pl-4 border-l-2 border-muted">
          <div className="grid grid-cols-2 gap-4">
            {SSH_TUNNEL_FIELDS.map(field => (
              <div key={field.name} className={field.gridColumn === 'full' ? 'col-span-2' : ''}>
                <FormField
                  field={field}
                  value={config[field.name]}
                  onChange={onChange}
                  readOnly={false}
                  showRequired={true}
                />
              </div>
            ))}
          </div>
          <div className="p-3 bg-muted/30 rounded-lg">
            <p className="text-xs text-muted-foreground">
              SSH keypair will be generated when you save. Add the public key to the bastion server's authorized_keys file.
            </p>
          </div>
        </div>
      )}
    </div>
  );
}

/**
 * Main ConnectionFormRenderer component
 *
 * @param {Object} props
 * @param {string} props.datasourceType - The datasource type (e.g., 'postgres', 'clickhouse')
 * @param {Object} props.config - Current connection config values
 * @param {Function} props.onChange - Callback when a field value changes (fieldName, value)
 * @param {boolean} props.readOnly - Whether the form is read-only
 * @param {boolean} props.showRequired - Whether to show required indicators
 * @param {Object} props.discoveredResources - Resources discovered from backend (for create mode)
 * @param {string} props.discoveryStatus - Status of discovery: 'idle', 'loading', 'success', 'error'
 * @param {boolean} props.isCreateMode - Whether in create mode (uses new field structure)
 */
export default function ConnectionFormRenderer({
  datasourceType,
  config,
  onChange,
  readOnly = false,
  showRequired = true,
  discoveredResources = {},
  discoveryStatus = 'idle',
  isCreateMode = false,
}) {
  const schema = getConnectionSchema(datasourceType);

  // BigQuery and other OAuth-based datasources show a message instead of fields
  if (schema.usesOAuth) {
    return (
      <p className="text-sm text-muted-foreground">
        {schema.oauthMessage}
      </p>
    );
  }

  const handleChange = (fieldName, value) => {
    onChange(fieldName, value);
  };

  // Use new schema structure (connectionFields + discoveryFields) if available
  // Discovery fields are rendered by DatasourceModal AFTER credentials section
  const hasNewSchemaStructure = schema.connectionFields || schema.discoveryFields;

  if (hasNewSchemaStructure) {
    // Only render connectionFields here - discovery fields go after credentials
    const connectionFields = schema.connectionFields || [];
    const connectionRows = groupFieldsIntoRows(connectionFields);

    return (
      <div className="space-y-4">
        {/* Connection fields section */}
        {connectionRows.map((row, rowIndex) => (
          <div key={`conn-${rowIndex}`} className="grid grid-cols-2 gap-4">
            {row.map((field, colIndex) => {
              if (!field) {
                return <div key={`empty-${colIndex}`} />;
              }
              return (
                <div
                  key={field.name}
                  className={field.span === 2 ? 'col-span-2' : ''}
                >
                  <FormField
                    field={field}
                    value={config[field.name]}
                    onChange={handleChange}
                    readOnly={readOnly}
                    showRequired={showRequired}
                    discoveredResources={discoveredResources}
                    discoveryStatus={discoveryStatus}
                  />
                </div>
              );
            })}
          </div>
        ))}

        {/* SSH Tunnel Section - only for supported types and admin users */}
        {schema.sshTunnelSupported && !readOnly && (
          <SSHTunnelSection
            config={config}
            onChange={handleChange}
            readOnly={readOnly}
          />
        )}
      </div>
    );
  }

  // Legacy mode: use schema.fields
  // No fields defined (shouldn't happen, but handle gracefully)
  if (!schema.fields || schema.fields.length === 0) {
    return (
      <p className="text-sm text-muted-foreground">
        No connection settings required for this datasource type.
      </p>
    );
  }

  // Group fields into rows for grid layout
  const rows = groupFieldsIntoRows(schema.fields);

  return (
    <div className="space-y-4">
      {rows.map((row, rowIndex) => (
        <div key={rowIndex} className="grid grid-cols-2 gap-4">
          {row.map((field, colIndex) => {
            if (!field) {
              // Empty placeholder for grid alignment
              return <div key={`empty-${colIndex}`} />;
            }
            return (
              <div
                key={field.name}
                className={field.span === 2 ? 'col-span-2' : ''}
              >
                <FormField
                  field={field}
                  value={config[field.name]}
                  onChange={handleChange}
                  readOnly={readOnly}
                  showRequired={showRequired}
                  discoveredResources={discoveredResources}
                  discoveryStatus={discoveryStatus}
                />
              </div>
            );
          })}
        </div>
      ))}

      {/* SSH Tunnel Section - only for supported types and admin users */}
      {schema.sshTunnelSupported && !readOnly && (
        <SSHTunnelSection
          config={config}
          onChange={handleChange}
          readOnly={readOnly}
        />
      )}

      {/* Show SSH config in read-only mode if enabled */}
      {schema.sshTunnelSupported && readOnly && config.ssh_enabled && (
        <SSHTunnelSection
          config={config}
          onChange={() => {}}
          readOnly={true}
        />
      )}
    </div>
  );
}
