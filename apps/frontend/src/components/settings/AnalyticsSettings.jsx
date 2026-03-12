// SPDX-License-Identifier: AGPL-3.0-or-later
import { useState, useEffect } from 'react';
import { Copy, Plus, Trash2, Globe, Code, Pencil, Database, RefreshCw } from 'lucide-react';
import apiClient from '../../api/apiClient';
import { Button } from '../ui/button';
import { Input } from '../ui/input';
import { Label } from '../ui/label';
import { Badge } from '../ui/badge';
import { Card, CardHeader, CardTitle, CardDescription, CardContent } from '../ui/card';
import { Spinner } from '../ui/spinner';
import ConfirmDialog from '../ConfirmDialog';
import useConfirm from '../../hooks/useConfirm';
import { toast } from '../../lib/toast';

export default function AnalyticsSettings() {
  const [sites, setSites] = useState([]);
  const [usageData, setUsageData] = useState(null);
  const [loading, setLoading] = useState(true);
  const [showForm, setShowForm] = useState(false);
  const [editingSite, setEditingSite] = useState(null);
  const [formName, setFormName] = useState('');
  const [formDomains, setFormDomains] = useState('');
  const [formDatasourceSlug, setFormDatasourceSlug] = useState('');
  const [datasourceSlugEdited, setDatasourceSlugEdited] = useState(false);
  const [saving, setSaving] = useState(false);
  const [refreshingSiteId, setRefreshingSiteId] = useState(null);
  const { isOpen, dialogProps, confirm } = useConfirm();

  useEffect(() => {
    fetchSites();
    fetchUsage();
  }, []);

  const fetchUsage = async () => {
    try {
      const response = await apiClient.get('/api/v1/analytics/usage');
      setUsageData(response.data);
    } catch (error) {
      // Usage data is non-critical; sites still load even if usage fails
    }
  };

  const fetchSites = async () => {
    try {
      setLoading(true);
      const response = await apiClient.get('/api/v1/analytics/sites');
      setSites(response.data);
    } catch (error) {
      toast.error('Failed to load analytics sites: ' + (error.response?.data?.detail || error.message));
    } finally {
      setLoading(false);
    }
  };

  const resetForm = () => {
    setFormName('');
    setFormDomains('');
    setFormDatasourceSlug('');
    setDatasourceSlugEdited(false);
    setShowForm(false);
    setEditingSite(null);
  };

  const generateSlug = (name) => {
    return name.toLowerCase().replace(/[^a-z0-9]+/g, '-').replace(/^-|-$/g, '') + '-analytics';
  };

  const parseDomains = (input) => {
    return input
      .split(',')
      .map((d) => d.trim())
      .filter((d) => d.length > 0);
  };

  const handleCreate = async () => {
    const name = formName.trim();
    const allowed_domains = parseDomains(formDomains);

    if (!name) {
      toast.error('Site name is required');
      return;
    }
    if (allowed_domains.length === 0) {
      toast.error('At least one domain is required');
      return;
    }

    try {
      setSaving(true);
      await apiClient.post('/api/v1/analytics/sites', {
        name,
        allowed_domains,
        datasource_slug: formDatasourceSlug || undefined,
      });
      toast.success('Analytics site created');
      resetForm();
      await fetchSites();
      fetchUsage();
    } catch (error) {
      toast.error('Failed to create site: ' + (error.response?.data?.detail || error.message));
    } finally {
      setSaving(false);
    }
  };

  const handleUpdate = async (id) => {
    const name = formName.trim();
    const allowed_domains = parseDomains(formDomains);

    if (!name) {
      toast.error('Site name is required');
      return;
    }
    if (allowed_domains.length === 0) {
      toast.error('At least one domain is required');
      return;
    }

    try {
      setSaving(true);
      await apiClient.put(`/api/v1/analytics/sites/${id}`, {
        name,
        allowed_domains,
        datasource_slug: datasourceSlugEdited ? (formDatasourceSlug || undefined) : undefined,
      });
      toast.success('Analytics site updated');
      resetForm();
      await fetchSites();
    } catch (error) {
      toast.error('Failed to update site: ' + (error.response?.data?.detail || error.message));
    } finally {
      setSaving(false);
    }
  };

  const handleDelete = async (id, name) => {
    const confirmed = await confirm({
      title: 'Delete Analytics Site?',
      message: `Delete "${name}"? This cannot be undone.`,
      confirmText: 'Delete',
      variant: 'destructive',
    });
    if (!confirmed) return;

    try {
      await apiClient.delete(`/api/v1/analytics/sites/${id}`);
      toast.success('Analytics site deleted');
      await fetchSites();
      fetchUsage();
    } catch (error) {
      toast.error('Failed to delete site: ' + (error.response?.data?.detail || error.message));
    }
  };

  const handleRefreshCatalog = async (site) => {
    if (!site.datasource_slug) return;
    setRefreshingSiteId(site.id);
    try {
      const response = await apiClient.refreshCatalog(site.datasource_slug, { force: false });
      if (response.status === 'completed') {
        toast.success(`Catalog refreshed: ${response.message}`);
      } else if (response.status === 'error') {
        toast.error(`Catalog refresh failed: ${response.message}`);
      } else {
        toast.info(response.message || 'Catalog refresh started');
      }
    } catch (error) {
      toast.error('Failed to refresh catalog: ' + (error.response?.data?.detail || error.message));
    } finally {
      setRefreshingSiteId(null);
    }
  };

  const startEditing = (site) => {
    setEditingSite(site);
    setFormName(site.name);
    setFormDomains(site.allowed_domains.join(', '));
    setFormDatasourceSlug(site.datasource_slug || '');
    setDatasourceSlugEdited(true); // Don't auto-generate when editing
    setShowForm(false);
  };

  const copySnippet = async (snippet) => {
    try {
      await navigator.clipboard.writeText(snippet);
      toast.success('Snippet copied to clipboard');
    } catch {
      toast.error('Failed to copy snippet — please copy it manually');
    }
  };

  if (loading) {
    return (
      <div className="p-6">
        <div className="flex items-center justify-center py-12">
          <Spinner size="lg" className="text-primary" />
        </div>
      </div>
    );
  }

  return (
    <div className="p-6">
      <h2 className="text-xl font-semibold text-foreground mb-6">Analytics</h2>

      <div className="space-y-6">
        {/* Header with Add button */}
        <div className="flex items-center justify-between">
          <div>
            <h3 className="text-lg font-medium text-foreground">Analytics Sites</h3>
            <p className="text-sm text-muted-foreground">
              Install analytics on your websites to track visitor data.
            </p>
          </div>
          {!showForm && !editingSite && (
            <Button onClick={() => { setShowForm(true); setEditingSite(null); setFormName(''); setFormDomains(''); setFormDatasourceSlug(''); setDatasourceSlugEdited(false); }}>
              <Plus className="h-4 w-4 mr-2" />
              Add Site
            </Button>
          )}
        </div>

        {/* Event usage bar */}
        {usageData && usageData.events_limit > 0 && (
          <Card>
            <CardContent className="pt-6">
              <div className="flex justify-between mb-2">
                <span className="text-sm font-medium text-foreground">
                  Event Usage This Month
                </span>
                <span className="text-sm font-medium text-foreground">
                  {usageData.events_used.toLocaleString()} / {usageData.events_limit.toLocaleString()} ({usageData.usage_percent.toFixed(1)}%)
                </span>
              </div>
              <div className="w-full bg-muted rounded-full h-2">
                <div
                  className={`h-2 rounded-full transition-all ${
                    usageData.status === 'blocked' || usageData.status === 'exceeded'
                      ? 'bg-error-foreground'
                      : usageData.status === 'warning'
                      ? 'bg-warning-foreground'
                      : 'bg-success-foreground'
                  }`}
                  style={{ width: `${Math.min(100, usageData.usage_percent)}%` }}
                />
              </div>
              {usageData.status === 'blocked' && (
                <p className="text-sm text-error-foreground mt-2">
                  Event quota exceeded. Analytics events are being dropped.
                </p>
              )}
              {usageData.status === 'exceeded' && (
                <p className="text-sm text-error-foreground mt-2">
                  Event quota reached. Events are still accepted during the grace period.
                </p>
              )}
            </CardContent>
          </Card>
        )}

        {/* Inline create/edit form */}
        {(showForm || editingSite) && (
          <Card>
            <CardHeader>
              <CardTitle>{editingSite ? 'Edit Site' : 'New Analytics Site'}</CardTitle>
              <CardDescription>
                {editingSite
                  ? 'Update the site name or allowed domains.'
                  : 'Add a new site to start tracking analytics.'}
              </CardDescription>
            </CardHeader>
            <CardContent>
              <div className="space-y-4">
                <div className="space-y-2">
                  <Label htmlFor="site-name">Site Name</Label>
                  <Input
                    id="site-name"
                    value={formName}
                    onChange={(e) => {
                      const name = e.target.value;
                      setFormName(name);
                      if (!editingSite && !datasourceSlugEdited) {
                        setFormDatasourceSlug(generateSlug(name));
                      }
                    }}
                    placeholder="e.g. My Website"
                  />
                </div>
                <div className="space-y-2">
                  <Label htmlFor="site-domains">Allowed Domains</Label>
                  <Input
                    id="site-domains"
                    value={formDomains}
                    onChange={(e) => setFormDomains(e.target.value)}
                    placeholder="e.g. example.com, app.example.com"
                  />
                  <p className="text-xs text-muted-foreground">
                    Comma-separated list of domains that are allowed to send analytics events.
                  </p>
                </div>
                <div className="space-y-2">
                  <Label htmlFor="datasource-slug">Datasource Slug</Label>
                  <Input
                    id="datasource-slug"
                    value={formDatasourceSlug}
                    onChange={(e) => { setFormDatasourceSlug(e.target.value); setDatasourceSlugEdited(true); }}
                    placeholder="e.g. my-website-analytics"
                  />
                  <p className="text-xs text-muted-foreground">
                    {editingSite
                      ? 'Rename the datasource slug. Existing queries and dashboards using the old slug will break.'
                      : 'This creates a queryable datasource in your workspace. Use this slug to reference it in queries and dashboards.'}
                  </p>
                </div>
                <div className="flex items-center gap-2 pt-2">
                  <Button
                    onClick={() => editingSite ? handleUpdate(editingSite.id) : handleCreate()}
                    disabled={saving}
                  >
                    {saving ? (
                      <>
                        <Spinner size="sm" className="mr-2" />
                        Saving...
                      </>
                    ) : (
                      editingSite ? 'Save Changes' : 'Create Site'
                    )}
                  </Button>
                  <Button variant="outline" onClick={resetForm} disabled={saving}>
                    Cancel
                  </Button>
                </div>
              </div>
            </CardContent>
          </Card>
        )}

        {/* Site list */}
        {sites.filter((site) => !editingSite || site.id !== editingSite.id).map((site) => (
          <Card key={site.id}>
            <CardHeader>
              <div className="flex items-center justify-between">
                <div className="space-y-1">
                  <CardTitle className="flex items-center gap-2">
                    <Globe className="h-4 w-4 text-muted-foreground" />
                    {site.name}
                  </CardTitle>
                  <div className="flex flex-wrap items-center gap-1.5">
                    {site.allowed_domains.map((domain) => (
                      <Badge key={domain} variant="secondary">
                        {domain}
                      </Badge>
                    ))}
                  </div>
                  {site.datasource_slug && (
                    <p className="text-xs text-muted-foreground flex items-center gap-1">
                      <Database className="h-3 w-3" />
                      Datasource: <span className="font-mono">{site.datasource_slug}</span>
                    </p>
                  )}
                </div>
                <div className="flex items-center gap-1">
                  {site.datasource_slug && (
                    <Button
                      variant="ghost"
                      size="sm"
                      onClick={() => handleRefreshCatalog(site)}
                      disabled={refreshingSiteId === site.id}
                      title="Refresh catalog"
                    >
                      <RefreshCw className={`h-4 w-4 text-muted-foreground ${refreshingSiteId === site.id ? 'animate-spin' : ''}`} />
                    </Button>
                  )}
                  <Button
                    variant="ghost"
                    size="sm"
                    onClick={() => startEditing(site)}
                    title="Edit site"
                  >
                    <Pencil className="h-4 w-4 text-muted-foreground" />
                  </Button>
                  <Button
                    variant="ghost"
                    size="sm"
                    onClick={() => handleDelete(site.id, site.name)}
                    title="Delete site"
                  >
                    <Trash2 className="h-4 w-4 text-muted-foreground" />
                  </Button>
                </div>
              </div>
            </CardHeader>
            <CardContent>
              <div className="space-y-3">
                <div className="flex items-center gap-2 text-sm text-muted-foreground">
                  <Code className="h-4 w-4" />
                  <span>Tracking snippet</span>
                </div>
                <div className="relative">
                  <pre className="bg-muted rounded-lg p-3 text-sm font-mono overflow-x-auto pr-12 text-foreground">
                    {site.snippet}
                  </pre>
                  <Button
                    size="sm"
                    variant="ghost"
                    className="absolute top-2 right-2"
                    onClick={() => copySnippet(site.snippet)}
                    title="Copy snippet"
                  >
                    <Copy className="h-4 w-4" />
                  </Button>
                </div>
                <p className="text-xs text-muted-foreground">
                  Created {new Date(site.created_at).toLocaleDateString()}
                </p>
              </div>
            </CardContent>
          </Card>
        ))}

        {/* Empty state */}
        {sites.length === 0 && !showForm && !editingSite && (
          <Card>
            <CardContent className="py-12 text-center">
              <Globe className="h-10 w-10 text-muted-foreground mx-auto mb-3" />
              <p className="text-muted-foreground">
                No analytics sites yet. Add one to start tracking visitor data.
              </p>
            </CardContent>
          </Card>
        )}
      </div>

      {/* Confirm Dialog */}
      <ConfirmDialog isOpen={isOpen} {...dialogProps} />
    </div>
  );
}
