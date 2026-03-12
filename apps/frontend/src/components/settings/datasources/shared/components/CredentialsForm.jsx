// SPDX-License-Identifier: AGPL-3.0-or-later
// shared/components/CredentialsForm.jsx
import { FormField } from './FormField';

/**
 * CredentialsForm - Generic credentials form from schema
 *
 * Renders a grid of form fields based on the provided field definitions.
 * Uses FormField for consistent rendering and handles 2-column layout.
 *
 * @param {Array} fields - Array of field definitions from schema
 * @param {Object} values - Current values { [fieldName]: value }
 * @param {function} onChange - Handler for value changes (fieldName, value) => void
 * @param {boolean} disabled - Whether all fields are disabled
 * @param {Object} credentialFlags - Flags indicating stored credentials { has_password, has_username, has_access_token }
 * @param {Object} dirtyFields - Which fields have been modified { [fieldName]: boolean }
 */
export function CredentialsForm({
  fields,
  values,
  onChange,
  disabled = false,
  credentialFlags = {},
  dirtyFields = {},
}) {
  if (!fields || fields.length === 0) {
    return null;
  }

  // Map field names to credential flags
  const getHasStoredValue = (fieldName) => {
    if (fieldName === 'password') return credentialFlags.has_password;
    if (fieldName === 'username') return credentialFlags.has_username;
    if (fieldName === 'access_token') return credentialFlags.has_access_token;
    return false;
  };

  return (
    <div className="grid grid-cols-2 gap-4">
      {fields.map((field) => {
        const hasStoredValue = getHasStoredValue(field.name);
        const isDirty = dirtyFields[field.name] || false;
        // Show masked indicator if credential is stored and field hasn't been modified
        const showMaskedIndicator = hasStoredValue && !isDirty;

        return (
          <FormField
            key={field.name}
            field={field}
            value={values[field.name]}
            onChange={(value) => onChange(field.name, value)}
            disabled={disabled}
            showMaskedIndicator={showMaskedIndicator}
          />
        );
      })}
    </div>
  );
}
