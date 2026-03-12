// SPDX-License-Identifier: AGPL-3.0-or-later
import { useState, useEffect } from 'react';
import {
  RefreshCw,
  Database,
  AlertCircle,
  Plus,
  Trash2,
  Clock,
  Layers,
} from 'lucide-react';
import { Spinner } from '../ui/spinner';
import { Button } from '../ui/button';
import { Badge } from '../ui/badge';
import { Alert, AlertDescription } from '../ui/alert';
import { Card, CardHeader, CardTitle, CardDescription, CardContent } from '../ui/card';
import { Switch } from '../ui/switch';
import { toast } from '../../lib/toast';

/**
 * CatalogSection - Displays catalog status and controls for a datasource
 *
 * Shows:
 * - Table count and schema/project count
 * - Last indexed timestamp
 * - Current indexing status
 * - Manual refresh button
 * - Catalog configuration (what to index)
 *
 * For PostgreSQL: Shows a schema picker that fetches available schemas from the database
 *
 * Per UX plan: This section MUST be inline, NOT in a modal.
 * See: docs/specifications/DATASOURCE_SETTINGS_UX_PLAN.md
 */
export default function CatalogSection({
  datasource,
  apiClient,
  isAdmin,
  onConfigChange,
}) {
  const [loading, setLoading] = useState(true);
  const [status, setStatus] = useState(null);
  const [refreshing, setRefreshing] = useState(false);
  const [newItemInput, setNewItemInput] = useState('');

  // Schema picker state (for PostgreSQL, ClickHouse, etc.)
  const [availableSchemas, setAvailableSchemas] = useState([]);
  const [loadingSchemas, setLoadingSchemas] = useState(false);
  const [schemaError, setSchemaError] = useState(null);

  // Fetch catalog status on mount and when datasource changes
  useEffect(() => {
    if (datasource?.id) {
      fetchCatalogStatus();
    }
  }, [datasource?.id]);

  // Auto-fetch available schemas for datasources that support discovery
  useEffect(() => {
    if (datasource?.id && isAdmin) {
      const supportsDiscovery = ['postgres', 'clickhouse', 'snowflake', 'bigquery', 'mysql', 'databricks', 'redshift', 'sqlserver', 'synapse'].includes(
        datasource.datasource_type
      );
      if (supportsDiscovery) {
        fetchAvailableSchemas();
      }
    }
  }, [datasource?.id, isAdmin]);

  const fetchCatalogStatus = async () => {
    if (!datasource?.id || !apiClient) return;

    setLoading(true);
    try {
      const response = await apiClient.get(`/api/v1/datasources/${datasource.id}/catalog/status`);
      setStatus(response.data);
    } catch (error) {
      // Don't show error toast on initial load - might just be no data yet
    } finally {
      setLoading(false);
    }
  };

  // Fetch available schemas from the database
  const fetchAvailableSchemas = async () => {
    if (!datasource?.id || !apiClient) return;

    // Only fetch for datasource types that support schema discovery
    const supportsSchemaDiscovery = ['postgres', 'clickhouse', 'snowflake', 'bigquery', 'mysql', 'databricks', 'redshift', 'sqlserver', 'synapse'].includes(
      datasource.datasource_type
    );
    if (!supportsSchemaDiscovery) return;

    setLoadingSchemas(true);
    setSchemaError(null);
    try {
      const response = await apiClient.get(`/api/v1/datasources/${datasource.id}/schemas`);
      setAvailableSchemas(response.data.schemas || []);
      if (response.data.message) {
        // Show message for unsupported types
        setSchemaError(response.data.message);
      }
    } catch (error) {
      const errorMessage = error.response?.data?.detail || 'Failed to fetch schemas. Check credentials.';
      setSchemaError(errorMessage);
      setAvailableSchemas([]);
    } finally {
      setLoadingSchemas(false);
    }
  };

  const handleRefresh = async () => {
    if (!datasource?.id || !apiClient) return;

    setRefreshing(true);
    try {
      const response = await apiClient.post(`/api/v1/datasources/${datasource.id}/catalog/refresh`, {
        force: false,
      });

      if (response.data.status === 'completed') {
        toast.success(response.data.message);
        await fetchCatalogStatus(); // Refresh stats
      } else if (response.data.status === 'already_running') {
        toast.info(response.data.message);
      } else {
        toast.error(response.data.message);
      }
    } catch (error) {
      toast.error(error.response?.data?.detail || 'Failed to refresh catalog');
    } finally {
      setRefreshing(false);
    }
  };

  // Get the appropriate config key and label based on datasource type
  const getCatalogConfigInfo = () => {
    switch (datasource?.datasource_type) {
      case 'bigquery':
        return {
          key: 'catalog_projects',
          label: 'Projects to Index',
          itemLabel: 'project',
          placeholder: 'Enter project ID',
          supportsDiscovery: true,
        };
      case 'postgres':
        return {
          key: 'catalog_schemas',
          label: 'Schemas to Index',
          itemLabel: 'schema',
          placeholder: 'Enter schema name (e.g., public)',
          supportsDiscovery: true,
        };
      case 'clickhouse':
        return {
          key: 'catalog_databases',
          label: 'Databases to Index',
          itemLabel: 'database',
          placeholder: 'Enter database name',
          supportsDiscovery: true,
        };
      case 'snowflake':
        return {
          key: 'catalog_databases',
          label: 'Databases to Index',
          itemLabel: 'database',
          placeholder: 'Enter database name',
          supportsDiscovery: true,
        };
      case 'mysql':
        return {
          key: 'catalog_databases',
          label: 'Databases to Index',
          itemLabel: 'database',
          placeholder: 'Enter database name',
          supportsDiscovery: true,
        };
      case 'databricks':
        return {
          key: 'catalog_catalogs',
          label: 'Catalogs to Index',
          itemLabel: 'catalog',
          placeholder: 'Enter catalog name (e.g., main)',
          supportsDiscovery: true,
        };
      case 'redshift':
        return {
          key: 'catalog_schemas',
          label: 'Schemas to Index',
          itemLabel: 'schema',
          placeholder: 'Enter schema name (e.g., public)',
          supportsDiscovery: true,
        };
      case 'sqlserver':
        return {
          key: 'catalog_schemas',
          label: 'Schemas to Index',
          itemLabel: 'schema',
          placeholder: 'Enter schema name (e.g., dbo)',
          supportsDiscovery: true,
        };
      case 'synapse':
        return {
          key: 'catalog_schemas',
          label: 'Schemas to Index',
          itemLabel: 'schema',
          placeholder: 'Enter schema name (e.g., dbo)',
          supportsDiscovery: true,
        };
      default:
        return {
          key: 'catalog_items',
          label: 'Items to Index',
          itemLabel: 'item',
          placeholder: 'Enter item name',
          supportsDiscovery: false,
        };
    }
  };

  const configInfo = getCatalogConfigInfo();
  const catalogItems = status?.catalog_config?.[configInfo.key] || [];

  // Toggle a schema in the selected list
  const handleToggleSchema = (schema) => {
    const isSelected = catalogItems.includes(schema);
    const newItems = isSelected
      ? catalogItems.filter((s) => s !== schema)
      : [...catalogItems, schema];

    onConfigChange?.(configInfo.key, newItems);

    // Update local state optimistically
    setStatus((prev) => ({
      ...(prev || {}),
      catalog_config: {
        ...(prev?.catalog_config || {}),
        [configInfo.key]: newItems,
      },
    }));
  };

  // Select all available schemas
  const handleSelectAll = () => {
    onConfigChange?.(configInfo.key, [...availableSchemas]);
    setStatus((prev) => ({
      ...(prev || {}),
      catalog_config: {
        ...(prev?.catalog_config || {}),
        [configInfo.key]: [...availableSchemas],
      },
    }));
  };

  // Clear all selected schemas
  const handleClearAll = () => {
    onConfigChange?.(configInfo.key, []);
    setStatus((prev) => ({
      ...(prev || {}),
      catalog_config: {
        ...(prev?.catalog_config || {}),
        [configInfo.key]: [],
      },
    }));
  };

  const handleAddItem = () => {
    const value = newItemInput.trim();
    if (!value) return;

    if (catalogItems.includes(value)) {
      toast.error(`${configInfo.itemLabel} "${value}" is already in the list`);
      return;
    }

    const newItems = [...catalogItems, value];

    // Call parent handler to save to backend
    if (onConfigChange) {
      onConfigChange(configInfo.key, newItems);
    } else {
      toast.error('Unable to save - configuration error');
      return;
    }

    setNewItemInput('');

    // Update local state optimistically
    setStatus((prev) => ({
      ...(prev || {}),
      catalog_config: {
        ...(prev?.catalog_config || {}),
        [configInfo.key]: newItems,
      },
    }));
  };

  const handleRemoveItem = (item) => {
    const newItems = catalogItems.filter((i) => i !== item);
    onConfigChange?.(configInfo.key, newItems);

    // Update local state optimistically
    setStatus((prev) => ({
      ...prev,
      catalog_config: {
        ...prev?.catalog_config,
        [configInfo.key]: newItems,
      },
    }));
  };

  const formatLastIndexed = (isoString) => {
    if (!isoString) return 'Never';
    try {
      const date = new Date(isoString);
      return date.toLocaleString();
    } catch {
      return isoString;
    }
  };

  if (loading) {
    return (
      <Card>
        <CardHeader>
          <CardTitle className="text-base flex items-center gap-2">
            <Layers className="h-4 w-4" />
            Data Catalog
          </CardTitle>
        </CardHeader>
        <CardContent>
          <div className="flex items-center justify-center py-8">
            <Spinner className="text-muted-foreground" />
            <span className="ml-2 text-sm text-muted-foreground">Loading catalog status...</span>
          </div>
        </CardContent>
      </Card>
    );
  }

  // Render schema picker (checkbox list)
  const renderSchemaPicker = () => {
    const itemLabel = configInfo.itemLabel;
    const itemLabelPlural = `${itemLabel}s`;

    // Loading state
    if (loadingSchemas) {
      return (
        <div className="flex items-center gap-2 py-4">
          <Spinner className="text-muted-foreground" />
          <span className="text-sm text-muted-foreground">Loading {itemLabelPlural}...</span>
        </div>
      );
    }

    // Error state
    if (schemaError) {
      return (
        <div className="space-y-3">
          <Alert variant="warning">
            <AlertCircle className="h-4 w-4" />
            <AlertDescription>{schemaError}</AlertDescription>
          </Alert>
          <Button variant="outline" size="sm" onClick={fetchAvailableSchemas}>
            <RefreshCw className="h-4 w-4 mr-2" />
            Retry
          </Button>
        </div>
      );
    }

    // No items found
    if (availableSchemas.length === 0) {
      const helpText = datasource?.datasource_type === 'bigquery'
        ? 'No projects found. Make sure your Google account is connected with BigQuery access.'
        : `No ${itemLabelPlural} found. Make sure your credentials are configured and the connection works.`;

      return (
        <div className="space-y-3">
          <p className="text-sm text-muted-foreground">{helpText}</p>
          <Button variant="outline" size="sm" onClick={fetchAvailableSchemas}>
            <RefreshCw className="h-4 w-4 mr-2" />
            Retry
          </Button>
        </div>
      );
    }

    // Item list (schemas/projects/databases)
    return (
      <div className="space-y-3">
        {/* Action buttons */}
        <div className="flex items-center gap-2">
          <Button variant="ghost" size="sm" onClick={handleSelectAll}>
            Select All
          </Button>
          <Button variant="ghost" size="sm" onClick={handleClearAll}>
            Clear
          </Button>
          <span className="text-xs text-muted-foreground ml-auto">
            {catalogItems.length} of {availableSchemas.length} selected
          </span>
        </div>

        {/* Checkbox list */}
        <div className="border border-border rounded-lg divide-y divide-border max-h-60 overflow-y-auto">
          {availableSchemas.map((item) => {
            const isSelected = catalogItems.includes(item);
            return (
              <label
                key={item}
                className="flex items-center gap-3 px-3 py-2 cursor-pointer hover:bg-accent/50 transition-colors"
              >
                <input
                  type="checkbox"
                  checked={isSelected}
                  onChange={() => handleToggleSchema(item)}
                  className="h-4 w-4 rounded border-border"
                />
                <span className="text-sm font-mono">{item}</span>
              </label>
            );
          })}
        </div>

        {/* Help text */}
        <p className="text-xs text-muted-foreground">
          {catalogItems.length === 0
            ? `Leave empty to index all available ${itemLabelPlural}.`
            : 'Changes take effect on next refresh.'}
        </p>
      </div>
    );
  };

  // Render text input (for BigQuery projects, etc.)
  const renderTextInput = () => {
    return (
      <>
        {/* Items list */}
        {catalogItems.length > 0 ? (
          <div className="space-y-2 mb-3">
            {catalogItems.map((item, index) => (
              <div
                key={index}
                className="flex items-center justify-between p-2 bg-muted/50 rounded-lg"
              >
                <div className="flex items-center gap-2">
                  <Database className="h-4 w-4 text-muted-foreground" />
                  <span className="text-sm font-mono">{item}</span>
                </div>
                <Button
                  variant="ghost"
                  size="sm"
                  onClick={() => handleRemoveItem(item)}
                  className="h-8 w-8 p-0"
                >
                  <Trash2 className="h-4 w-4 text-error-foreground" />
                </Button>
              </div>
            ))}
          </div>
        ) : (
          <p className="text-sm text-muted-foreground mb-3">
            No {configInfo.itemLabel}s configured.
          </p>
        )}

        {/* Add new item */}
        <div className="flex gap-2">
          <input
            type="text"
            value={newItemInput}
            onChange={(e) => setNewItemInput(e.target.value)}
            onKeyDown={(e) => {
              if (e.key === 'Enter') {
                e.preventDefault();
                handleAddItem();
              }
            }}
            placeholder={configInfo.placeholder}
            className="flex-1 px-3 py-2 border border-input rounded-md bg-background text-foreground text-sm focus:ring-2 focus:ring-ring"
          />
          <Button
            variant="outline"
            size="sm"
            onClick={handleAddItem}
            disabled={!newItemInput.trim()}
          >
            <Plus className="h-4 w-4" />
          </Button>
        </div>

        {/* Help text */}
        <p className="text-xs text-muted-foreground mt-2">
          Add {configInfo.itemLabel}s to index. Changes take effect on next refresh.
        </p>
      </>
    );
  };

  return (
    <Card>
      <CardHeader className="pb-3">
        <div className="flex items-center justify-between">
          <div>
            <CardTitle className="text-base flex items-center gap-2">
              <Layers className="h-4 w-4" />
              Data Catalog
            </CardTitle>
            <CardDescription className="mt-1">
              Indexed tables for semantic search and AI discovery
            </CardDescription>
          </div>
          {isAdmin && !datasource?.connection_config?.is_sample && (
            <Button
              variant="outline"
              size="sm"
              onClick={handleRefresh}
              disabled={refreshing || status?.indexing_status === 'running'}
            >
              {refreshing ? (
                <>
                  <Spinner className="mr-2" />
                  Refreshing...
                </>
              ) : (
                <>
                  <RefreshCw className="h-4 w-4 mr-2" />
                  Refresh
                </>
              )}
            </Button>
          )}
        </div>
      </CardHeader>
      <CardContent className="space-y-4">
        {/* Status Alert for Indexing */}
        {status?.indexing_status === 'running' && (
          <Alert variant="info">
            <Spinner />
            <AlertDescription>
              Catalog indexing in progress...
              {status?.indexing_progress?.current && (
                <span className="ml-1">({status.indexing_progress.current})</span>
              )}
            </AlertDescription>
          </Alert>
        )}

        {/* Stats Row */}
        <div className="grid grid-cols-2 sm:grid-cols-3 gap-4">
          <div className="flex items-center gap-2 p-3 bg-muted/50 rounded-lg">
            <Database className="h-4 w-4 text-muted-foreground" />
            <div>
              <p className="text-lg font-semibold">{status?.table_count || 0}</p>
              <p className="text-xs text-muted-foreground">Tables indexed</p>
            </div>
          </div>
          <div className="flex items-center gap-2 p-3 bg-muted/50 rounded-lg">
            <Layers className="h-4 w-4 text-muted-foreground" />
            <div>
              <p className="text-lg font-semibold">{status?.schema_count || 0}</p>
              <p className="text-xs text-muted-foreground">
                {datasource?.datasource_type === 'bigquery' ? 'Datasets' : 'Schemas'}
              </p>
            </div>
          </div>
          <div className="flex items-center gap-2 p-3 bg-muted/50 rounded-lg col-span-2 sm:col-span-1">
            <Clock className="h-4 w-4 text-muted-foreground" />
            <div>
              <p className="text-sm font-medium truncate">
                {formatLastIndexed(status?.last_indexed)}
              </p>
              <p className="text-xs text-muted-foreground">Last indexed</p>
            </div>
          </div>
        </div>

        {/* Catalog Configuration - Admin only, not for sample datasources */}
        {isAdmin && !datasource?.connection_config?.is_sample && (
          <div className="border-t border-border pt-4">
            <h4 className="text-sm font-medium text-foreground mb-3">{configInfo.label}</h4>

            {/* BigQuery special: Include Public Datasets toggle */}
            {datasource?.datasource_type === 'bigquery' && (
              <div className="flex items-center justify-between p-3 bg-muted/30 rounded-lg mb-3">
                <div>
                  <p className="text-sm font-medium">Include Public Datasets</p>
                  <p className="text-xs text-muted-foreground">
                    Show BigQuery public datasets in search results
                  </p>
                </div>
                <Switch
                  checked={status?.catalog_config?.include_public_datasets || false}
                  onCheckedChange={(newValue) => {
                    onConfigChange?.('include_public_datasets', newValue);
                    setStatus((prev) => ({
                      ...prev,
                      catalog_config: {
                        ...prev?.catalog_config,
                        include_public_datasets: newValue,
                      },
                    }));
                  }}
                />
              </div>
            )}

            {/* Render schema picker or text input based on datasource type */}
            {configInfo.supportsDiscovery ? renderSchemaPicker() : renderTextInput()}
          </div>
        )}
      </CardContent>
    </Card>
  );
}
