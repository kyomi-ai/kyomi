// SPDX-License-Identifier: AGPL-3.0-or-later
import { useState, useEffect, useRef, useCallback, useMemo } from 'react';
import { useQuery, useMutation, useQueryClient } from '@tanstack/react-query';
import { useAuth } from '../context/AuthContext';
import { useNavigate, useSearchParams } from 'react-router-dom';
import { Card, CardHeader, CardTitle, CardContent, CardFooter } from '../components/ui/card';
import { Button } from '../components/ui/button';
import { Tooltip, TooltipTrigger, TooltipContent } from '../components/ui/tooltip';
import { Spinner } from '../components/ui/spinner';
import { Select, SelectTrigger, SelectValue, SelectContent, SelectItem } from '../components/ui/select';
import { useCapabilities } from '../context/CapabilitiesContext';
import { useWebSocket } from '../context/WebSocketContext';
import ConfirmDialog from '../components/ConfirmDialog';
import useConfirm from '../hooks/useConfirm';
import { toast } from '../lib/toast';
import useDatasources from '../hooks/useDatasources';
import NoDatasourcesEmptyState from '../components/NoDatasourcesEmptyState';

// Custom hook for debouncing a value
function useDebounce(value, delay) {
  const [debouncedValue, setDebouncedValue] = useState(value);

  useEffect(() => {
    const handler = setTimeout(() => {
      setDebouncedValue(value);
    }, delay);

    return () => clearTimeout(handler);
  }, [value, delay]);

  return debouncedValue;
}

// Hook to detect mobile screen size
function useIsMobile() {
  const [isMobile, setIsMobile] = useState(() =>
    typeof window !== 'undefined' && window.innerWidth < 768
  );

  useEffect(() => {
    const checkMobile = () => setIsMobile(window.innerWidth < 768);
    window.addEventListener('resize', checkMobile);
    return () => window.removeEventListener('resize', checkMobile);
  }, []);

  return isMobile;
}

export default function DashboardsList() {
  const { apiClient } = useAuth();
  const navigate = useNavigate();
  const queryClient = useQueryClient();
  const [searchParams, setSearchParams] = useSearchParams();
  const capabilities = useCapabilities();
  const { isOpen, dialogProps, confirm } = useConfirm();
  const { subscribe } = useWebSocket();
  const isMobile = useIsMobile();

  // Refetch dashboards when an async summary is generated
  useEffect(() => {
    const unsubscribe = subscribe('dashboard_update', (message) => {
      if (message.data?.context_type !== 'dashboard_summary') return;
      queryClient.invalidateQueries(['dashboards']);
    });
    return unsubscribe;
  }, [subscribe, queryClient]);

  // Datasources check for empty state
  const { hasDatasources, loading: datasourcesLoading } = useDatasources();

  // Search and sort state
  const [searchQuery, setSearchQuery] = useState('');
  const [sortBy, setSortBy] = useState('recent');
  const debouncedSearchQuery = useDebounce(searchQuery, 600);
  const searchInputRef = useRef(null);

  // State for collection management
  const [showCollectionModal, setShowCollectionModal] = useState(false);
  const [editingCollection, setEditingCollection] = useState(null);
  const [collectionFormData, setCollectionFormData] = useState({
    name: '',
    description: '',
    color: '#d97706',
    is_public: false
  });
  const [showAddToCollectionModal, setShowAddToCollectionModal] = useState(false);
  const [selectedDashboard, setSelectedDashboard] = useState(null);

  // Collections sidebar state - closed by default
  const [collectionsOpen, setCollectionsOpen] = useState(false);

  // Resize state for collections sidebar
  const [sidebarWidth, setSidebarWidth] = useState(320); // Default 320px (w-80)
  const [isResizing, setIsResizing] = useState(false);
  const resizeStartX = useRef(0);
  const resizeStartWidth = useRef(320);

  // Resize handlers
  const handleResizeStart = useCallback((e) => {
    resizeStartX.current = e.clientX;
    resizeStartWidth.current = sidebarWidth;
    setIsResizing(true);
    e.preventDefault();
  }, [sidebarWidth]);

  useEffect(() => {
    if (!isResizing) return;

    const handleMouseMove = (e) => {
      // Moving left increases width (sidebar is on right)
      const diff = resizeStartX.current - e.clientX;
      const newWidth = resizeStartWidth.current + diff;
      // Clamp between 280px and 480px
      setSidebarWidth(Math.max(280, Math.min(newWidth, 480)));
    };

    const handleMouseUp = () => {
      setIsResizing(false);
      document.body.style.cursor = '';
      document.body.style.userSelect = '';
    };

    document.body.style.cursor = 'col-resize';
    document.body.style.userSelect = 'none';

    document.addEventListener('mousemove', handleMouseMove);
    document.addEventListener('mouseup', handleMouseUp);

    return () => {
      document.removeEventListener('mousemove', handleMouseMove);
      document.removeEventListener('mouseup', handleMouseUp);
      document.body.style.cursor = '';
      document.body.style.userSelect = '';
    };
  }, [isResizing]);

  // Get active collection filter from URL
  const activeCollectionId = searchParams.get('collection');

  // Fetch dashboards with search and sort
  const { data: dashboards, isLoading: dashboardsLoading, isFetching: dashboardsFetching, refetch: refetchDashboards } = useQuery({
    queryKey: ['dashboards', debouncedSearchQuery, sortBy],
    queryFn: async () => {
      const params = new URLSearchParams();
      if (debouncedSearchQuery) {
        params.append('query', debouncedSearchQuery);
      }
      params.append('sort_by', sortBy);

      const response = await apiClient.get(`/api/v1/dashboards?${params.toString()}`);
      return response.data;
    },
    // Keep previous data visible while fetching new search results
    placeholderData: (previousData) => previousData,
  });

  // Restore focus to search input after query completes (if it had focus)
  const previousLoadingRef = useRef(dashboardsLoading);
  useEffect(() => {
    // If we just finished loading and the search input exists and had focus
    if (previousLoadingRef.current && !dashboardsLoading && searchInputRef.current) {
      const activeElement = document.activeElement;
      // Only refocus if focus was lost (not on the input or clear button)
      if (activeElement !== searchInputRef.current && !activeElement?.closest('.search-container')) {
        searchInputRef.current.focus();
      }
    }
    previousLoadingRef.current = dashboardsLoading;
  }, [dashboardsLoading]);

  // Fetch collections
  const { data: collections, isLoading: collectionsLoading } = useQuery({
    queryKey: ['collections'],
    queryFn: async () => {
      const response = await apiClient.get('/api/v1/collections');
      return response.data;
    },
  });

  // Get dashboard IDs for the active collection
  const activeCollectionDashboardIds = activeCollectionId && collections
    ? new Set(
        collections
          .find(c => c.id === activeCollectionId)
          ?.dashboards?.map(d => d.dashboard_id) || []
      )
    : null;

  // Filter dashboards based on active collection
  const filteredDashboards = dashboards?.filter(dashboard => {
    if (!activeCollectionId) return true; // Show all if no filter
    return activeCollectionDashboardIds?.has(dashboard.dashboard_id);
  });

  // Get collection membership for each dashboard
  const getDashboardCollections = (dashboardId) => {
    if (!collections) return [];
    return collections.filter(collection =>
      collection.dashboards?.some(d => d.dashboard_id === dashboardId)
    );
  };

  // Check if dashboard is public (in at least one public collection)
  const isDashboardPublic = (dashboardId) => {
    const dashboardCollections = getDashboardCollections(dashboardId);
    return dashboardCollections.some(c => c.is_public);
  };

  // Create collection mutation
  const createCollectionMutation = useMutation({
    mutationFn: async (data) => {
      await apiClient.post('/api/v1/collections', data);
    },
    onSuccess: () => {
      queryClient.invalidateQueries(['collections']);
      setShowCollectionModal(false);
      setCollectionFormData({ name: '', description: '', color: '#d97706' });
    },
  });

  // Update collection mutation
  const updateCollectionMutation = useMutation({
    mutationFn: async ({ id, data }) => {
      await apiClient.patch(`/api/v1/collections/${id}`, data);
    },
    onSuccess: () => {
      queryClient.invalidateQueries(['collections']);
      setEditingCollection(null);
      setShowCollectionModal(false);
      setCollectionFormData({ name: '', description: '', color: '#d97706' });
    },
  });

  // Delete collection mutation
  const deleteCollectionMutation = useMutation({
    mutationFn: async (id) => {
      await apiClient.delete(`/api/v1/collections/${id}`);
    },
    onSuccess: () => {
      queryClient.invalidateQueries(['collections']);
      // If deleted collection was active, clear filter
      if (activeCollectionId === deleteCollectionMutation.variables) {
        setSearchParams({});
      }
    },
  });

  // Add dashboard to collection mutation
  const addToCollectionMutation = useMutation({
    mutationFn: async ({ collectionId, dashboardId }) => {
      await apiClient.post(`/api/v1/collections/${collectionId}/dashboards`, {
        dashboard_id: dashboardId,
      });
    },
    onSuccess: () => {
      queryClient.invalidateQueries(['collections']);
      setShowAddToCollectionModal(false);
      setSelectedDashboard(null);
    },
  });

  // Remove dashboard from collection mutation
  const removeFromCollectionMutation = useMutation({
    mutationFn: async ({ collectionId, dashboardId }) => {
      await apiClient.delete(`/api/v1/collections/${collectionId}/dashboards/${dashboardId}`);
    },
    onSuccess: () => {
      queryClient.invalidateQueries(['collections']);
    },
  });

  const handleDeleteDashboard = async (dashboardId, title) => {
    const confirmed = await confirm({
      title: 'Delete Dashboard?',
      message: `Are you sure you want to delete "${title}"? This action cannot be undone.`,
      confirmText: 'Delete',
      variant: 'destructive'
    });

    if (!confirmed) {
      return;
    }

    try {
      await apiClient.delete(`/api/v1/dashboards/${dashboardId}`);
      refetchDashboards();
      // Invalidate default-dashboard query in case the deleted dashboard was the default
      queryClient.invalidateQueries(['default-dashboard']);
    } catch (err) {
      toast.error('Failed to delete dashboard. Please try again.');
    }
  };

  const handleCreateCollection = () => {
    setCollectionFormData({ name: '', description: '', color: '#d97706', is_public: false });
    setEditingCollection(null);
    setShowCollectionModal(true);
  };

  const handleEditCollection = (collection) => {
    setCollectionFormData({
      name: collection.name,
      description: collection.description || '',
      color: collection.color || '#d97706',
      is_public: collection.is_public || false,
    });
    setEditingCollection(collection);
    setShowCollectionModal(true);
  };

  const handleDeleteCollection = async (collection) => {
    const confirmed = await confirm({
      title: 'Delete Collection?',
      message: `Are you sure you want to delete "${collection.name}"? Dashboards will not be deleted.`,
      confirmText: 'Delete Collection',
      variant: 'destructive'
    });

    if (!confirmed) {
      return;
    }
    deleteCollectionMutation.mutate(collection.id);
  };

  const handleCollectionSubmit = (e) => {
    e.preventDefault();
    if (editingCollection) {
      updateCollectionMutation.mutate({ id: editingCollection.id, data: collectionFormData });
    } else {
      createCollectionMutation.mutate(collectionFormData);
    }
  };

  const handleAddToCollection = (dashboard) => {
    setSelectedDashboard(dashboard);
    setShowAddToCollectionModal(true);
  };

  const handleRemoveFromCollection = async (collectionId, dashboardId, collectionName) => {
    const confirmed = await confirm({
      title: 'Remove from Collection?',
      message: `Remove this dashboard from "${collectionName}"?`,
      confirmText: 'Remove',
      variant: 'destructive'
    });

    if (!confirmed) {
      return;
    }
    removeFromCollectionMutation.mutate({ collectionId, dashboardId });
  };

  const handleCollectionClick = (collectionId) => {
    if (activeCollectionId === collectionId) {
      // If clicking active collection, clear filter
      setSearchParams({});
    } else {
      setSearchParams({ collection: collectionId });
    }
  };

  // Get available collections for a dashboard (not already in)
  const getAvailableCollections = (dashboardId) => {
    if (!collections) return [];
    const dashboardCollections = getDashboardCollections(dashboardId);
    const dashboardCollectionIds = new Set(dashboardCollections.map(c => c.id));
    return collections.filter(c => !dashboardCollectionIds.has(c.id));
  };

  const activeCollection = collections?.find(c => c.id === activeCollectionId);

  // Check if user has reached dashboard limit
  const maxDashboards = capabilities.max_dashboards || 0;
  const atDashboardLimit = maxDashboards > 0 && dashboards && dashboards.length >= maxDashboards;

  // Show empty state if no datasources are configured
  if (!datasourcesLoading && !hasDatasources) {
    return <NoDatasourcesEmptyState context="dashboards" />;
  }

  // Only show full-page spinner on true initial load (no data yet).
  // During search/sort changes, keep the existing content visible (placeholderData handles this).
  if ((dashboardsLoading && !dashboards) || collectionsLoading) {
    return (
      <div className="flex h-full items-center justify-center bg-muted">
        <img src="/kyomi_animated_logo.svg" alt="Loading" className="w-12 h-12" />
      </div>
    );
  }

  return (
    <div className="flex flex-col h-full bg-muted" style={{flexDirection: 'column'}}>
      {/* Header */}
      <div className="min-h-16 border-b border-border bg-card px-6 py-3 flex-shrink-0 flex flex-col sm:flex-row sm:items-center gap-3">
        <h1 className="text-2xl font-semibold text-foreground flex-shrink-0">
          {activeCollection ? activeCollection.name : 'All Dashboards'}
        </h1>

        {/* Search and Sort Controls */}
        <div className="flex-1 flex items-center gap-3 justify-start sm:justify-center">
          {/* Search Input */}
          <div className="relative flex-1 max-w-md search-container">
            <svg
              className="absolute left-3 top-1/2 -translate-y-1/2 w-4 h-4 text-muted-foreground"
              fill="none"
              stroke="currentColor"
              viewBox="0 0 24 24"
            >
              <path
                strokeLinecap="round"
                strokeLinejoin="round"
                strokeWidth={2}
                d="M21 21l-6-6m2-5a7 7 0 11-14 0 7 7 0 0114 0z"
              />
            </svg>
            <input
              ref={searchInputRef}
              type="text"
              placeholder="Search dashboards..."
              value={searchQuery}
              onChange={(e) => setSearchQuery(e.target.value)}
              className="w-full pl-10 pr-4 py-2 text-sm border border-input rounded-lg focus:ring-2 focus:ring-primary/20 focus:border-primary bg-card text-foreground transition-colors"
            />
            {searchQuery && (
              <button
                onClick={() => setSearchQuery('')}
                className="absolute right-3 top-1/2 -translate-y-1/2 text-muted-foreground hover:text-foreground"
                aria-label="Clear search"
              >
                <svg className="w-4 h-4" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                  <path strokeLinecap="round" strokeLinejoin="round" strokeWidth={2} d="M6 18L18 6M6 6l12 12" />
                </svg>
              </button>
            )}
          </div>

          {/* Sort Dropdown */}
          <Select value={sortBy} onValueChange={setSortBy}>
            <SelectTrigger className="w-[160px] h-9 text-sm">
              <SelectValue placeholder="Sort by" />
            </SelectTrigger>
            <SelectContent>
              <SelectItem value="recent">Recently Updated</SelectItem>
              <SelectItem value="popularity">Most Popular</SelectItem>
              <SelectItem value="created">Newest First</SelectItem>
              <SelectItem value="title">Alphabetical</SelectItem>
            </SelectContent>
          </Select>
        </div>

        <div className="flex items-center gap-3 flex-shrink-0">
          {/* Collections toggle button */}
          <Tooltip>
            <TooltipTrigger asChild>
              <button
                onClick={() => setCollectionsOpen(!collectionsOpen)}
                className={`flex items-center gap-2 px-2 md:px-4 py-2 text-sm font-medium rounded-lg transition-colors ${
                  collectionsOpen
                    ? 'bg-primary/10 text-primary'
                    : 'bg-accent text-foreground hover:bg-accent/80'
                }`}
                aria-label="Toggle Collections"
              >
                <svg className="w-4 h-4 flex-shrink-0" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                  <path strokeLinecap="round" strokeLinejoin="round" strokeWidth={2} d="M19 11H5m14 0a2 2 0 012 2v6a2 2 0 01-2 2H5a2 2 0 01-2-2v-6a2 2 0 012-2m14 0V9a2 2 0 00-2-2M5 11V9a2 2 0 012-2m0 0V5a2 2 0 012-2h6a2 2 0 012 2v2M7 7h10" />
                </svg>
                <span className="hidden sm:inline">Collections</span>
              </button>
            </TooltipTrigger>
            <TooltipContent>Organize dashboards into collections</TooltipContent>
          </Tooltip>

          <Tooltip>
            <TooltipTrigger asChild>
              <button
                onClick={() => !atDashboardLimit && navigate('/dashboard/new/edit')}
                disabled={atDashboardLimit}
                className={`flex items-center gap-2 px-3 md:px-4 py-2 text-sm font-medium rounded-lg transition-colors ${
                  atDashboardLimit
                    ? 'text-muted-foreground bg-muted cursor-not-allowed'
                    : 'text-white bg-primary hover:bg-primary/90'
                }`}
              >
                <svg className="w-4 h-4 flex-shrink-0" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                  <path strokeLinecap="round" strokeLinejoin="round" strokeWidth={2} d="M12 4v16m8-8H4" />
                </svg>
                <span className="hidden sm:inline whitespace-nowrap">Create Dashboard</span>
              </button>
            </TooltipTrigger>
            {atDashboardLimit && (
              <TooltipContent>
                Free tier is limited to {maxDashboards} dashboards. Upgrade to create more.
              </TooltipContent>
            )}
          </Tooltip>
        </div>
      </div>

      {/* Content Area */}
      <div className="flex flex-1 min-h-0">
        {/* Main Content - Dashboards Grid */}
        <div className="flex-1 overflow-y-auto @container">
          <div className="p-4 md:p-6">

          {!filteredDashboards || filteredDashboards.length === 0 ? (
            <div className="text-center py-16 bg-card rounded-2xl shadow-sm border border-border">
              <div className="max-w-md mx-auto">
                <svg className="w-24 h-24 mx-auto text-muted-foreground/50 mb-6" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                  <path strokeLinecap="round" strokeLinejoin="round" strokeWidth={1.5} d="M9 17v-2m3 2v-4m3 4v-6m2 10H7a2 2 0 01-2-2V5a2 2 0 012-2h5.586a1 1 0 01.707.293l5.414 5.414a1 1 0 01.293.707V19a2 2 0 01-2 2z" />
                </svg>
                <h3 className="text-xl font-semibold text-foreground mb-2">
                  {debouncedSearchQuery
                    ? 'No matching dashboards'
                    : activeCollection
                      ? 'No dashboards in this collection'
                      : 'No dashboards yet'
                  }
                </h3>
                <p className="text-muted-foreground mb-6">
                  {debouncedSearchQuery
                    ? `No dashboards found for "${debouncedSearchQuery}". Try a different search term.`
                    : activeCollection
                      ? 'Add dashboards to this collection using the + icon on dashboard cards'
                      : 'Get started by creating your first markdown dashboard with embedded charts'
                  }
                </p>
                {!activeCollection && !debouncedSearchQuery && (
                  <Tooltip>
                    <TooltipTrigger asChild>
                      <button
                        onClick={() => !atDashboardLimit && navigate('/dashboard/new/edit')}
                        disabled={atDashboardLimit}
                        className={`inline-flex items-center gap-2 px-4 py-2 text-sm font-medium rounded-lg transition-colors ${
                          atDashboardLimit
                            ? 'text-muted-foreground bg-muted cursor-not-allowed'
                            : 'text-white bg-primary hover:bg-primary/90'
                        }`}
                      >
                        <svg className="w-4 h-4" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                          <path strokeLinecap="round" strokeLinejoin="round" strokeWidth={2} d="M12 4v16m8-8H4" />
                        </svg>
                        Create Your First Dashboard
                      </button>
                    </TooltipTrigger>
                    {atDashboardLimit && (
                      <TooltipContent>
                        Free tier is limited to {maxDashboards} dashboards. Upgrade to create more.
                      </TooltipContent>
                    )}
                  </Tooltip>
                )}
              </div>
            </div>
          ) : (
            <div className="w-full grid gap-6 @lg:grid-cols-2 @3xl:grid-cols-3 @5xl:grid-cols-4">
              {filteredDashboards.map((dashboard) => {
                const dashboardCollections = getDashboardCollections(dashboard.dashboard_id);
                const availableCollections = getAvailableCollections(dashboard.dashboard_id);

                return (
                  <Card
                    key={dashboard.dashboard_id}
                    className="hover:border-primary/30 transition-colors duration-200 flex flex-col"
                  >
                    <CardHeader>
                      <div className="flex items-start justify-between">
                        <CardTitle className="text-xl flex-1 pr-2 line-clamp-2">
                          {dashboard.title}
                        </CardTitle>
                        <div className="flex gap-1">
                          {/* Add to Collection */}
                          {availableCollections.length > 0 && (
                            <Tooltip>
                              <TooltipTrigger asChild>
                                <button
                                  onClick={() => handleAddToCollection(dashboard)}
                                  className="flex-shrink-0 p-2 text-muted-foreground hover:text-success-foreground hover:bg-success/10 rounded-lg transition-colors"
                                  aria-label="Add to collection"
                                >
                                  <svg className="w-5 h-5" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                                    <path strokeLinecap="round" strokeLinejoin="round" strokeWidth={2} d="M12 4v16m8-8H4" />
                                  </svg>
                                </button>
                              </TooltipTrigger>
                              <TooltipContent>Add to collection</TooltipContent>
                            </Tooltip>
                          )}
                          {/* Delete Dashboard */}
                          <Tooltip>
                            <TooltipTrigger asChild>
                              <button
                                onClick={() => handleDeleteDashboard(dashboard.dashboard_id, dashboard.title)}
                                className="flex-shrink-0 p-2 text-muted-foreground hover:text-error-foreground hover:bg-error/10 rounded-lg transition-colors"
                                aria-label="Delete dashboard"
                              >
                                <svg className="w-5 h-5" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                                  <path strokeLinecap="round" strokeLinejoin="round" strokeWidth={2} d="M19 7l-.867 12.142A2 2 0 0116.138 21H7.862a2 2 0 01-1.995-1.858L5 7m5 4v6m4-6v6m1-10V4a1 1 0 00-1-1h-4a1 1 0 00-1 1v3M4 7h16" />
                                </svg>
                              </button>
                            </TooltipTrigger>
                            <TooltipContent>Delete dashboard</TooltipContent>
                          </Tooltip>
                        </div>
                      </div>
                    </CardHeader>

                    <CardContent className="flex-1 flex flex-col">
                      {/* AI-generated summary */}
                      {dashboard.summary && (
                        <p className="text-sm text-muted-foreground mb-3 line-clamp-4">{dashboard.summary}</p>
                      )}

                      {/* Collection Badges */}
                      {dashboardCollections.length > 0 && (
                        <div className="flex flex-wrap gap-2 mb-4">
                          {dashboardCollections.map((collection) => (
                            <div
                              key={collection.id}
                              className="group relative inline-flex items-center gap-1.5 px-2 py-1 rounded-full text-xs font-medium cursor-pointer hover:opacity-80 transition-opacity"
                              style={{
                                backgroundColor: `${collection.color}20`,
                                color: collection.color
                              }}
                              onClick={() => handleCollectionClick(collection.id)}
                            >
                              <div
                                className="w-2 h-2 rounded-full"
                                style={{ backgroundColor: collection.color }}
                              />
                              {collection.name}
                              <Tooltip>
                                <TooltipTrigger asChild>
                                  <button
                                    onClick={(e) => {
                                      e.stopPropagation();
                                      handleRemoveFromCollection(collection.id, dashboard.dashboard_id, collection.name);
                                    }}
                                    className="ml-1 hover:bg-foreground/10 rounded-full p-0.5"
                                    aria-label="Remove from collection"
                                  >
                                    <svg className="w-3 h-3" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                                      <path strokeLinecap="round" strokeLinejoin="round" strokeWidth={2} d="M6 18L18 6M6 6l12 12" />
                                    </svg>
                                  </button>
                                </TooltipTrigger>
                                <TooltipContent>Remove from collection</TooltipContent>
                              </Tooltip>
                            </div>
                          ))}
                        </div>
                      )}

                      <div className="flex flex-wrap items-center gap-2 text-xs text-muted-foreground mt-auto">
                        <div className="flex items-center gap-1">
                          <svg className="w-4 h-4 flex-shrink-0" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                            <path strokeLinecap="round" strokeLinejoin="round" strokeWidth={2} d="M12 8v4l3 3m6-3a9 9 0 11-18 0 9 9 0 0118 0z" />
                          </svg>
                          <span className="whitespace-nowrap">{new Date(dashboard.updated_at).toLocaleDateString()}</span>
                        </div>
                        <div className="flex items-center gap-1">
                          <svg className="w-4 h-4 flex-shrink-0" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                            <path strokeLinecap="round" strokeLinejoin="round" strokeWidth={2} d="M15 12a3 3 0 11-6 0 3 3 0 016 0z" />
                            <path strokeLinecap="round" strokeLinejoin="round" strokeWidth={2} d="M2.458 12C3.732 7.943 7.523 5 12 5c4.478 0 8.268 2.943 9.542 7-1.274 4.057-5.064 7-9.542 7-4.477 0-8.268-2.943-9.542-7z" />
                          </svg>
                          <span className="whitespace-nowrap">{dashboard.view_count || 0}</span>
                        </div>
                        {isDashboardPublic(dashboard.dashboard_id) ? (
                          <div className="flex items-center gap-1 px-2 py-1 rounded-md bg-success/10 text-success-foreground">
                            <svg className="w-3 h-3" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                              <path strokeLinecap="round" strokeLinejoin="round" strokeWidth={2} d="M3.055 11H5a2 2 0 012 2v1a2 2 0 002 2 2 2 0 012 2v2.945M8 3.935V5.5A2.5 2.5 0 0010.5 8h.5a2 2 0 012 2 2 2 0 104 0 2 2 0 012-2h1.064M15 20.488V18a2 2 0 012-2h3.064M21 12a9 9 0 11-18 0 9 9 0 0118 0z" />
                            </svg>
                            <span className="font-medium">Public</span>
                          </div>
                        ) : (
                          <div className="flex items-center gap-1 px-2 py-1 rounded-md bg-muted text-muted-foreground">
                            <svg className="w-3 h-3" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                              <path strokeLinecap="round" strokeLinejoin="round" strokeWidth={2} d="M12 15v2m-6 4h12a2 2 0 002-2v-6a2 2 0 00-2-2H6a2 2 0 00-2 2v6a2 2 0 002 2zm10-10V7a4 4 0 00-8 0v4h8z" />
                            </svg>
                            <span className="font-medium">Private</span>
                          </div>
                        )}
                      </div>
                    </CardContent>

                    <CardFooter className="flex gap-2 pt-0">
                      <Button
                        variant="default"
                        onClick={() => navigate(`/dashboard/${dashboard.dashboard_id}`)}
                        className="flex-1"
                      >
                          <svg className="w-4 h-4" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                            <path strokeLinecap="round" strokeLinejoin="round" strokeWidth={2} d="M15 12a3 3 0 11-6 0 3 3 0 016 0z" />
                            <path strokeLinecap="round" strokeLinejoin="round" strokeWidth={2} d="M2.458 12C3.732 7.943 7.523 5 12 5c4.478 0 8.268 2.943 9.542 7-1.274 4.057-5.064 7-9.542 7-4.477 0-8.268-2.943-9.542-7z" />
                          </svg>
                        View
                      </Button>
                      <Button
                        variant="outline"
                        onClick={() => navigate(`/dashboard/${dashboard.dashboard_id}/edit`)}
                        className="flex-1"
                      >
                        <svg className="w-4 h-4" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                          <path strokeLinecap="round" strokeLinejoin="round" strokeWidth={2} d="M11 5H6a2 2 0 00-2 2v11a2 2 0 002 2h11a2 2 0 002-2v-5m-1.414-9.414a2 2 0 112.828 2.828L11.828 15H9v-2.828l8.586-8.586z" />
                        </svg>
                        Edit
                      </Button>
                    </CardFooter>
                  </Card>
                );
              })}
            </div>
          )}
          </div>
        </div>

        {/* Right Sidebar - Collections */}
        {collectionsOpen && (
          isMobile ? (
            // Mobile: Fixed overlay with backdrop
            // 64px main header + 64px page header = 128px
            <>
              <div
                className="fixed top-32 left-0 right-0 bottom-0 bg-black/50 z-40"
                onClick={() => setCollectionsOpen(false)}
              />
              <div className="fixed top-32 right-0 bottom-0 w-80 max-w-[85vw] z-50 bg-card flex flex-col shadow-xl">
                {/* Sidebar Header */}
                <div className="p-4 border-b border-border flex items-center justify-between flex-shrink-0">
            <h3 className="font-semibold text-foreground">Collections</h3>
            <Tooltip>
              <TooltipTrigger asChild>
                <button
                  onClick={() => setCollectionsOpen(false)}
                  className="p-1 text-muted-foreground hover:text-foreground rounded transition-colors"
                  aria-label="Close"
                >
                  <svg className="w-5 h-5" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                    <path strokeLinecap="round" strokeLinejoin="round" strokeWidth={2} d="M6 18L18 6M6 6l12 12" />
                  </svg>
                </button>
              </TooltipTrigger>
              <TooltipContent>Close</TooltipContent>
            </Tooltip>
          </div>

          <div className="flex-shrink-0 p-4 border-b border-border">
            <button
              onClick={handleCreateCollection}
              className="w-full flex items-center justify-center gap-2 px-4 py-2 text-sm font-medium bg-primary text-white rounded-lg hover:bg-primary/90 transition-colors"
            >
              <svg className="w-4 h-4" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                <path strokeLinecap="round" strokeLinejoin="round" strokeWidth={2} d="M12 4v16m8-8H4" />
              </svg>
              New Collection
            </button>
          </div>

          <div className="flex-1 overflow-y-auto">
            {/* All Dashboards */}
            <button
              onClick={() => setSearchParams({})}
              className={`w-full flex items-center gap-3 px-4 py-3 text-left transition-colors ${
                !activeCollectionId
                  ? 'bg-warning text-foreground font-medium'
                  : 'text-foreground hover:bg-accent'
              }`}
            >
              <svg className="w-5 h-5" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                <path strokeLinecap="round" strokeLinejoin="round" strokeWidth={2} d="M9 17V7m0 10a2 2 0 01-2 2H5a2 2 0 01-2-2V7a2 2 0 012-2h2a2 2 0 012 2m0 10a2 2 0 002 2h2a2 2 0 002-2M9 7a2 2 0 012-2h2a2 2 0 012 2m0 10V7m0 10a2 2 0 002 2h2a2 2 0 002-2V7a2 2 0 00-2-2h-2a2 2 0 00-2 2" />
              </svg>
              <span className="flex-1">All Dashboards</span>
              <span className="text-sm text-muted-foreground">{dashboards?.length || 0}</span>
            </button>

            {/* Collections List - Grouped by Public/Private */}
            {collections && collections.length > 0 && (
              <div className="py-2">
                {/* Public Collections Section */}
                {collections.filter(c => c.is_public).length > 0 && (
                  <>
                    <div className="px-4 py-2 text-xs font-semibold text-muted-foreground uppercase tracking-wider flex items-center gap-2">
                      <svg className="w-3 h-3" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                        <path strokeLinecap="round" strokeLinejoin="round" strokeWidth={2} d="M3.055 11H5a2 2 0 012 2v1a2 2 0 002 2 2 2 0 012 2v2.945M8 3.935V5.5A2.5 2.5 0 0010.5 8h.5a2 2 0 012 2 2 2 0 104 0 2 2 0 012-2h1.064M15 20.488V18a2 2 0 012-2h3.064M21 12a9 9 0 11-18 0 9 9 0 0118 0z" />
                      </svg>
                      Public Collections
                    </div>
                    {collections.filter(c => c.is_public).map((collection) => (
                  <div
                    key={collection.id}
                    className={`group relative ${
                      activeCollectionId === collection.id
                        ? 'bg-primary/10'
                        : 'hover:bg-accent'
                    }`}
                  >
                    <button
                      onClick={() => handleCollectionClick(collection.id)}
                      className={`w-full flex items-center gap-3 px-4 py-3 text-left transition-colors ${
                        activeCollectionId === collection.id
                          ? 'text-foreground font-medium'
                          : 'text-foreground'
                      }`}
                    >
                      <div
                        className="w-3 h-3 rounded-full flex-shrink-0"
                        style={{ backgroundColor: collection.color || '#d97706' }}
                      />
                      <span className="flex-1 truncate">{collection.name}</span>
                      <span className="text-sm text-muted-foreground">
                        {collection.dashboards?.length || 0}
                      </span>
                    </button>

                    {/* Quick Actions (visible on hover) */}
                    <div className="absolute right-2 top-1/2 -translate-y-1/2 opacity-0 group-hover:opacity-100 transition-opacity flex gap-1">
                      <Tooltip>
                        <TooltipTrigger asChild>
                          <button
                            onClick={(e) => {
                              e.stopPropagation();
                              handleEditCollection(collection);
                            }}
                            className="p-1 text-muted-foreground hover:text-primary hover:bg-primary/10 rounded"
                            aria-label="Edit collection"
                          >
                            <svg className="w-4 h-4" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                              <path strokeLinecap="round" strokeLinejoin="round" strokeWidth={2} d="M11 5H6a2 2 0 00-2 2v11a2 2 0 002 2h11a2 2 0 002-2v-5m-1.414-9.414a2 2 0 112.828 2.828L11.828 15H9v-2.828l8.586-8.586z" />
                            </svg>
                          </button>
                        </TooltipTrigger>
                        <TooltipContent>Edit collection</TooltipContent>
                      </Tooltip>
                      <Tooltip>
                        <TooltipTrigger asChild>
                          <button
                            onClick={(e) => {
                              e.stopPropagation();
                              handleDeleteCollection(collection);
                            }}
                            className="p-1 text-muted-foreground hover:text-error-foreground hover:bg-error/10 rounded"
                            aria-label="Delete collection"
                          >
                            <svg className="w-4 h-4" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                              <path strokeLinecap="round" strokeLinejoin="round" strokeWidth={2} d="M19 7l-.867 12.142A2 2 0 0116.138 21H7.862a2 2 0 01-1.995-1.858L5 7m5 4v6m4-6v6m1-10V4a1 1 0 00-1-1h-4a1 1 0 00-1 1v3M4 7h16" />
                            </svg>
                          </button>
                        </TooltipTrigger>
                        <TooltipContent>Delete collection</TooltipContent>
                      </Tooltip>
                    </div>
                  </div>
                    ))}
                  </>
                )}

                {/* Private Collections Section */}
                {collections.filter(c => !c.is_public).length > 0 && (
                  <>
                    <div className="px-4 py-2 text-xs font-semibold text-muted-foreground uppercase tracking-wider flex items-center gap-2 mt-2">
                      <svg className="w-3 h-3" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                        <path strokeLinecap="round" strokeLinejoin="round" strokeWidth={2} d="M12 15v2m-6 4h12a2 2 0 002-2v-6a2 2 0 00-2-2H6a2 2 0 00-2 2v6a2 2 0 002 2zm10-10V7a4 4 0 00-8 0v4h8z" />
                      </svg>
                      Private Collections
                    </div>
                    {collections.filter(c => !c.is_public).map((collection) => (
                  <div
                    key={collection.id}
                    className={`group relative ${
                      activeCollectionId === collection.id
                        ? 'bg-primary/10'
                        : 'hover:bg-accent'
                    }`}
                  >
                    <button
                      onClick={() => handleCollectionClick(collection.id)}
                      className={`w-full flex items-center gap-3 px-4 py-3 text-left transition-colors ${
                        activeCollectionId === collection.id
                          ? 'text-foreground font-medium'
                          : 'text-foreground'
                      }`}
                    >
                      <div
                        className="w-3 h-3 rounded-full flex-shrink-0"
                        style={{ backgroundColor: collection.color || '#d97706' }}
                      />
                      <span className="flex-1 truncate">{collection.name}</span>
                      <span className="text-sm text-muted-foreground">
                        {collection.dashboards?.length || 0}
                      </span>
                    </button>

                    {/* Quick Actions (visible on hover) */}
                    <div className="absolute right-2 top-1/2 -translate-y-1/2 opacity-0 group-hover:opacity-100 transition-opacity flex gap-1">
                      <Tooltip>
                        <TooltipTrigger asChild>
                          <button
                            onClick={(e) => {
                              e.stopPropagation();
                              handleEditCollection(collection);
                            }}
                            className="p-1 text-muted-foreground hover:text-primary hover:bg-primary/10 rounded"
                            aria-label="Edit collection"
                          >
                            <svg className="w-4 h-4" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                              <path strokeLinecap="round" strokeLinejoin="round" strokeWidth={2} d="M11 5H6a2 2 0 00-2 2v11a2 2 0 002 2h11a2 2 0 002-2v-5m-1.414-9.414a2 2 0 112.828 2.828L11.828 15H9v-2.828l8.586-8.586z" />
                            </svg>
                          </button>
                        </TooltipTrigger>
                        <TooltipContent>Edit collection</TooltipContent>
                      </Tooltip>
                      <Tooltip>
                        <TooltipTrigger asChild>
                          <button
                            onClick={(e) => {
                              e.stopPropagation();
                              handleDeleteCollection(collection);
                            }}
                            className="p-1 text-muted-foreground hover:text-error-foreground hover:bg-error/10 rounded"
                            aria-label="Delete collection"
                          >
                            <svg className="w-4 h-4" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                              <path strokeLinecap="round" strokeLinejoin="round" strokeWidth={2} d="M19 7l-.867 12.142A2 2 0 0116.138 21H7.862a2 2 0 01-1.995-1.858L5 7m5 4v6m4-6v6m1-10V4a1 1 0 00-1-1h-4a1 1 0 00-1 1v3M4 7h16" />
                            </svg>
                          </button>
                        </TooltipTrigger>
                        <TooltipContent>Delete collection</TooltipContent>
                      </Tooltip>
                    </div>
                  </div>
                    ))}
                  </>
                )}
              </div>
            )}
              </div>
            </div>
          </>
        ) : (
          // Desktop: Inline resizable sidebar
          <div
            className="border-l border-border bg-card flex h-full overflow-hidden flex-shrink-0"
            style={{ width: `${sidebarWidth}px` }}
          >
            {/* Resize Handle */}
            <Tooltip>
              <TooltipTrigger asChild>
                <div
                  className="flex items-center justify-center cursor-col-resize select-none px-1 -mr-2 relative z-10"
                  onMouseDown={handleResizeStart}
                  aria-label="Drag to resize"
                >
                  <div className="w-1 h-12 bg-border hover:bg-muted-foreground/50 rounded transition-colors" />
                </div>
              </TooltipTrigger>
              <TooltipContent>Drag to resize</TooltipContent>
            </Tooltip>

            {/* Main Content */}
            <div className="flex flex-col flex-1 min-w-0">
              {/* Sidebar Header */}
              <div className="p-4 border-b border-border flex items-center justify-between flex-shrink-0">
                <h3 className="font-semibold text-foreground">Collections</h3>
                <Tooltip>
                  <TooltipTrigger asChild>
                    <button
                      onClick={() => setCollectionsOpen(false)}
                      className="p-1 text-muted-foreground hover:text-foreground rounded transition-colors"
                      aria-label="Close"
                    >
                      <svg className="w-5 h-5" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                        <path strokeLinecap="round" strokeLinejoin="round" strokeWidth={2} d="M6 18L18 6M6 6l12 12" />
                      </svg>
                    </button>
                  </TooltipTrigger>
                  <TooltipContent>Close</TooltipContent>
                </Tooltip>
              </div>

              <div className="flex-shrink-0 p-4 border-b border-border">
                <button
                  onClick={handleCreateCollection}
                  className="w-full flex items-center justify-center gap-2 px-4 py-2 text-sm font-medium bg-primary text-white rounded-lg hover:bg-primary/90 transition-colors"
                >
                  <svg className="w-4 h-4" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                    <path strokeLinecap="round" strokeLinejoin="round" strokeWidth={2} d="M12 4v16m8-8H4" />
                  </svg>
                  New Collection
                </button>
              </div>

              <div className="flex-1 overflow-y-auto">
                {/* All Dashboards */}
                <button
                  onClick={() => setSearchParams({})}
                  className={`w-full flex items-center gap-3 px-4 py-3 text-left transition-colors ${
                    !activeCollectionId
                      ? 'bg-warning text-foreground font-medium'
                      : 'text-foreground hover:bg-accent'
                  }`}
                >
                  <svg className="w-5 h-5" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                    <path strokeLinecap="round" strokeLinejoin="round" strokeWidth={2} d="M9 17V7m0 10a2 2 0 01-2 2H5a2 2 0 01-2-2V7a2 2 0 012-2h2a2 2 0 012 2m0 10a2 2 0 002 2h2a2 2 0 002-2M9 7a2 2 0 012-2h2a2 2 0 012 2m0 10V7m0 10a2 2 0 002 2h2a2 2 0 002-2V7a2 2 0 00-2-2h-2a2 2 0 00-2 2" />
                  </svg>
                  <span className="flex-1">All Dashboards</span>
                  <span className="text-sm text-muted-foreground">{dashboards?.length || 0}</span>
                </button>

                {/* Collections List - Grouped by Public/Private */}
                {collections && collections.length > 0 && (
                  <div className="py-2">
                    {/* Public Collections Section */}
                    {collections.filter(c => c.is_public).length > 0 && (
                      <>
                        <div className="px-4 py-2 text-xs font-semibold text-muted-foreground uppercase tracking-wider flex items-center gap-2">
                          <svg className="w-3 h-3" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                            <path strokeLinecap="round" strokeLinejoin="round" strokeWidth={2} d="M3.055 11H5a2 2 0 012 2v1a2 2 0 002 2 2 2 0 012 2v2.945M8 3.935V5.5A2.5 2.5 0 0010.5 8h.5a2 2 0 012 2 2 2 0 104 0 2 2 0 012-2h1.064M15 20.488V18a2 2 0 012-2h3.064M21 12a9 9 0 11-18 0 9 9 0 0118 0z" />
                          </svg>
                          Public Collections
                        </div>
                        {collections.filter(c => c.is_public).map((collection) => (
                          <div
                            key={collection.id}
                            className={`group relative ${
                              activeCollectionId === collection.id
                                ? 'bg-primary/10'
                                : 'hover:bg-accent'
                            }`}
                          >
                            <button
                              onClick={() => handleCollectionClick(collection.id)}
                              className={`w-full flex items-center gap-3 px-4 py-3 text-left transition-colors ${
                                activeCollectionId === collection.id
                                  ? 'text-foreground font-medium'
                                  : 'text-foreground'
                              }`}
                            >
                              <div
                                className="w-3 h-3 rounded-full flex-shrink-0"
                                style={{ backgroundColor: collection.color || '#d97706' }}
                              />
                              <span className="flex-1 truncate">{collection.name}</span>
                              <span className="text-sm text-muted-foreground">
                                {collection.dashboards?.length || 0}
                              </span>
                            </button>

                            {/* Quick Actions (visible on hover) */}
                            <div className="absolute right-2 top-1/2 -translate-y-1/2 opacity-0 group-hover:opacity-100 transition-opacity flex gap-1">
                              <Tooltip>
                                <TooltipTrigger asChild>
                                  <button
                                    onClick={(e) => {
                                      e.stopPropagation();
                                      handleEditCollection(collection);
                                    }}
                                    className="p-1 text-muted-foreground hover:text-primary hover:bg-primary/10 rounded"
                                    aria-label="Edit collection"
                                  >
                                    <svg className="w-4 h-4" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                                      <path strokeLinecap="round" strokeLinejoin="round" strokeWidth={2} d="M11 5H6a2 2 0 00-2 2v11a2 2 0 002 2h11a2 2 0 002-2v-5m-1.414-9.414a2 2 0 112.828 2.828L11.828 15H9v-2.828l8.586-8.586z" />
                                    </svg>
                                  </button>
                                </TooltipTrigger>
                                <TooltipContent>Edit collection</TooltipContent>
                              </Tooltip>
                              <Tooltip>
                                <TooltipTrigger asChild>
                                  <button
                                    onClick={(e) => {
                                      e.stopPropagation();
                                      handleDeleteCollection(collection);
                                    }}
                                    className="p-1 text-muted-foreground hover:text-error-foreground hover:bg-error/10 rounded"
                                    aria-label="Delete collection"
                                  >
                                    <svg className="w-4 h-4" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                                      <path strokeLinecap="round" strokeLinejoin="round" strokeWidth={2} d="M19 7l-.867 12.142A2 2 0 0116.138 21H7.862a2 2 0 01-1.995-1.858L5 7m5 4v6m4-6v6m1-10V4a1 1 0 00-1-1h-4a1 1 0 00-1 1v3M4 7h16" />
                                    </svg>
                                  </button>
                                </TooltipTrigger>
                                <TooltipContent>Delete collection</TooltipContent>
                              </Tooltip>
                            </div>
                          </div>
                        ))}
                      </>
                    )}

                    {/* Private Collections Section */}
                    {collections.filter(c => !c.is_public).length > 0 && (
                      <>
                        <div className="px-4 py-2 text-xs font-semibold text-muted-foreground uppercase tracking-wider flex items-center gap-2 mt-2">
                          <svg className="w-3 h-3" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                            <path strokeLinecap="round" strokeLinejoin="round" strokeWidth={2} d="M12 15v2m-6 4h12a2 2 0 002-2v-6a2 2 0 00-2-2H6a2 2 0 00-2 2v6a2 2 0 002 2zm10-10V7a4 4 0 00-8 0v4h8z" />
                          </svg>
                          Private Collections
                        </div>
                        {collections.filter(c => !c.is_public).map((collection) => (
                          <div
                            key={collection.id}
                            className={`group relative ${
                              activeCollectionId === collection.id
                                ? 'bg-primary/10'
                                : 'hover:bg-accent'
                            }`}
                          >
                            <button
                              onClick={() => handleCollectionClick(collection.id)}
                              className={`w-full flex items-center gap-3 px-4 py-3 text-left transition-colors ${
                                activeCollectionId === collection.id
                                  ? 'text-foreground font-medium'
                                  : 'text-foreground'
                              }`}
                            >
                              <div
                                className="w-3 h-3 rounded-full flex-shrink-0"
                                style={{ backgroundColor: collection.color || '#d97706' }}
                              />
                              <span className="flex-1 truncate">{collection.name}</span>
                              <span className="text-sm text-muted-foreground">
                                {collection.dashboards?.length || 0}
                              </span>
                            </button>

                            {/* Quick Actions (visible on hover) */}
                            <div className="absolute right-2 top-1/2 -translate-y-1/2 opacity-0 group-hover:opacity-100 transition-opacity flex gap-1">
                              <Tooltip>
                                <TooltipTrigger asChild>
                                  <button
                                    onClick={(e) => {
                                      e.stopPropagation();
                                      handleEditCollection(collection);
                                    }}
                                    className="p-1 text-muted-foreground hover:text-primary hover:bg-primary/10 rounded"
                                    aria-label="Edit collection"
                                  >
                                    <svg className="w-4 h-4" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                                      <path strokeLinecap="round" strokeLinejoin="round" strokeWidth={2} d="M11 5H6a2 2 0 00-2 2v11a2 2 0 002 2h11a2 2 0 002-2v-5m-1.414-9.414a2 2 0 112.828 2.828L11.828 15H9v-2.828l8.586-8.586z" />
                                    </svg>
                                  </button>
                                </TooltipTrigger>
                                <TooltipContent>Edit collection</TooltipContent>
                              </Tooltip>
                              <Tooltip>
                                <TooltipTrigger asChild>
                                  <button
                                    onClick={(e) => {
                                      e.stopPropagation();
                                      handleDeleteCollection(collection);
                                    }}
                                    className="p-1 text-muted-foreground hover:text-error-foreground hover:bg-error/10 rounded"
                                    aria-label="Delete collection"
                                  >
                                    <svg className="w-4 h-4" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                                      <path strokeLinecap="round" strokeLinejoin="round" strokeWidth={2} d="M19 7l-.867 12.142A2 2 0 0116.138 21H7.862a2 2 0 01-1.995-1.858L5 7m5 4v6m4-6v6m1-10V4a1 1 0 00-1-1h-4a1 1 0 00-1 1v3M4 7h16" />
                                    </svg>
                                  </button>
                                </TooltipTrigger>
                                <TooltipContent>Delete collection</TooltipContent>
                              </Tooltip>
                            </div>
                          </div>
                        ))}
                      </>
                    )}
                  </div>
                )}
              </div>
            </div>
          </div>
        )
      )}
      </div>

      {/* Create/Edit Collection Modal */}
      {showCollectionModal && (
        <div className="fixed inset-0 bg-black/50 flex items-center justify-center z-50 p-4">
          <div className="bg-card rounded-2xl shadow-2xl max-w-md w-full p-6">
            <div className="flex justify-between items-center mb-6">
              <h2 className="text-2xl font-bold text-foreground">
                {editingCollection ? 'Edit Collection' : 'Create Collection'}
              </h2>
              <button
                onClick={() => {
                  setShowCollectionModal(false);
                  setEditingCollection(null);
                  setCollectionFormData({ name: '', description: '', color: '#d97706', is_public: false });
                }}
                className="text-muted-foreground hover:text-foreground transition-colors"
              >
                <svg className="w-6 h-6" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                  <path strokeLinecap="round" strokeLinejoin="round" strokeWidth={2} d="M6 18L18 6M6 6l12 12" />
                </svg>
              </button>
            </div>

            <form onSubmit={handleCollectionSubmit} className="space-y-4">
              <div>
                <label className="block text-sm font-medium text-foreground mb-2">
                  Name *
                </label>
                <input
                  type="text"
                  value={collectionFormData.name}
                  onChange={(e) => setCollectionFormData({ ...collectionFormData, name: e.target.value })}
                  className="w-full px-4 py-2 border border-input rounded-lg bg-card text-foreground focus:ring-2 focus:ring-primary focus:border-transparent"
                  placeholder="Marketing Dashboards"
                  required
                />
              </div>

              <div>
                <label className="block text-sm font-medium text-foreground mb-2">
                  Description
                </label>
                <textarea
                  value={collectionFormData.description}
                  onChange={(e) => setCollectionFormData({ ...collectionFormData, description: e.target.value })}
                  className="w-full px-4 py-2 border border-input rounded-lg bg-card text-foreground focus:ring-2 focus:ring-primary focus:border-transparent resize-none"
                  placeholder="Dashboards for marketing team analytics"
                  rows={3}
                />
              </div>

              <div>
                <label className="block text-sm font-medium text-foreground mb-2">
                  Color
                </label>
                <div className="flex items-center gap-3">
                  <input
                    type="color"
                    value={collectionFormData.color}
                    onChange={(e) => setCollectionFormData({ ...collectionFormData, color: e.target.value })}
                    className="h-10 w-20 rounded-lg border border-input cursor-pointer"
                  />
                  <input
                    type="text"
                    value={collectionFormData.color}
                    onChange={(e) => setCollectionFormData({ ...collectionFormData, color: e.target.value })}
                    className="flex-1 px-4 py-2 border border-input rounded-lg bg-card text-foreground focus:ring-2 focus:ring-primary focus:border-transparent font-mono text-sm"
                    placeholder="#d97706"
                    pattern="^#[0-9A-Fa-f]{6}$"
                  />
                </div>
              </div>

              <div>
                <label className="flex items-center gap-3 cursor-pointer">
                  <input
                    type="checkbox"
                    checked={collectionFormData.is_public || false}
                    onChange={(e) => setCollectionFormData({ ...collectionFormData, is_public: e.target.checked })}
                    className="w-4 h-4 text-primary border-border rounded focus:ring-primary"
                  />
                  <div className="flex-1">
                    <span className="block text-sm font-medium text-foreground">
                      Make collection public
                    </span>
                    <span className="block text-xs text-muted-foreground">
                      Public collections are visible to all workspace members
                    </span>
                  </div>
                </label>
              </div>

              <div className="flex gap-3 pt-4">
                <button
                  type="button"
                  onClick={() => {
                    setShowCollectionModal(false);
                    setEditingCollection(null);
                    setCollectionFormData({ name: '', description: '', color: '#d97706', is_public: false });
                  }}
                  className="flex-1 px-4 py-2 text-sm font-medium bg-accent text-foreground hover:bg-accent/80 rounded-lg transition-colors"
                >
                  Cancel
                </button>
                <button
                  type="submit"
                  disabled={createCollectionMutation.isPending || updateCollectionMutation.isPending}
                  className="flex-1 px-4 py-2 text-sm font-medium text-white bg-primary hover:bg-primary/90 rounded-lg transition-colors disabled:opacity-50 disabled:cursor-not-allowed"
                >
                  {createCollectionMutation.isPending || updateCollectionMutation.isPending ? (
                    <span className="flex items-center justify-center gap-2">
                      <Spinner size="sm" />
                      Saving...
                    </span>
                  ) : (
                    editingCollection ? 'Update Collection' : 'Create Collection'
                  )}
                </button>
              </div>
            </form>
          </div>
        </div>
      )}

      {/* Add to Collection Modal */}
      {showAddToCollectionModal && selectedDashboard && (
        <div className="fixed inset-0 bg-black/50 flex items-center justify-center z-50 p-4">
          <div className="bg-card rounded-2xl shadow-2xl max-w-md w-full">
            <div className="flex justify-between items-center p-6 border-b border-border">
              <h2 className="text-2xl font-bold text-foreground">Add to Collection</h2>
              <button
                onClick={() => {
                  setShowAddToCollectionModal(false);
                  setSelectedDashboard(null);
                }}
                className="text-muted-foreground hover:text-foreground transition-colors"
              >
                <svg className="w-6 h-6" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                  <path strokeLinecap="round" strokeLinejoin="round" strokeWidth={2} d="M6 18L18 6M6 6l12 12" />
                </svg>
              </button>
            </div>

            <div className="p-6">
              <p className="text-sm text-muted-foreground mb-4">
                Add <span className="font-semibold">{selectedDashboard.title}</span> to:
              </p>

              {getAvailableCollections(selectedDashboard.dashboard_id).length === 0 ? (
                <div className="text-center py-8">
                  <p className="text-muted-foreground">This dashboard is in all collections</p>
                </div>
              ) : (
                <div className="space-y-2 max-h-96 overflow-y-auto">
                  {getAvailableCollections(selectedDashboard.dashboard_id).map((collection) => (
                    <button
                      key={collection.id}
                      onClick={() => {
                        addToCollectionMutation.mutate({
                          collectionId: collection.id,
                          dashboardId: selectedDashboard.dashboard_id,
                        });
                      }}
                      disabled={addToCollectionMutation.isPending}
                      className="w-full flex items-center gap-3 p-4 rounded-lg border border-border hover:bg-muted transition-colors disabled:opacity-50"
                    >
                      <div
                        className="w-4 h-4 rounded-full flex-shrink-0"
                        style={{ backgroundColor: collection.color || '#d97706' }}
                      />
                      <div className="flex-1 text-left">
                        <div className="flex items-center gap-2 mb-1">
                          <h3 className="font-medium text-foreground">{collection.name}</h3>
                          {collection.is_public ? (
                            <div className="flex items-center gap-1 px-1.5 py-0.5 rounded text-xs bg-success/10 text-success-foreground">
                              <svg className="w-3 h-3" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                                <path strokeLinecap="round" strokeLinejoin="round" strokeWidth={2} d="M3.055 11H5a2 2 0 012 2v1a2 2 0 002 2 2 2 0 012 2v2.945M8 3.935V5.5A2.5 2.5 0 0010.5 8h.5a2 2 0 012 2 2 2 0 104 0 2 2 0 012-2h1.064M15 20.488V18a2 2 0 012-2h3.064M21 12a9 9 0 11-18 0 9 9 0 0118 0z" />
                              </svg>
                              Public
                            </div>
                          ) : (
                            <div className="flex items-center gap-1 px-1.5 py-0.5 rounded text-xs bg-muted text-muted-foreground">
                              <svg className="w-3 h-3" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                                <path strokeLinecap="round" strokeLinejoin="round" strokeWidth={2} d="M12 15v2m-6 4h12a2 2 0 002-2v-6a2 2 0 00-2-2H6a2 2 0 00-2 2v6a2 2 0 002 2zm10-10V7a4 4 0 00-8 0v4h8z" />
                              </svg>
                              Private
                            </div>
                          )}
                        </div>
                        {collection.description && (
                          <p className="text-sm text-muted-foreground">{collection.description}</p>
                        )}
                      </div>
                      <svg className="w-5 h-5 text-muted-foreground" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                        <path strokeLinecap="round" strokeLinejoin="round" strokeWidth={2} d="M9 5l7 7-7 7" />
                      </svg>
                    </button>
                  ))}
                </div>
              )}
            </div>
          </div>
        </div>
      )}

      {/* Confirm Dialog */}
      <ConfirmDialog isOpen={isOpen} {...dialogProps} />
    </div>
  );
}
