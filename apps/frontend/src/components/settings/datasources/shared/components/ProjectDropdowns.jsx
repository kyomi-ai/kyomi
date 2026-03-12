// SPDX-License-Identifier: AGPL-3.0-or-later
// shared/components/ProjectDropdowns.jsx
import { useState } from 'react';
import { Alert, AlertDescription } from '@/components/ui/alert';
import {
  Select,
  SelectTrigger,
  SelectValue,
  SelectContent,
  SelectItem,
  SelectSeparator,
} from '@/components/ui/select';
import { AlertCircle } from 'lucide-react';

/**
 * ProjectDropdowns - Reusable project dropdown fields for BigQuery
 * Uses the combobox pattern: dropdown with "Enter custom project ID" option
 *
 * Handles both formats:
 * - OAuth modes: projects as { project_id, name } objects
 * - Service account mode: projects as plain strings
 *
 * @param {Object} credentialsForm - Current form values
 * @param {function} onCredentialsChange - Handler for value changes (fieldName, value) => void
 * @param {Array} projectsList - Array of projects (strings or { project_id, name })
 * @param {string} errorMessage - Optional error message to display
 */
export function ProjectDropdowns({ credentialsForm, onCredentialsChange, projectsList, errorMessage }) {
  const [customBillingProject, setCustomBillingProject] = useState(false);
  const [customDefaultProject, setCustomDefaultProject] = useState(false);

  const renderProjectOption = (project) => {
    const value = typeof project === 'string' ? project : project.project_id;
    const label = typeof project === 'string' ? project : project.name;
    return (
      <SelectItem key={value} value={value}>
        {label}
      </SelectItem>
    );
  };

  const hasProjects = projectsList.length > 0;

  return (
    <div className="space-y-3">
      {/* Show error message but still allow manual entry */}
      {errorMessage && !hasProjects && (
        <Alert variant="warning">
          <AlertCircle className="h-4 w-4" />
          <AlertDescription>
            {errorMessage} You can still enter project IDs manually below.
          </AlertDescription>
        </Alert>
      )}

      <div className="grid grid-cols-2 gap-4">
        {/* Billing Project */}
        <div>
          <label className="block text-sm font-medium text-foreground mb-1">Billing Project</label>
          {hasProjects && !customBillingProject ? (
            <Select
              value={credentialsForm.billing_project || undefined}
              onValueChange={(value) => {
                if (value === '__custom__') {
                  setCustomBillingProject(true);
                  onCredentialsChange('billing_project', '');
                } else {
                  onCredentialsChange('billing_project', value);
                }
              }}
            >
              <SelectTrigger>
                <SelectValue placeholder="Select a project..." />
              </SelectTrigger>
              <SelectContent>
                {projectsList.map(renderProjectOption)}
                <SelectSeparator />
                <SelectItem value="__custom__">Enter custom project ID...</SelectItem>
              </SelectContent>
            </Select>
          ) : (
            <div className="space-y-1">
              <input
                type="text"
                value={credentialsForm.billing_project || ''}
                onChange={(e) => onCredentialsChange('billing_project', e.target.value)}
                placeholder="Enter project ID"
                className="w-full px-3 py-2 border border-input rounded-md bg-background text-sm"
              />
              {hasProjects && (
                <button
                  type="button"
                  onClick={() => setCustomBillingProject(false)}
                  className="text-sm text-primary hover:underline"
                >
                  Back to dropdown
                </button>
              )}
            </div>
          )}
        </div>

        {/* Default Project */}
        <div>
          <label className="block text-sm font-medium text-foreground mb-1">Default Project</label>
          {hasProjects && !customDefaultProject ? (
            <Select
              value={credentialsForm.default_project || undefined}
              onValueChange={(value) => {
                if (value === '__custom__') {
                  setCustomDefaultProject(true);
                  onCredentialsChange('default_project', '');
                } else {
                  onCredentialsChange('default_project', value);
                }
              }}
            >
              <SelectTrigger>
                <SelectValue placeholder="Select a project..." />
              </SelectTrigger>
              <SelectContent>
                {projectsList.map(renderProjectOption)}
                <SelectSeparator />
                <SelectItem value="__custom__">Enter custom project ID...</SelectItem>
              </SelectContent>
            </Select>
          ) : (
            <div className="space-y-1">
              <input
                type="text"
                value={credentialsForm.default_project || ''}
                onChange={(e) => onCredentialsChange('default_project', e.target.value)}
                placeholder="Enter project ID"
                className="w-full px-3 py-2 border border-input rounded-md bg-background text-sm"
              />
              {hasProjects && (
                <button
                  type="button"
                  onClick={() => setCustomDefaultProject(false)}
                  className="text-sm text-primary hover:underline"
                >
                  Back to dropdown
                </button>
              )}
            </div>
          )}
          <p className="text-xs text-muted-foreground mt-1">
            {hasProjects
              ? 'Select from discovered projects or enter a custom ID'
              : 'Connect to discover projects, or enter project ID manually'}
          </p>
        </div>
      </div>
    </div>
  );
}
