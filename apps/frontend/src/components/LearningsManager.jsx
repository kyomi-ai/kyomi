// SPDX-License-Identifier: AGPL-3.0-or-later
import React, { useState, useEffect, useCallback } from 'react';
import { useAuth } from '../context/AuthContext';
import { Card, CardContent } from './ui/card';
import { Button } from './ui/button';
import { Switch } from './ui/switch';
import { Input } from './ui/input';
import { Badge } from './ui/badge';
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from './ui/select';
import { Trash2, Edit2, Search, Filter, ChevronLeft, ChevronRight, Database, Settings, Calculator, Code, Plus } from 'lucide-react';
import { Spinner } from './ui/spinner';
import useConfirm from '../hooks/useConfirm';
import { useDebouncedValue } from '../hooks/useDebouncedValue';
import ConfirmDialog from './ConfirmDialog';
import Modal from './Modal';
import { DatasourceIcon } from './ui/DatasourceIcon';

const ITEMS_PER_PAGE = 50;

/**
 * LearningCard - A single learning card
 */
const LearningCard = ({
  learning,
  datasources,
  canManage,
  onToggle,
  onEdit,
  onDelete,
  formatDate,
}) => {
  const ds = learning.datasource_slug
    ? datasources.find(d => d.slug === learning.datasource_slug)
    : null;

  return (
    <Card className={`${!learning.enabled ? 'opacity-50' : ''}`}>
      <CardContent className="pt-3 pb-2 px-4">
        {/* Top row: badges + controls */}
        <div className="flex items-center justify-between gap-2 mb-1">
          <div className="flex flex-wrap items-center gap-1.5 min-w-0">
            <Badge variant={learning.scope === 'workspace' ? 'default' : 'secondary'} className="flex-shrink-0 text-xs">
              {learning.scope === 'workspace' ? 'Workspace' : 'Personal'}
            </Badge>
            <Badge
              variant={learning.learning_type === 'preference' ? 'secondary' : learning.learning_type === 'metric' ? 'default' : 'outline'}
              className="flex-shrink-0 text-xs flex items-center gap-1"
            >
              {learning.learning_type === 'metric' ? <><Calculator className="h-3 w-3" /> Metric</> :
               learning.learning_type === 'preference' ? <><Settings className="h-3 w-3" /> Preference</> :
               <><Database className="h-3 w-3" /> Learning</>}
            </Badge>
            {learning.datasource_slug && (
              <Badge variant="outline" className="flex items-center gap-1 text-xs min-w-0">
                {ds && <DatasourceIcon type={ds.datasource_type} className="h-3 w-3 flex-shrink-0" />}
                <span className="truncate">{learning.datasource_slug}</span>
              </Badge>
            )}
            {learning.reference_queries?.length > 0 && (
              <Badge variant="outline" className="flex items-center gap-1 text-xs">
                <Code className="h-3 w-3" />
                {learning.reference_queries.length} {learning.reference_queries.length === 1 ? 'query' : 'queries'}
              </Badge>
            )}
          </div>
          <div className="flex gap-1 items-center flex-shrink-0">
            <Switch
              checked={learning.enabled}
              onCheckedChange={() => onToggle(learning.learning_id, learning.enabled)}
              disabled={!canManage}
            />
            <Button
              variant="ghost"
              size="sm"
              onClick={() => onEdit(learning)}
              className="text-muted-foreground hover:text-foreground h-7 w-7 p-0"
              disabled={!canManage}
            >
              <Edit2 className="w-3.5 h-3.5" />
            </Button>
            <Button
              variant="ghost"
              size="sm"
              onClick={() => onDelete(learning.learning_id)}
              className="text-muted-foreground hover:text-foreground h-7 w-7 p-0"
              disabled={!canManage}
            >
              <Trash2 className="w-3.5 h-3.5" />
            </Button>
          </div>
        </div>
        {/* Content */}
        <p className="text-sm line-clamp-2 mb-1">{learning.insight}</p>
        {/* Footer */}
        <div className="space-y-0.5">
          {learning.context && (
            <p className="text-xs text-muted-foreground truncate">
              Context: {learning.context}
            </p>
          )}
          <p className="text-xs text-muted-foreground">
            Learned {formatDate(learning.created_at)}
            {learning.times_used > 0 && ` • Used ${learning.times_used} times`}
          </p>
        </div>
      </CardContent>
    </Card>
  );
};

/**
 * LearningsManager - Manage agent learnings for workspace
 * Shows all learnings with ability to toggle enabled/disabled, edit, and delete
 *
 * Features:
 * - Server-side pagination and filtering
 * - Full-text search with debouncing
 * - Optimistic updates for toggle operations
 *
 * Permissions:
 * - Admins: Can manage all learnings (workspace and user-scoped)
 * - Regular users: Can only manage their own user-scoped learnings
 */
const LearningsManager = () => {
  const { apiClient, user } = useAuth();
  const { isOpen, dialogProps, confirm } = useConfirm();

  // Data state
  const [learnings, setLearnings] = useState([]);
  const [datasources, setDatasources] = useState([]);
  const [totalCount, setTotalCount] = useState(0);

  // Loading/error state
  const [loading, setLoading] = useState(true);
  const [initialLoad, setInitialLoad] = useState(true);
  const [error, setError] = useState(null);

  // Pagination state
  const [page, setPage] = useState(0);
  const [hasMore, setHasMore] = useState(false);

  // Filter state
  const [searchQuery, setSearchQuery] = useState('');
  const [scopeFilter, setScopeFilter] = useState('all'); // 'all', 'workspace', 'user'
  const [datasourceFilter, setDatasourceFilter] = useState('all'); // 'all', 'global', or specific slug
  const [typeFilter, setTypeFilter] = useState('all'); // 'all', 'learning', 'metric', 'preference'

  // Edit modal state
  const [editingLearning, setEditingLearning] = useState(null);
  const [editForm, setEditForm] = useState({ insight: '', context: '', datasource_slug: '', learning_type: 'learning', reference_queries: [] });
  const [saving, setSaving] = useState(false);
  const [editingQueryIdx, setEditingQueryIdx] = useState(null);

  // Debounced search value (300ms delay)
  const debouncedSearch = useDebouncedValue(searchQuery, 300);

  // Helper function to check if user can manage a learning
  const canManageLearning = (learning) => {
    // Admins can manage everything
    const isAdmin = user?.workspace_roles?.includes('workspace_admin');
    if (isAdmin) return true;

    // Regular users can only manage their own user-scoped learnings
    const isUserScoped = learning.scope === 'user';
    const isOwner = learning.learned_from_user === user?.user_id;
    return isUserScoped && isOwner;
  };

  // Load learnings from server with current filters
  const loadLearnings = useCallback(async () => {
    try {
      setLoading(true);
      setError(null);

      // Build query parameters
      const params = new URLSearchParams({
        offset: (page * ITEMS_PER_PAGE).toString(),
        limit: ITEMS_PER_PAGE.toString(),
      });

      // Add filters only if they have values
      if (debouncedSearch.trim()) {
        params.append('search', debouncedSearch.trim());
      }
      if (scopeFilter !== 'all') {
        params.append('scope', scopeFilter);
      }
      if (datasourceFilter !== 'all') {
        params.append('datasource', datasourceFilter);
      }

      const response = await apiClient.get(
        `/api/v1/workspaces/${user.workspace_id}/learnings?${params.toString()}`
      );

      const { items, total, has_more } = response.data;
      setLearnings(items);
      setTotalCount(total);
      setHasMore(has_more);
    } catch (err) {
      setError(err.response?.data?.detail || 'Failed to load learnings');
    } finally {
      setLoading(false);
      setInitialLoad(false);
    }
  }, [apiClient, user?.workspace_id, page, debouncedSearch, scopeFilter, datasourceFilter]);

  // Load datasources (for filter dropdown)
  const loadDatasources = useCallback(async () => {
    try {
      const response = await apiClient.get('/api/v1/datasources');
      setDatasources(response.data || []);
    } catch (err) {
      // Don't show error for this - just means no datasource filtering available
    }
  }, [apiClient]);

  // Load learnings when filters or pagination changes
  useEffect(() => {
    loadLearnings();
  }, [loadLearnings]);

  // Load datasources once on mount
  useEffect(() => {
    loadDatasources();
  }, [loadDatasources]);

  // Reset to first page when filters change
  useEffect(() => {
    setPage(0);
  }, [debouncedSearch, scopeFilter, datasourceFilter, typeFilter]);

  // Client-side filter for learning type (until backend API supports it)
  // Map old types (navigation, event_context) to 'learning' for filtering
  const normalizeType = (t) => (t === 'navigation' || t === 'event_context') ? 'learning' : t;
  const filteredLearnings = typeFilter === 'all'
    ? learnings
    : learnings.filter(l => normalizeType(l.learning_type) === typeFilter);

  // Optimistic toggle with rollback on error
  async function toggleLearning(learningId, currentEnabled) {
    // Optimistically update the UI
    setLearnings(prev => prev.map(l =>
      l.learning_id === learningId
        ? { ...l, enabled: !currentEnabled }
        : l
    ));

    try {
      setError(null);
      await apiClient.patch(
        `/api/v1/workspaces/${user.workspace_id}/learnings/${learningId}`,
        { enabled: !currentEnabled }
      );
      // Success - optimistic update already applied
    } catch (err) {

      // Revert the optimistic update
      setLearnings(prev => prev.map(l =>
        l.learning_id === learningId
          ? { ...l, enabled: currentEnabled }
          : l
      ));

      const errorMessage = err.response?.data?.detail || 'Failed to update learning';
      if (err.response?.status === 403) {
        setError('You need admin permissions to manage learnings');
      } else {
        setError(errorMessage);
      }
    }
  }

  async function deleteLearning(learningId) {
    const confirmed = await confirm({
      title: 'Delete Learning?',
      message: 'Are you sure you want to delete this learning? This action cannot be undone.',
      confirmText: 'Delete',
      cancelText: 'Cancel',
      variant: 'destructive'
    });

    if (!confirmed) {
      return;
    }

    // Store the learning for potential rollback
    const learningToDelete = learnings.find(l => l.learning_id === learningId);

    // Optimistically remove from UI
    setLearnings(prev => prev.filter(l => l.learning_id !== learningId));
    setTotalCount(prev => prev - 1);

    try {
      setError(null);
      await apiClient.delete(`/api/v1/workspaces/${user.workspace_id}/learnings/${learningId}`);
      // Success - optimistic update already applied
    } catch (err) {

      // Revert the optimistic update
      if (learningToDelete) {
        setLearnings(prev => [...prev, learningToDelete].sort(
          (a, b) => new Date(b.created_at) - new Date(a.created_at)
        ));
        setTotalCount(prev => prev + 1);
      }

      const errorMessage = err.response?.data?.detail || 'Failed to delete learning';
      if (err.response?.status === 403) {
        setError('You need admin permissions to delete learnings');
      } else {
        setError(errorMessage);
      }
    }
  }

  function formatDate(dateString) {
    const date = new Date(dateString);
    return date.toLocaleDateString('en-US', {
      year: 'numeric',
      month: 'short',
      day: 'numeric'
    });
  }

  function openEditModal(learning) {
    setEditingLearning(learning);
    setEditForm({
      insight: learning.insight,
      context: learning.context || '',
      datasource_slug: learning.datasource_slug || '',
      learning_type: (learning.learning_type === 'navigation' || learning.learning_type === 'event_context') ? 'learning' : (learning.learning_type || 'learning'),
      reference_queries: learning.reference_queries || [],
    });
    setEditingQueryIdx(null);
  }

  function closeEditModal() {
    setEditingLearning(null);
    setEditForm({ insight: '', context: '', datasource_slug: '', learning_type: 'learning', reference_queries: [] });
    setEditingQueryIdx(null);
  }

  async function saveEdit() {
    if (!editForm.insight.trim()) {
      setError('Insight cannot be empty');
      return;
    }

    try {
      setSaving(true);
      setError(null);

      // Build update payload
      const payload = {
        insight: editForm.insight,
        context: editForm.context || null,
        learning_type: editForm.learning_type
      };

      // Only include datasource_slug if it changed
      const originalSlug = editingLearning.datasource_slug || '';
      const newSlug = editForm.datasource_slug || '';
      if (originalSlug !== newSlug) {
        payload.datasource_slug = newSlug; // Empty string clears, slug sets
      }

      // Filter out empty queries and include reference_queries
      const cleanedQueries = editForm.reference_queries.filter(q => q.sql?.trim());
      payload.reference_queries = cleanedQueries.length > 0 ? cleanedQueries : null;

      await apiClient.patch(
        `/api/v1/workspaces/${user.workspace_id}/learnings/${editingLearning.learning_id}`,
        payload
      );

      // Update the local state optimistically
      setLearnings(prev => prev.map(l =>
        l.learning_id === editingLearning.learning_id
          ? {
              ...l,
              insight: editForm.insight,
              context: editForm.context || null,
              datasource_slug: newSlug || null,
              learning_type: editForm.learning_type,
              reference_queries: cleanedQueries.length > 0 ? cleanedQueries : null,
            }
          : l
      ));

      closeEditModal();
    } catch (err) {
      const errorMessage = err.response?.data?.detail || 'Failed to update learning';
      if (err.response?.status === 403) {
        setError('You need admin permissions to edit learnings');
      } else {
        setError(errorMessage);
      }
    } finally {
      setSaving(false);
    }
  }

  // Pagination helpers
  const offset = page * ITEMS_PER_PAGE;
  const showingStart = totalCount === 0 ? 0 : offset + 1;
  const showingEnd = Math.min(offset + learnings.length, totalCount);

  if (initialLoad) {
    return (
      <div className="flex items-center justify-center h-64">
        <Spinner size="lg" className="text-muted-foreground" />
      </div>
    );
  }

  return (
    <>
      <ConfirmDialog isOpen={isOpen} {...dialogProps} />

      <div className="flex flex-col flex-1 min-h-0">
        {/* Header */}
        <div className="flex flex-col sm:flex-row sm:justify-between sm:items-start gap-2 mb-4 flex-shrink-0">
          <div className="min-w-0">
            <h2 className="text-lg sm:text-xl font-semibold text-foreground">Auto-Learnings</h2>
            <p className="text-xs sm:text-sm text-muted-foreground mt-1">
              Insights the AI learns from your conversations
            </p>
          </div>
          <div className="text-xs sm:text-sm text-muted-foreground flex-shrink-0">
            {totalCount} total learning{totalCount !== 1 ? 's' : ''}
          </div>
        </div>

        {/* Search Bar and Filters */}
        <div className="flex flex-col sm:flex-row gap-2 sm:gap-3 mb-4 flex-shrink-0">
          <div className="relative flex-1">
            <Search className="absolute left-3 top-1/2 transform -translate-y-1/2 w-4 h-4 text-muted-foreground" />
            <Input
              type="text"
              placeholder="Search learnings..."
              value={searchQuery}
              onChange={(e) => setSearchQuery(e.target.value)}
              className="pl-10"
            />
          </div>
          <div className="flex items-center gap-2 sm:min-w-[150px]">
            <Filter className="w-4 h-4 text-muted-foreground flex-shrink-0" />
            <Select value={scopeFilter} onValueChange={setScopeFilter}>
              <SelectTrigger className="w-full">
                <SelectValue placeholder="Filter by scope" />
              </SelectTrigger>
              <SelectContent>
                <SelectItem value="all">All Scopes</SelectItem>
                <SelectItem value="workspace">Workspace</SelectItem>
                <SelectItem value="user">Personal</SelectItem>
              </SelectContent>
            </Select>
          </div>
          {datasources.length > 0 && (
            <div className="flex items-center gap-2 sm:min-w-[180px]">
              <Select value={datasourceFilter} onValueChange={setDatasourceFilter}>
                <SelectTrigger className="w-full">
                  <SelectValue placeholder="Filter by datasource" />
                </SelectTrigger>
                <SelectContent>
                  <SelectItem value="all">All Datasources</SelectItem>
                  <SelectItem value="global">Global Only</SelectItem>
                  {datasources.map(ds => (
                    <SelectItem key={ds.slug} value={ds.slug}>
                      <span className="flex items-center gap-2">
                        <DatasourceIcon type={ds.datasource_type} className="h-4 w-4" />
                        {ds.display_name || ds.slug}
                      </span>
                    </SelectItem>
                  ))}
                </SelectContent>
              </Select>
            </div>
          )}
          <div className="flex items-center gap-2 sm:min-w-[160px]">
            <Select value={typeFilter} onValueChange={setTypeFilter}>
              <SelectTrigger className="w-full">
                <SelectValue placeholder="Filter by type" />
              </SelectTrigger>
              <SelectContent>
                <SelectItem value="all">All Types</SelectItem>
                <SelectItem value="learning"><Database className="h-3 w-3 inline mr-1.5" />Learning</SelectItem>
                <SelectItem value="metric"><Calculator className="h-3 w-3 inline mr-1.5" />Metric</SelectItem>
                <SelectItem value="preference"><Settings className="h-3 w-3 inline mr-1.5" />Preference</SelectItem>
              </SelectContent>
            </Select>
          </div>
        </div>

        {/* Error Message */}
        {error && (
          <div className="bg-error text-error-foreground border border-error-border px-4 py-3 rounded-lg text-sm mb-4 flex-shrink-0">
            {error}
          </div>
        )}

        {/* Loading indicator for filter changes */}
        {loading && learnings.length > 0 && (
          <div className="flex items-center gap-2 text-sm text-muted-foreground mb-4 flex-shrink-0">
            <Spinner />
            Updating...
          </div>
        )}

        {/* Learnings List */}
        {totalCount === 0 && !loading ? (
          <div className="text-center py-12 text-muted-foreground flex-1">
            {searchQuery || scopeFilter !== 'all' || datasourceFilter !== 'all' || typeFilter !== 'all' ? (
              <>
                <p>No learnings match your filters.</p>
                <p className="text-sm mt-2">Try adjusting your search or filters.</p>
              </>
            ) : (
              <>
                <p>No learnings yet.</p>
                <p className="text-sm mt-2">The AI will automatically save insights as you chat.</p>
              </>
            )}
          </div>
        ) : (
          <>
            <div className="flex-1 min-h-0 overflow-y-auto space-y-3">
              {filteredLearnings.map(learning => (
                <LearningCard
                  key={learning.learning_id}
                  learning={learning}
                  datasources={datasources}
                  canManage={canManageLearning(learning)}
                  onToggle={toggleLearning}
                  onEdit={openEditModal}
                  onDelete={deleteLearning}
                  formatDate={formatDate}
                />
              ))}
            </div>

            {/* Pagination Controls */}
            {totalCount > ITEMS_PER_PAGE && (
              <div className="flex flex-col sm:flex-row justify-between items-center gap-4 mt-4 pt-4 border-t border-border flex-shrink-0">
                <span className="text-sm text-muted-foreground">
                  Showing {showingStart}-{showingEnd} of {totalCount}
                </span>
                <div className="flex gap-2">
                  <Button
                    variant="outline"
                    size="sm"
                    disabled={page === 0 || loading}
                    onClick={() => setPage(p => p - 1)}
                  >
                    <ChevronLeft className="w-4 h-4 mr-1" />
                    Previous
                  </Button>
                  <Button
                    variant="outline"
                    size="sm"
                    disabled={!hasMore || loading}
                    onClick={() => setPage(p => p + 1)}
                  >
                    Next
                    <ChevronRight className="w-4 h-4 ml-1" />
                  </Button>
                </div>
              </div>
            )}
          </>
        )}
      </div>

      {/* Edit Modal */}
      <Modal
        show={editingLearning !== null}
        onClose={closeEditModal}
        title="Edit Learning"
        size="lg"
        footer={
          <>
            <Button variant="outline" onClick={closeEditModal} disabled={saving}>
              Cancel
            </Button>
            <Button onClick={saveEdit} disabled={saving}>
              {saving && <Spinner className="mr-2" />}
              {saving ? 'Saving...' : 'Save Changes'}
            </Button>
          </>
        }
      >
        <div className="space-y-4">
          <div>
            <label className="block text-sm font-medium text-foreground mb-2">
              Insight *
            </label>
            <textarea
              value={editForm.insight}
              onChange={(e) => setEditForm({ ...editForm, insight: e.target.value })}
              className="w-full px-3 py-2 border border-border rounded-md text-foreground bg-background resize-none"
              rows={4}
              placeholder="What the AI should learn..."
            />
          </div>
          <div>
            <label className="block text-sm font-medium text-foreground mb-2">
              Context (optional)
            </label>
            <textarea
              value={editForm.context}
              onChange={(e) => setEditForm({ ...editForm, context: e.target.value })}
              className="w-full px-3 py-2 border border-border rounded-md text-foreground bg-background resize-none"
              rows={3}
              placeholder="Additional context about when/why this was learned..."
            />
          </div>
          <div>
            <label className="block text-sm font-medium text-foreground mb-2">
              Type
            </label>
            <Select
              value={editForm.learning_type}
              onValueChange={(value) => setEditForm({ ...editForm, learning_type: value })}
            >
              <SelectTrigger className="w-full">
                <SelectValue placeholder="Select type" />
              </SelectTrigger>
              <SelectContent>
                <SelectItem value="learning"><Database className="h-3 w-3 inline mr-1.5" />Learning</SelectItem>
                <SelectItem value="metric"><Calculator className="h-3 w-3 inline mr-1.5" />Metric Definition</SelectItem>
                <SelectItem value="preference"><Settings className="h-3 w-3 inline mr-1.5" />Preference</SelectItem>
              </SelectContent>
            </Select>
            <p className="text-xs text-muted-foreground mt-1">
              Learning: How to navigate and query data • Metric: Metric definitions • Preference: Display settings
            </p>
          </div>
          {datasources.length > 0 && (
            <div>
              <label className="block text-sm font-medium text-foreground mb-2">
                Datasource (optional)
              </label>
              <Select
                value={editForm.datasource_slug || '_global'}
                onValueChange={(value) => setEditForm({ ...editForm, datasource_slug: value === '_global' ? '' : value })}
              >
                <SelectTrigger className="w-full">
                  <SelectValue placeholder="Select datasource" />
                </SelectTrigger>
                <SelectContent>
                  <SelectItem value="_global">Global (all datasources)</SelectItem>
                  {datasources.map(ds => (
                    <SelectItem key={ds.slug} value={ds.slug}>
                      <span className="flex items-center gap-2">
                        <DatasourceIcon type={ds.datasource_type} className="h-4 w-4" />
                        {ds.display_name || ds.slug}
                      </span>
                    </SelectItem>
                  ))}
                </SelectContent>
              </Select>
              <p className="text-xs text-muted-foreground mt-1">
                Leave as "Global" for learnings that apply to all datasources
              </p>
            </div>
          )}

          {/* Reference Queries */}
          <div>
            <div className="flex items-center justify-between mb-2">
              <label className="block text-sm font-medium text-foreground flex items-center gap-2">
                <Code className="h-4 w-4" />
                Reference Queries
                {editForm.reference_queries.length > 0 && (
                  <Badge variant="secondary" className="text-xs">{editForm.reference_queries.length}</Badge>
                )}
              </label>
              <Button
                variant="outline"
                size="sm"
                onClick={() => {
                  setEditForm({
                    ...editForm,
                    reference_queries: [...editForm.reference_queries, { comment: '', sql: '', datasource: null }]
                  });
                  setEditingQueryIdx(editForm.reference_queries.length);
                }}
              >
                <Plus className="h-3.5 w-3.5 mr-1" />
                Add Query
              </Button>
            </div>
            <p className="text-xs text-muted-foreground mb-3">
              Canonical SQL queries the AI can use as starting points when this learning is relevant.
            </p>

            {editForm.reference_queries.length === 0 ? (
              <div className="text-sm text-muted-foreground italic p-3 bg-muted/50 rounded border border-border">
                No reference queries configured
              </div>
            ) : (
              <div className="space-y-2">
                {editForm.reference_queries.map((query, idx) => (
                  editingQueryIdx === idx ? (
                    // Edit mode
                    <div key={idx} className="border border-border rounded-lg p-4 space-y-3 bg-muted/20">
                      <div className="flex items-start gap-2">
                        <div className="flex-1 space-y-1">
                          <label className="text-xs font-medium text-foreground">Query Title</label>
                          <Input
                            value={query.comment}
                            onChange={(e) => {
                              const newQueries = [...editForm.reference_queries];
                              newQueries[idx] = { ...query, comment: e.target.value };
                              setEditForm({ ...editForm, reference_queries: newQueries });
                            }}
                            placeholder="e.g., Daily Revenue Trend"
                            className="text-sm"
                          />
                        </div>
                        <Button
                          variant="ghost"
                          size="sm"
                          onClick={() => {
                            const newQueries = editForm.reference_queries.filter((_, i) => i !== idx);
                            setEditForm({ ...editForm, reference_queries: newQueries });
                            setEditingQueryIdx(null);
                          }}
                          className="mt-5"
                        >
                          <Trash2 className="h-4 w-4 text-destructive" />
                        </Button>
                      </div>

                      <div className="space-y-1">
                        <label className="text-xs font-medium text-foreground">SQL Query</label>
                        <textarea
                          value={query.sql}
                          onChange={(e) => {
                            const newQueries = [...editForm.reference_queries];
                            newQueries[idx] = { ...query, sql: e.target.value };
                            setEditForm({ ...editForm, reference_queries: newQueries });
                          }}
                          placeholder="SELECT ..."
                          rows={4}
                          className="w-full font-mono text-xs p-2 border border-input rounded bg-background resize-y"
                        />
                      </div>

                      {datasources.length > 0 && (
                        <div className="space-y-1">
                          <label className="text-xs font-medium text-foreground">Datasource (optional)</label>
                          <Select
                            value={query.datasource || '_none'}
                            onValueChange={(value) => {
                              const newQueries = [...editForm.reference_queries];
                              newQueries[idx] = { ...query, datasource: value === '_none' ? null : value };
                              setEditForm({ ...editForm, reference_queries: newQueries });
                            }}
                          >
                            <SelectTrigger className="h-9">
                              <SelectValue placeholder="Select a datasource" />
                            </SelectTrigger>
                            <SelectContent>
                              <SelectItem value="_none">None</SelectItem>
                              {datasources.map(ds => (
                                <SelectItem key={ds.slug} value={ds.slug}>
                                  <span className="flex items-center gap-2">
                                    <DatasourceIcon type={ds.datasource_type} className="h-4 w-4" />
                                    {ds.display_name || ds.slug}
                                  </span>
                                </SelectItem>
                              ))}
                            </SelectContent>
                          </Select>
                        </div>
                      )}

                      <div className="flex gap-2 pt-2 border-t border-border">
                        <Button
                          variant="outline"
                          size="sm"
                          onClick={() => setEditingQueryIdx(null)}
                        >
                          Done
                        </Button>
                      </div>
                    </div>
                  ) : (
                    // Read-only view
                    <div key={idx} className="flex items-start gap-3 p-3 rounded-lg border border-border bg-muted/30 hover:bg-muted/50 transition-colors group">
                      <Code className="h-4 w-4 text-muted-foreground mt-0.5 shrink-0" />
                      <div className="flex-1 min-w-0">
                        <p className="font-medium text-sm text-foreground break-words">{query.comment || 'Untitled query'}</p>
                        <p className="text-xs text-muted-foreground font-mono mt-1 truncate">{query.sql}</p>
                        {query.datasource && (
                          <div className="mt-2">
                            <span className="inline-block px-2 py-1 rounded text-xs bg-secondary text-secondary-foreground">
                              {query.datasource}
                            </span>
                          </div>
                        )}
                      </div>
                      <Button
                        variant="ghost"
                        size="sm"
                        onClick={() => setEditingQueryIdx(idx)}
                        className="shrink-0 sm:opacity-0 sm:group-hover:opacity-100 transition-opacity"
                      >
                        <Edit2 className="h-4 w-4" />
                      </Button>
                    </div>
                  )
                ))}
              </div>
            )}
          </div>
        </div>
      </Modal>
    </>
  );
};

export default LearningsManager;
