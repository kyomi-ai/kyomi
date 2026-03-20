// SPDX-License-Identifier: AGPL-3.0-or-later
import { useState, useEffect } from 'react';
import { Eye, EyeOff, Zap } from 'lucide-react';
import { Button } from '../ui/button';
import { Input } from '../ui/input';
import { Label } from '../ui/label';
import { Select, SelectTrigger, SelectValue, SelectContent, SelectItem } from '../ui/select';
import { Alert, AlertDescription } from '../ui/alert';
import { Spinner } from '../ui/spinner';
import { toast } from '../../lib/toast';

const STORAGE_KEY = 'kyomi_llm_config';

const PROVIDERS = {
  anthropic: {
    label: 'Anthropic',
    defaultModel: 'claude-sonnet-4-20250514',
    defaultBaseUrl: 'https://api.anthropic.com',
  },
  openai: {
    label: 'OpenAI',
    defaultModel: 'gpt-4o',
    defaultBaseUrl: 'https://api.openai.com/v1',
  },
  gemini: {
    label: 'Gemini',
    defaultModel: 'gemini-2.5-pro',
    defaultBaseUrl: 'https://generativelanguage.googleapis.com/v1beta',
  },
};

function loadConfig() {
  try {
    const stored = localStorage.getItem(STORAGE_KEY);
    if (stored) {
      return JSON.parse(stored);
    }
  } catch {
    // Corrupted data — ignore
  }
  return null;
}

function saveConfig(config) {
  localStorage.setItem(STORAGE_KEY, JSON.stringify(config));
}

export default function AIProviderSettings() {
  const [provider, setProvider] = useState('anthropic');
  const [apiKey, setApiKey] = useState('');
  const [modelOverride, setModelOverride] = useState('');
  const [baseUrlOverride, setBaseUrlOverride] = useState('');
  const [showApiKey, setShowApiKey] = useState(false);
  const [testing, setTesting] = useState(false);

  // Load saved config on mount
  useEffect(() => {
    const config = loadConfig();
    if (config) {
      setProvider(config.provider || 'anthropic');
      setApiKey(config.api_key || '');
      setModelOverride(config.model_override || '');
      setBaseUrlOverride(config.base_url_override || '');
    }
  }, []);

  const selectedProvider = PROVIDERS[provider];

  const handleTestConnection = async () => {
    if (!apiKey.trim()) {
      toast.error('Please enter an API key first.');
      return;
    }

    setTesting(true);
    try {
      // The backend endpoint doesn't exist yet — show a placeholder toast
      toast.info('Connection test is not yet available. Save your config and it will be validated when first used.');
    } finally {
      setTesting(false);
    }
  };

  const handleSave = () => {
    if (!apiKey.trim()) {
      toast.error('Please enter an API key.');
      return;
    }

    const config = {
      provider,
      api_key: apiKey.trim(),
      model_override: modelOverride.trim() || null,
      base_url_override: baseUrlOverride.trim() || null,
    };

    saveConfig(config);
    toast.success('AI provider configuration saved.');
  };

  return (
    <div className="space-y-4">
      <Alert>
        <AlertDescription>
          Optional — only needed for built-in chat and automated watches. MCP tools work without an API key.
        </AlertDescription>
      </Alert>

      {/* Provider */}
      <div className="space-y-2">
        <Label>Provider</Label>
        <Select value={provider} onValueChange={setProvider}>
          <SelectTrigger>
            <SelectValue />
          </SelectTrigger>
          <SelectContent>
            {Object.entries(PROVIDERS).map(([key, { label }]) => (
              <SelectItem key={key} value={key}>
                {label}
              </SelectItem>
            ))}
          </SelectContent>
        </Select>
      </div>

      {/* API Key */}
      <div className="space-y-2">
        <Label>API Key</Label>
        <div className="relative">
          <Input
            type={showApiKey ? 'text' : 'password'}
            value={apiKey}
            onChange={(e) => setApiKey(e.target.value)}
            placeholder={`Enter your ${selectedProvider.label} API key`}
            className="pr-10"
          />
          <button
            type="button"
            onClick={() => setShowApiKey(!showApiKey)}
            className="absolute right-2 top-1/2 -translate-y-1/2 p-1 text-muted-foreground hover:text-foreground transition-colors"
            aria-label={showApiKey ? 'Hide API key' : 'Show API key'}
          >
            {showApiKey ? <EyeOff className="h-4 w-4" /> : <Eye className="h-4 w-4" />}
          </button>
        </div>
      </div>

      {/* Model Override */}
      <div className="space-y-2">
        <Label>Model Override <span className="text-muted-foreground font-normal">(optional)</span></Label>
        <Input
          type="text"
          value={modelOverride}
          onChange={(e) => setModelOverride(e.target.value)}
          placeholder={selectedProvider.defaultModel}
        />
        <p className="text-xs text-muted-foreground">
          Leave blank to use the default model for the selected provider.
        </p>
      </div>

      {/* Base URL Override */}
      <div className="space-y-2">
        <Label>Base URL Override <span className="text-muted-foreground font-normal">(optional)</span></Label>
        <Input
          type="text"
          value={baseUrlOverride}
          onChange={(e) => setBaseUrlOverride(e.target.value)}
          placeholder={selectedProvider.defaultBaseUrl}
        />
        <p className="text-xs text-muted-foreground">
          Override the API base URL for proxies or custom endpoints.
        </p>
      </div>

      {/* Actions */}
      <div className="flex items-center gap-3 pt-2">
        <Button onClick={handleSave}>
          Save
        </Button>
        <Button variant="outline" onClick={handleTestConnection} disabled={testing}>
          {testing ? (
            <>
              <Spinner size="sm" className="mr-2" />
              Testing...
            </>
          ) : (
            <>
              <Zap className="h-4 w-4 mr-2" />
              Test Connection
            </>
          )}
        </Button>
      </div>
    </div>
  );
}
