// SPDX-License-Identifier: AGPL-3.0-or-later
import { useState, useEffect, useCallback } from 'react';
import { useSearchParams } from 'react-router-dom';
import { Loader2, Plus, AlertTriangle, ArrowLeft, CheckCircle2 } from 'lucide-react';
import { useAuth } from '../context/AuthContext';
import apiClient from '@/api/apiClient';
import { Button } from '@/components/ui/button';
import { Card } from '@/components/ui/card';
import { Alert, AlertDescription } from '@/components/ui/alert';
import { Input } from '@/components/ui/input';
import { DatasourceIcon, getDatasourceLabel } from '@/components/ui/DatasourceIcon';
import CopyButton from '@/components/settings/datasources/shared/components/CopyButton';

/** Generate a URL-safe slug from a display name (matches backend logic). */
function generateSlug(name) {
  return name
    .toLowerCase()
    .replace(/[\s_]+/g, '-')
    .replace(/[^a-z0-9-]/g, '')
    .replace(/-+/g, '-')
    .replace(/^-|-$/g, '');
}

/** Datasource types that support Kyomi Connect. */
const CONNECT_TYPES = [
  { value: 'postgres', label: 'PostgreSQL' },
  { value: 'mysql', label: 'MySQL' },
  { value: 'clickhouse', label: 'ClickHouse' },
  { value: 'sqlserver', label: 'SQL Server' },
  { value: 'redshift', label: 'Redshift' },
];

export default function ConnectSetupPage() {
  const { user } = useAuth();
  const [searchParams] = useSearchParams();
  const callbackPort = searchParams.get('callback_port');
  const callbackState = searchParams.get('state');

  const [datasources, setDatasources] = useState([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState(null);

  // 'select' | 'create' | 'success'
  const [step, setStep] = useState('select');

  // Create form state
  const [newName, setNewName] = useState('');
  const [newSlug, setNewSlug] = useState('');
  const [showSlug, setShowSlug] = useState(false);
  const [newType, setNewType] = useState('postgres');
  const [creating, setCreating] = useState(false);

  // Token generation state
  const [generatingTokenFor, setGeneratingTokenFor] = useState(null);

  // Success state
  const [token, setToken] = useState(null);
  const [deliveryStatus, setDeliveryStatus] = useState(null); // null | 'pending' | 'delivered' | 'failed'

  const isAdmin = user?.workspace_roles?.includes('workspace_admin');
  const hasCallback = callbackPort && callbackState;

  // Fetch Connect datasources
  useEffect(() => {
    fetchDatasources();
  }, []);

  const fetchDatasources = async () => {
    try {
      setLoading(true);
      const response = await apiClient.get('/api/v1/datasources');
      const connectDs = response.data.filter(ds => ds.connection_type === 'connect');
      setDatasources(connectDs);
    } catch (err) {
      setError('Failed to load datasources. Please try again.');
      console.error('Failed to fetch datasources:', err);
    } finally {
      setLoading(false);
    }
  };

  // Deliver token to CLI callback server via background fetch (page stays visible)
  const deliverToken = useCallback(async (tokenValue) => {
    if (!hasCallback) return;

    setDeliveryStatus('pending');

    try {
      const callbackUrl = `http://127.0.0.1:${callbackPort}/callback?token=${encodeURIComponent(tokenValue)}&state=${encodeURIComponent(callbackState)}`;
      const res = await fetch(callbackUrl);
      if (res.ok) {
        setDeliveryStatus('delivered');
      } else {
        setDeliveryStatus('failed');
      }
    } catch {
      // Server unreachable (SSH/headless, port closed, etc.)
      setDeliveryStatus('failed');
    }
  }, [hasCallback, callbackPort, callbackState]);

  // Select an existing datasource and generate a token
  const handleSelectDatasource = async (ds) => {
    try {
      setGeneratingTokenFor(ds.id);
      const response = await apiClient.post(`/api/v1/datasources/${ds.id}/connect/rotate-token`);
      setToken(response.data.token);
      setStep('success');
      deliverToken(response.data.token);
    } catch (err) {
      const msg = err.response?.data?.detail || err.response?.data?.error || 'Failed to generate token. Please try again.';
      setError(msg);
      console.error('Failed to rotate token:', err);
    } finally {
      setGeneratingTokenFor(null);
    }
  };

  // Create a new datasource and get the token
  const handleCreate = async (e) => {
    e.preventDefault();
    if (!newName.trim()) return;

    try {
      setCreating(true);
      setError(null);
      const payload = {
        name: newName.trim(),
        datasource_type: newType,
        connection_type: 'connect',
      };
      // Only send slug if user has customized it after a conflict
      if (showSlug && newSlug) {
        payload.slug = newSlug;
      }
      const response = await apiClient.post('/api/v1/datasources', payload);
      const tokenValue = response.data.connect_token;
      setToken(tokenValue);
      setStep('success');
      deliverToken(tokenValue);
    } catch (err) {
      const msg = err.response?.data?.detail || err.response?.data?.error || 'Failed to create datasource. Please try again.';

      // 409 = name or slug conflict — show slug field so user can customize
      if (err.response?.status === 409) {
        setShowSlug(true);
        if (!newSlug) {
          setNewSlug(generateSlug(newName));
        }
      }

      setError(msg);
      console.error('Failed to create datasource:', err);
    } finally {
      setCreating(false);
    }
  };

  // Non-admin: show a message
  if (!loading && !isAdmin) {
    return (
      <div className="min-h-screen flex items-center justify-center bg-background p-4">
        <Card className="max-w-lg w-full p-8">
          <div className="text-center mb-8">
            <div className="mb-6">
              <img src="/kyomi_full_logo.svg" alt="Kyomi" className="h-10 mx-auto dark:hidden" />
              <img src="/kyomi_full_logo_white.svg" alt="Kyomi" className="h-10 mx-auto hidden dark:block" />
            </div>
            <h2 className="text-3xl font-bold mb-2">Admin Access Required</h2>
            <p className="text-muted-foreground">
              Only workspace admins can set up Kyomi Connect datasources.
              Contact your workspace admin to get a Connect token.
            </p>
          </div>
        </Card>
      </div>
    );
  }

  return (
    <div className="min-h-screen flex items-center justify-center bg-background p-4">
      <Card className="max-w-lg w-full p-8">
        {/* Logo + Header */}
        <div className="text-center mb-8">
          <div className="mb-6">
            <img src="/kyomi_full_logo.svg" alt="Kyomi" className="h-10 mx-auto dark:hidden" />
            <img src="/kyomi_full_logo_white.svg" alt="Kyomi" className="h-10 mx-auto hidden dark:block" />
          </div>
          <h2 className="text-3xl font-bold mb-2">Connect Setup</h2>
          <p className="text-muted-foreground">
            {step === 'select' && 'Select or create a datasource for your agent'}
            {step === 'create' && 'Create a new datasource'}
            {step === 'success' && 'Your Connect token is ready'}
          </p>
        </div>

        {error && (
          <Alert variant="destructive" className="mb-6">
            <AlertDescription>{error}</AlertDescription>
          </Alert>
        )}

        {/* Loading */}
        {loading && (
          <div className="flex items-center justify-center py-12">
            <Loader2 className="h-6 w-6 animate-spin text-muted-foreground" />
          </div>
        )}

        {/* Step: Select datasource */}
        {!loading && step === 'select' && (
          <SelectStep
            datasources={datasources}
            generatingTokenFor={generatingTokenFor}
            onSelect={handleSelectDatasource}
            onCreateNew={() => { setError(null); setStep('create'); }}
          />
        )}

        {/* Step: Create new datasource */}
        {!loading && step === 'create' && (
          <CreateStep
            name={newName}
            slug={newSlug}
            showSlug={showSlug}
            type={newType}
            creating={creating}
            onNameChange={setNewName}
            onSlugChange={(v) => setNewSlug(v.toLowerCase().replace(/[^a-z0-9-]/g, ''))}
            onTypeChange={setNewType}
            onSubmit={handleCreate}
            onBack={() => { setError(null); setShowSlug(false); setNewSlug(''); setStep('select'); }}
          />
        )}

        {/* Step: Token generated */}
        {!loading && step === 'success' && token && (
          <SuccessStep
            token={token}
            hasCallback={hasCallback}
            deliveryStatus={deliveryStatus}
          />
        )}
      </Card>
    </div>
  );
}

/** Datasource selection list. */
function SelectStep({ datasources, generatingTokenFor, onSelect, onCreateNew }) {
  return (
    <div className="space-y-3">
      {datasources.length > 0 && (
        <div className="space-y-3">
          {datasources.map(ds => (
            <button
              key={ds.id}
              onClick={() => onSelect(ds)}
              disabled={generatingTokenFor === ds.id}
              className="w-full flex items-center gap-3 p-4 border border-border rounded-xl bg-card
                         hover:border-primary/40 hover:bg-primary/5
                         transition-colors text-left group disabled:opacity-60"
            >
              <DatasourceIcon type={ds.datasource_type} className="h-8 w-8" opacity={0.8} />
              <div className="flex-1 min-w-0">
                <span className="font-medium">{ds.name}</span>
                <div className="text-sm text-muted-foreground">{getDatasourceLabel(ds.datasource_type)}</div>
              </div>
              {generatingTokenFor === ds.id ? (
                <Loader2 className="h-4 w-4 animate-spin text-muted-foreground shrink-0" />
              ) : (
                <span className="text-xs text-muted-foreground opacity-0 group-hover:opacity-100 transition-opacity shrink-0">
                  Generate token
                </span>
              )}
            </button>
          ))}
        </div>
      )}

      {datasources.length > 0 && (
        <div className="relative my-4">
          <div className="absolute inset-0 flex items-center">
            <span className="w-full border-t border-border" />
          </div>
          <div className="relative flex justify-center text-xs">
            <span className="bg-card px-2 text-muted-foreground">or</span>
          </div>
        </div>
      )}

      <Button
        variant={datasources.length === 0 ? 'default' : 'outline'}
        className="w-full"
        size="lg"
        onClick={onCreateNew}
      >
        <Plus className="h-4 w-4 mr-2" />
        Create new datasource
      </Button>

      {datasources.length > 0 && (
        <p className="text-xs text-center text-muted-foreground mt-4">
          Generating a new token will disconnect any currently connected agent.
        </p>
      )}
    </div>
  );
}

/** New datasource creation form. */
function CreateStep({ name, slug, showSlug, type, creating, onNameChange, onSlugChange, onTypeChange, onSubmit, onBack }) {
  return (
    <form onSubmit={onSubmit} className="space-y-5">
      <button
        type="button"
        onClick={onBack}
        className="flex items-center gap-1 text-sm text-muted-foreground hover:text-foreground transition-colors -mt-1 mb-2"
      >
        <ArrowLeft className="h-3.5 w-3.5" />
        Back
      </button>

      <div>
        <label className="block text-sm font-semibold mb-2">Datasource name</label>
        <Input
          value={name}
          onChange={e => onNameChange(e.target.value)}
          placeholder="e.g. Production PostgreSQL"
          autoFocus
          required
        />
      </div>

      {showSlug && (
        <div>
          <label className="block text-sm font-semibold mb-2">Slug</label>
          <Input
            value={slug}
            onChange={e => onSlugChange(e.target.value)}
            placeholder="my-database"
            className="font-mono"
            required
          />
          <p className="text-xs text-muted-foreground mt-1">
            Must be unique within your workspace.
          </p>
        </div>
      )}

      <div>
        <label className="block text-sm font-semibold mb-2">Database type</label>
        <div className="grid grid-cols-2 gap-2">
          {CONNECT_TYPES.map(ct => (
            <button
              key={ct.value}
              type="button"
              onClick={() => onTypeChange(ct.value)}
              className={`flex items-center gap-2 p-3 rounded-xl border text-left text-sm transition-colors ${
                type === ct.value
                  ? 'border-primary bg-primary/5 text-foreground'
                  : 'border-border text-muted-foreground hover:border-muted-foreground/40 hover:text-foreground'
              }`}
            >
              <DatasourceIcon type={ct.value} className="h-5 w-5" opacity={type === ct.value ? 0.9 : 0.5} />
              {ct.label}
            </button>
          ))}
        </div>
      </div>

      <Button type="submit" className="w-full" size="lg" disabled={creating || !name.trim()}>
        {creating ? (
          <>
            <Loader2 className="h-4 w-4 mr-2 animate-spin" />
            Creating...
          </>
        ) : (
          'Create & Generate Token'
        )}
      </Button>
    </form>
  );
}

/** Token display and delivery status. */
function SuccessStep({ token, hasCallback, deliveryStatus }) {
  return (
    <div className="space-y-5">
      {/* Delivery status */}
      {hasCallback && deliveryStatus === 'delivered' && (
        <div className="flex items-center justify-center gap-2 text-sm text-foreground py-2">
          <CheckCircle2 className="h-5 w-5 text-green-500" />
          Token delivered to CLI. You can close this tab.
        </div>
      )}

      {hasCallback && deliveryStatus === 'pending' && (
        <div className="flex items-center justify-center gap-2 text-sm text-muted-foreground py-2">
          <Loader2 className="h-4 w-4 animate-spin" />
          Sending token to CLI...
        </div>
      )}

      {/* Token display */}
      <div>
        <label className="block text-sm font-semibold mb-2">Connect token</label>
        <div className="flex items-center gap-2 p-3 rounded-xl border border-border bg-muted/30">
          <code className="flex-1 text-xs text-foreground font-mono break-all select-all line-clamp-3">
            {token}
          </code>
          <CopyButton text={token} className="shrink-0" />
        </div>
      </div>

      {/* Manual instructions (always shown as fallback, or when delivery failed) */}
      {(!hasCallback || deliveryStatus === 'failed') && (
        <div className="border border-border rounded-xl p-5 space-y-2">
          <p className="font-semibold">Next steps</p>
          <p className="text-sm text-muted-foreground">
            Install and configure Kyomi Connect in one command:
          </p>
          <div className="flex items-center gap-2 px-3 py-2 rounded-xl bg-muted/30 border border-border">
            <code className="text-xs text-foreground font-mono flex-1 break-all">
              curl -fsSL https://connect.kyomi.ai/install.sh | sh -s -- --token &quot;{token}&quot;
            </code>
            <CopyButton text={`curl -fsSL https://connect.kyomi.ai/install.sh | sh -s -- --token "${token}"`} className="shrink-0" />
          </div>
          <p className="text-xs text-muted-foreground mt-1">
            Already installed? Run: <code className="font-mono">kyomi-connect setup --token &lt;TOKEN&gt;</code>
          </p>
        </div>
      )}

      {hasCallback && deliveryStatus === 'failed' && (
        <p className="text-xs text-center text-muted-foreground">
          Could not reach the CLI automatically. Copy the token and paste it in your terminal.
        </p>
      )}
    </div>
  );
}
