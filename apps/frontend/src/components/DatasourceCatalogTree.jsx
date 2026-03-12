// SPDX-License-Identifier: AGPL-3.0-or-later
import { useState, useEffect } from 'react';
import apiClient from '../api/apiClient';
import { Tooltip, TooltipTrigger, TooltipContent } from './ui/tooltip';
import { Spinner } from './ui/spinner';

/**
 * DatasourceCatalogTree - Generic catalog tree for any datasource type
 *
 * Fetches the catalog tree from the indexed cache via:
 * GET /api/v1/datasources/{datasourceId}/catalog/tree
 *
 * Tree structure varies by datasource type:
 * - BigQuery: project > dataset > table
 * - PostgreSQL: schema > table
 * - ClickHouse: database > table
 *
 * @param {string} datasourceId - Datasource identifier (slug, e.g., "production-postgres")
 */
const DatasourceCatalogTree = ({
  onTableClick,
  onColumnClick,
  onTableDetails,
  searchQuery = '',
  useSemanticSearch = false,
  refreshTrigger = 0,
  datasourceId = null
}) => {
  const [tree, setTree] = useState([]);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState(null);
  const [datasourceType, setDatasourceType] = useState(null);
  const [tableCount, setTableCount] = useState(0);

  // Expanded states
  const [expandedNodes, setExpandedNodes] = useState(new Set());

  // Semantic search
  const [semanticResults, setSemanticResults] = useState([]);
  const [searchLoading, setSearchLoading] = useState(false);

  // Load tree when datasourceId changes or refresh triggered
  useEffect(() => {
    if (!datasourceId) {
      setTree([]);
      setError(null);
      return;
    }

    loadTree();
  }, [datasourceId, refreshTrigger]);

  // Semantic search
  useEffect(() => {
    if (!useSemanticSearch || !searchQuery || searchQuery.length < 3) {
      setSemanticResults([]);
      setSearchLoading(false);
      return;
    }

    const abortController = new AbortController();

    const timeoutId = setTimeout(async () => {
      setSearchLoading(true);

      try {
        const requestBody = {
          query: searchQuery,
          limit: 50,
          include_public: true
        };

        if (datasourceId) {
          requestBody.datasource = datasourceId; // datasourceId prop now contains slug
        }

        const response = await apiClient.post('/api/v1/bigquery/search', requestBody, {
          signal: abortController.signal
        });

        const results = response.data.results || [];
        setSemanticResults(results);
        setSearchLoading(false);
      } catch (error) {
        if (error.name === 'CanceledError' || error.name === 'AbortError') return;
        setSemanticResults([]);
        setSearchLoading(false);
      }
    }, 700);

    return () => {
      abortController.abort();
      clearTimeout(timeoutId);
    };
  }, [searchQuery, useSemanticSearch, datasourceId]);

  const loadTree = async () => {
    try {
      setLoading(true);
      setError(null);

      const response = await apiClient.get(`/api/v1/datasources/${datasourceId}/catalog/tree?include_columns=true`);
      const data = response.data;

      setTree(data.tree || []);
      setDatasourceType(data.datasource_type);
      setTableCount(data.table_count || 0);
      setExpandedNodes(new Set()); // Reset expanded state
    } catch (err) {
      setError(err.response?.data?.detail || err.message || 'Failed to load catalog');
      setTree([]);
    } finally {
      setLoading(false);
    }
  };

  const toggleNode = (nodeId) => {
    setExpandedNodes(prev => {
      const newSet = new Set(prev);
      if (newSet.has(nodeId)) {
        newSet.delete(nodeId);
      } else {
        newSet.add(nodeId);
      }
      return newSet;
    });
  };

  // Render a tree node recursively
  const renderNode = (node, depth = 0) => {
    const isExpanded = expandedNodes.has(node.id);
    const hasChildren = node.children && node.children.length > 0;
    const isTable = node.type === 'table';
    const isColumn = node.type === 'column';

    // Icon based on node type
    const getIcon = () => {
      switch (node.type) {
        case 'project':
        case 'database':
          return (
            <svg className="w-4 h-4 flex-shrink-0 text-muted-foreground" fill="none" stroke="currentColor" viewBox="0 0 24 24">
              <path strokeLinecap="round" strokeLinejoin="round" strokeWidth={2} d="M3 7v10a2 2 0 002 2h14a2 2 0 002-2V9a2 2 0 00-2-2h-6l-2-2H5a2 2 0 00-2 2z" />
            </svg>
          );
        case 'dataset':
        case 'schema':
          return (
            <svg className="w-4 h-4 flex-shrink-0 text-muted-foreground" fill="none" stroke="currentColor" viewBox="0 0 24 24">
              <path strokeLinecap="round" strokeLinejoin="round" strokeWidth={2} d="M4 7v10c0 2.21 3.582 4 8 4s8-1.79 8-4V7M4 7c0 2.21 3.582 4 8 4s8-1.79 8-4M4 7c0-2.21 3.582-4 8-4s8 1.79 8 4m0 5c0 2.21-3.582 4-8 4s-8-1.79-8-4" />
            </svg>
          );
        case 'table':
          return (
            <svg className="w-3 h-3 flex-shrink-0 text-primary" fill="none" stroke="currentColor" viewBox="0 0 24 24">
              <path strokeLinecap="round" strokeLinejoin="round" strokeWidth={2} d="M3 10h18M3 14h18m-9-4v8m-7 0h14a2 2 0 002-2V8a2 2 0 00-2-2H5a2 2 0 00-2 2v8a2 2 0 002 2z" />
            </svg>
          );
        case 'column':
          return (
            <svg className="w-3 h-3 flex-shrink-0 text-muted-foreground" fill="none" stroke="currentColor" viewBox="0 0 24 24">
              <path strokeLinecap="round" strokeLinejoin="round" strokeWidth={2} d="M7 7h.01M7 3h5c.512 0 1.024.195 1.414.586l7 7a2 2 0 010 2.828l-7 7a2 2 0 01-2.828 0l-7-7A1.994 1.994 0 013 12V7a4 4 0 014-4z" />
            </svg>
          );
        default:
          return null;
      }
    };

    const handleClick = () => {
      if (isTable) {
        onTableClick?.(node.id);
      } else if (isColumn) {
        onColumnClick?.(node.name);
      } else if (hasChildren) {
        toggleNode(node.id);
      }
    };

    return (
      <div key={node.id} style={{ marginLeft: depth > 0 ? '1rem' : 0 }}>
        <div
          className={`flex items-center gap-1 px-2 py-0.5 hover:bg-accent rounded cursor-pointer group`}
          onClick={handleClick}
        >
          {/* Expand/collapse arrow */}
          {hasChildren && !isColumn && (
            <svg
              className={`w-3 h-3 flex-shrink-0 text-muted-foreground transition-transform ${isExpanded ? 'rotate-90' : ''}`}
              fill="none"
              stroke="currentColor"
              viewBox="0 0 24 24"
              onClick={(e) => {
                e.stopPropagation();
                toggleNode(node.id);
              }}
            >
              <path strokeLinecap="round" strokeLinejoin="round" strokeWidth={2} d="M9 5l7 7-7 7" />
            </svg>
          )}
          {!hasChildren && !isColumn && <div className="w-3" />}

          {/* Icon */}
          {getIcon()}

          {/* Name */}
          <Tooltip>
            <TooltipTrigger asChild>
              <span className={`${isTable || isColumn ? 'font-mono text-xs' : ''} text-card-foreground whitespace-nowrap`}>
                {node.name}
              </span>
            </TooltipTrigger>
            <TooltipContent>
              {node.metadata?.description || node.name}
              {isColumn && node.metadata?.data_type && ` (${node.metadata.data_type})`}
            </TooltipContent>
          </Tooltip>

          {/* Column type */}
          {isColumn && node.metadata?.data_type && (
            <span className="text-muted-foreground text-xs whitespace-nowrap">{node.metadata.data_type}</span>
          )}

          {/* Child count */}
          {hasChildren && !isTable && (
            <span className="text-xs text-muted-foreground flex-shrink-0 whitespace-nowrap">
              ({node.children.length})
            </span>
          )}

          {/* Table details button */}
          {isTable && onTableDetails && (
            <Tooltip>
              <TooltipTrigger asChild>
                <button
                  onClick={(e) => {
                    e.stopPropagation();
                    onTableDetails({ table_id: node.name, full_table_id: node.id, ...node.metadata });
                  }}
                  className="opacity-0 group-hover:opacity-100 p-0.5 hover:bg-muted rounded transition-opacity"
                  aria-label="View details"
                >
                  <svg className="w-3 h-3 text-muted-foreground" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                    <path strokeLinecap="round" strokeLinejoin="round" strokeWidth={2} d="M13 16h-1v-4h-1m1-4h.01M21 12a9 9 0 11-18 0 9 9 0 0118 0z" />
                  </svg>
                </button>
              </TooltipTrigger>
              <TooltipContent>View details</TooltipContent>
            </Tooltip>
          )}
        </div>

        {/* Children */}
        {hasChildren && isExpanded && (
          <div>
            {node.children.map(child => renderNode(child, depth + 1))}
          </div>
        )}
      </div>
    );
  };

  // Render search loading state
  if (searchLoading) {
    return (
      <div className="flex flex-col items-center justify-center py-8 gap-2">
        <Spinner size="md" className="text-primary" />
        <p className="text-xs text-muted-foreground">Searching tables...</p>
      </div>
    );
  }

  // Render flat search results when actively searching
  if (useSemanticSearch && searchQuery && searchQuery.length >= 3 && semanticResults.length > 0) {
    return (
      <div className="text-xs">
        <div className="px-2 py-2 border-b border-border bg-accent/50 flex items-center justify-between sticky top-0">
          <span className="text-xs text-muted-foreground">
            Found {semanticResults.length} table{semanticResults.length !== 1 ? 's' : ''}
          </span>
        </div>

        <div className="divide-y divide-border">
          {semanticResults.map((result, idx) => {
            const fullTableId = `${result.project_id}.${result.dataset_id}.${result.table_name}`;
            const relevancePercent = Math.round((result.score || 0) * 100);

            return (
              <div
                key={idx}
                className="px-2 py-2 hover:bg-accent cursor-pointer transition-colors"
                onClick={() => onTableClick(fullTableId)}
              >
                <div className="flex items-center gap-2 mb-1">
                  <svg className="w-3 h-3 flex-shrink-0 text-primary" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                    <path strokeLinecap="round" strokeLinejoin="round" strokeWidth={2} d="M3 10h18M3 14h18m-9-4v8m-7 0h14a2 2 0 002-2V8a2 2 0 00-2-2H5a2 2 0 00-2 2v8a2 2 0 002 2z" />
                  </svg>
                  <span className="font-medium text-foreground font-mono">{result.table_name}</span>
                  {relevancePercent > 0 && (
                    <span className="text-xs text-muted-foreground">({relevancePercent}%)</span>
                  )}
                </div>
                <div className="text-xs text-muted-foreground ml-5 mb-1">
                  {result.project_id}.{result.dataset_id}
                </div>
                {result.description && (
                  <div className="text-xs text-muted-foreground ml-5 line-clamp-2">
                    {result.description}
                  </div>
                )}
              </div>
            );
          })}
        </div>
      </div>
    );
  }

  // Show empty search state
  if (useSemanticSearch && searchQuery && searchQuery.length >= 3 && semanticResults.length === 0) {
    return (
      <div className="flex flex-col items-center justify-center py-8 px-4 text-center">
        <svg className="w-12 h-12 text-muted-foreground mb-2" fill="none" stroke="currentColor" viewBox="0 0 24 24">
          <path strokeLinecap="round" strokeLinejoin="round" strokeWidth={1.5} d="M21 21l-6-6m2-5a7 7 0 11-14 0 7 7 0 0114 0z" />
        </svg>
        <p className="text-sm text-muted-foreground">No tables found</p>
        <p className="text-xs text-muted-foreground mt-1">Try a different search term</p>
      </div>
    );
  }

  // Loading state
  if (loading) {
    return (
      <div className="flex items-center justify-center py-8">
        <Spinner size="md" className="text-primary" />
      </div>
    );
  }

  // Error state
  if (error) {
    return (
      <div className="flex flex-col items-center justify-center py-8 px-4 text-center">
        <svg className="w-12 h-12 text-error-foreground mb-2" fill="none" stroke="currentColor" viewBox="0 0 24 24">
          <path strokeLinecap="round" strokeLinejoin="round" strokeWidth={1.5} d="M12 9v2m0 4h.01m-6.938 4h13.856c1.54 0 2.502-1.667 1.732-3L13.732 4c-.77-1.333-2.694-1.333-3.464 0L3.34 16c-.77 1.333.192 3 1.732 3z" />
        </svg>
        <p className="text-sm text-error-foreground">Failed to load catalog</p>
        <p className="text-xs text-muted-foreground mt-1">{error}</p>
      </div>
    );
  }

  // No datasource selected
  if (!datasourceId) {
    return (
      <div className="flex flex-col items-center justify-center py-8 px-4 text-center">
        <svg className="w-12 h-12 text-muted-foreground mb-2" fill="none" stroke="currentColor" viewBox="0 0 24 24">
          <path strokeLinecap="round" strokeLinejoin="round" strokeWidth={1.5} d="M4 7v10c0 2.21 3.582 4 8 4s8-1.79 8-4V7M4 7c0 2.21 3.582 4 8 4s8-1.79 8-4M4 7c0-2.21 3.582-4 8-4s8 1.79 8 4" />
        </svg>
        <p className="text-sm text-muted-foreground">Select a datasource</p>
        <p className="text-xs text-muted-foreground mt-1">Choose a datasource to browse its catalog</p>
      </div>
    );
  }

  // Empty catalog
  if (tree.length === 0) {
    return (
      <div className="flex flex-col items-center justify-center py-8 px-4 text-center">
        <svg className="w-12 h-12 text-muted-foreground mb-2" fill="none" stroke="currentColor" viewBox="0 0 24 24">
          <path strokeLinecap="round" strokeLinejoin="round" strokeWidth={1.5} d="M20 13V6a2 2 0 00-2-2H6a2 2 0 00-2 2v7m16 0v5a2 2 0 01-2 2H6a2 2 0 01-2-2v-5m16 0h-2.586a1 1 0 00-.707.293l-2.414 2.414a1 1 0 01-.707.293h-3.172a1 1 0 01-.707-.293l-2.414-2.414A1 1 0 006.586 13H4" />
        </svg>
        <p className="text-sm text-muted-foreground">No tables indexed</p>
        <p className="text-xs text-muted-foreground mt-1">Index the catalog in datasource settings</p>
      </div>
    );
  }

  // Regular tree view
  return (
    <div className="text-xs">
      {tableCount > 0 && (
        <div className="px-2 py-1 text-xs text-muted-foreground border-b border-border">
          {tableCount} table{tableCount !== 1 ? 's' : ''} indexed
        </div>
      )}
      {tree.map(node => renderNode(node, 0))}
    </div>
  );
};

export default DatasourceCatalogTree;
