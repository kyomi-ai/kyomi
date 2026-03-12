// SPDX-License-Identifier: AGPL-3.0-or-later
// shared/components/FormField.jsx
import { Select, SelectTrigger, SelectValue, SelectContent, SelectItem } from '@/components/ui/select';
import { FIELD_TYPES } from '../schemas';

/**
 * FormField - Generic field renderer that handles all FIELD_TYPES
 *
 * Renders appropriate input based on field.type:
 * - text, number, password: Standard input elements
 * - textarea: Multi-line text input
 * - select: Dropdown using shadcn/ui Select
 * - checkbox: Checkbox with inline label
 * - discovery: Treated as select (discovery data populated externally)
 *
 * @param {Object} field - Field definition from schema
 * @param {*} value - Current field value
 * @param {function} onChange - Value change handler
 * @param {boolean} disabled - Whether field is disabled
 * @param {string} error - Error message to display
 * @param {boolean} showMaskedIndicator - Show masked placeholder for stored credentials
 */
export function FormField({
  field,
  value,
  onChange,
  disabled = false,
  error = null,
  showMaskedIndicator = false,
}) {
  const { name, type, label, placeholder, required, helpText, options, rows, description } = field;

  const inputClasses =
    'w-full px-3 py-2 border border-input rounded-md bg-background text-foreground text-sm ' +
    'focus:outline-none focus:ring-1 focus:ring-ring disabled:cursor-not-allowed disabled:opacity-50';

  const renderInput = () => {
    switch (type) {
      case FIELD_TYPES.SELECT:
      case FIELD_TYPES.DISCOVERY:
        return (
          <Select value={value || ''} onValueChange={onChange} disabled={disabled}>
            <SelectTrigger>
              <SelectValue placeholder={placeholder} />
            </SelectTrigger>
            <SelectContent>
              {options?.map((opt) => (
                <SelectItem key={opt.value} value={opt.value}>
                  {opt.label}
                </SelectItem>
              ))}
            </SelectContent>
          </Select>
        );

      case FIELD_TYPES.TEXTAREA:
        return (
          <textarea
            name={name}
            value={value || ''}
            onChange={(e) => onChange(e.target.value)}
            placeholder={placeholder}
            rows={rows || 4}
            disabled={disabled}
            className={`${inputClasses} font-mono resize-none`}
          />
        );

      case FIELD_TYPES.CHECKBOX:
        return (
          <label className="flex items-center gap-2 cursor-pointer">
            <input
              type="checkbox"
              name={name}
              checked={value || false}
              onChange={(e) => onChange(e.target.checked)}
              disabled={disabled}
              className="h-4 w-4 rounded border-border text-primary focus:ring-ring"
            />
            <span className="text-sm text-foreground">{description || label}</span>
          </label>
        );

      case FIELD_TYPES.PASSWORD:
        // For password fields, show masked indicator if a value is stored but not yet modified
        // This tells user a password exists without revealing it
        return (
          <input
            type="password"
            name={name}
            value={value || ''}
            onChange={(e) => onChange(e.target.value)}
            placeholder={showMaskedIndicator ? '••••••••' : placeholder}
            disabled={disabled}
            className={inputClasses}
            autoComplete="off"
          />
        );

      case FIELD_TYPES.NUMBER:
        return (
          <input
            type="number"
            name={name}
            value={value ?? ''}
            onChange={(e) => {
              const val = e.target.value;
              onChange(val === '' ? null : Number(val));
            }}
            placeholder={placeholder}
            disabled={disabled}
            className={inputClasses}
          />
        );

      case FIELD_TYPES.TEXT:
      default:
        return (
          <input
            type="text"
            name={name}
            value={value || ''}
            onChange={(e) => onChange(e.target.value)}
            placeholder={placeholder}
            disabled={disabled}
            className={inputClasses}
          />
        );
    }
  };

  // Determine grid column class
  const getGridColumnClass = () => {
    if (field.gridColumn === 'full') return 'col-span-2';
    if (field.gridColumn === 2) return ''; // Normal single column
    return ''; // Default single column
  };

  // Checkbox has inline label - render differently
  if (type === FIELD_TYPES.CHECKBOX) {
    return (
      <div className={getGridColumnClass()}>
        {renderInput()}
        {helpText && <p className="text-xs text-muted-foreground mt-1">{helpText}</p>}
        {error && <p className="text-xs text-error-foreground mt-1">{error}</p>}
      </div>
    );
  }

  return (
    <div className={getGridColumnClass()}>
      <label className="block text-sm font-medium text-foreground mb-1">
        {label}
        {required && <span className="text-destructive"> *</span>}
      </label>
      {renderInput()}
      {helpText && <p className="text-xs text-muted-foreground mt-1">{helpText}</p>}
      {error && <p className="text-xs text-destructive mt-1">{error}</p>}
    </div>
  );
}
