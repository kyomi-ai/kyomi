// SPDX-License-Identifier: AGPL-3.0-or-later
import { useState, useEffect } from 'react';
import { useNavigate } from 'react-router-dom';
import { useAuth } from '../context/AuthContext';
import { Card, CardHeader, CardTitle, CardDescription, CardContent } from '../components/ui/card';
import { Button } from '../components/ui/button';
import { Copy, ExternalLink, Database, ArrowRight } from 'lucide-react';
import { toast } from '../lib/toast';

/**
 * PersonalSetupWizard - First-run setup for personal (desktop) mode.
 *
 * Two steps:
 *  1. Connect Data — navigate to /onboarding or skip with sample data
 *  2. Connect AI Tool — show MCP connection instructions for Claude Code,
 *     Claude Desktop, and Cursor
 */
export default function PersonalSetupWizard() {
  const navigate = useNavigate();
  const { apiClient } = useAuth();
  const [hasDatasources, setHasDatasources] = useState(null); // null = loading
  const [activeTab, setActiveTab] = useState('claude-code');

  // Check whether any datasources already exist
  useEffect(() => {
    if (!apiClient) return;

    const check = async () => {
      try {
        const response = await apiClient.get('/api/v1/datasources');
        const datasources = response.data || [];
        setHasDatasources(datasources.length > 0);
      } catch {
        // If the request fails, assume no datasources yet
        setHasDatasources(false);
      }
    };

    check();
  }, [apiClient]);

  const copyToClipboard = (text) => {
    navigator.clipboard.writeText(text);
    toast.success('Copied to clipboard!');
  };

  const port = window.location.port || '3000';
  const mcpUrl = `http://localhost:${port}/mcp`;

  const claudeDesktopConfig = JSON.stringify(
    { mcpServers: { kyomi: { url: mcpUrl } } },
    null,
    2,
  );

  const cursorConfig = btoa(JSON.stringify({ type: 'http', url: mcpUrl }));
  const cursorDeepLink = `cursor://anysphere.cursor-deeplink/mcp/install?name=kyomi&config=${cursorConfig}`;

  const cursorManualConfig = JSON.stringify(
    { kyomi: { type: 'http', url: mcpUrl } },
    null,
    2,
  );

  // Loading state
  if (hasDatasources === null) {
    return (
      <div className="min-h-screen bg-background flex items-center justify-center">
        <div className="text-muted-foreground">Loading...</div>
      </div>
    );
  }

  // ── Step 1: Connect Data ──────────────────────────────────────────────
  if (!hasDatasources) {
    return (
      <div className="min-h-screen flex items-center justify-center bg-background p-4">
        <Card className="max-w-xl w-full">
          <CardHeader className="text-center">
            <CardTitle className="text-2xl">Connect Your Data</CardTitle>
            <CardDescription>
              Kyomi works best when it can query your data directly. Choose how to get started.
            </CardDescription>
          </CardHeader>
          <CardContent className="space-y-4">
            <div className="border border-border rounded-xl p-5">
              <div className="flex items-start gap-4">
                <Database className="h-6 w-6 mt-0.5 text-muted-foreground flex-shrink-0" />
                <div className="flex-1">
                  <h3 className="font-semibold mb-1">Connect a Database</h3>
                  <p className="text-sm text-muted-foreground mb-3">
                    Connect your database to ask questions about your real data
                  </p>
                  <Button onClick={() => navigate('/onboarding')} className="w-full">
                    Connect Datasource
                  </Button>
                </div>
              </div>
            </div>

            <div className="border border-border rounded-xl p-5">
              <div className="flex items-start gap-4">
                <Database className="h-6 w-6 mt-0.5 text-muted-foreground flex-shrink-0" />
                <div className="flex-1">
                  <h3 className="font-semibold mb-1">Explore with Sample Data</h3>
                  <p className="text-sm text-muted-foreground mb-3">
                    Skip setup and explore Kyomi right away
                  </p>
                  <Button variant="outline" onClick={() => navigate('/')} className="w-full">
                    Skip for Now
                  </Button>
                </div>
              </div>
            </div>

            <p className="text-xs text-center text-muted-foreground pt-2">
              You can always add datasources later in Settings
            </p>
          </CardContent>
        </Card>
      </div>
    );
  }

  // ── Step 2: Connect Your AI Tool ──────────────────────────────────────
  const tabs = [
    { id: 'claude-code', label: 'Claude Code' },
    { id: 'claude-desktop', label: 'Claude Desktop' },
    { id: 'cursor', label: 'Cursor' },
  ];

  return (
    <div className="min-h-screen flex items-center justify-center bg-background p-4">
      <Card className="max-w-2xl w-full">
        <CardHeader className="text-center">
          <CardTitle className="text-2xl">Connect Kyomi to Your AI Tool</CardTitle>
          <CardDescription>
            Add Kyomi as an MCP server so your AI assistant can query your data.
          </CardDescription>
        </CardHeader>
        <CardContent className="space-y-6">
          {/* Tab bar */}
          <div className="flex border-b border-border">
            {tabs.map((tab) => (
              <button
                key={tab.id}
                onClick={() => setActiveTab(tab.id)}
                className={`px-4 py-2 text-sm font-medium transition-colors -mb-px ${
                  activeTab === tab.id
                    ? 'border-b-2 border-primary text-foreground'
                    : 'text-muted-foreground hover:text-foreground'
                }`}
              >
                {tab.label}
              </button>
            ))}
          </div>

          {/* Tab content */}
          {activeTab === 'claude-code' && (
            <div className="space-y-4">
              <div className="space-y-2">
                <h4 className="font-medium text-foreground text-sm">Run this command:</h4>
                <div className="relative">
                  <pre className="p-4 bg-muted rounded-md text-sm overflow-x-auto pr-12">
                    {`claude mcp add --transport http kyomi ${mcpUrl}`}
                  </pre>
                  <Button
                    size="sm"
                    variant="ghost"
                    className="absolute top-2 right-2"
                    onClick={() => copyToClipboard(`claude mcp add --transport http kyomi ${mcpUrl}`)}
                  >
                    <Copy className="h-4 w-4" />
                  </Button>
                </div>
              </div>
              <div className="space-y-2">
                <h4 className="font-medium text-foreground text-sm">Or add to your config:</h4>
                <div className="relative">
                  <pre className="p-4 bg-muted rounded-md text-sm overflow-x-auto pr-12">
                    {JSON.stringify(
                      { mcpServers: { kyomi: { type: 'http', url: mcpUrl } } },
                      null,
                      2,
                    )}
                  </pre>
                  <Button
                    size="sm"
                    variant="ghost"
                    className="absolute top-2 right-2"
                    onClick={() =>
                      copyToClipboard(
                        JSON.stringify(
                          { mcpServers: { kyomi: { type: 'http', url: mcpUrl } } },
                          null,
                          2,
                        ),
                      )
                    }
                  >
                    <Copy className="h-4 w-4" />
                  </Button>
                </div>
              </div>
            </div>
          )}

          {activeTab === 'claude-desktop' && (
            <div className="space-y-2">
              <h4 className="font-medium text-foreground text-sm">
                Add to claude_desktop_config.json:
              </h4>
              <div className="relative">
                <pre className="p-4 bg-muted rounded-md text-sm overflow-x-auto pr-12">
                  {claudeDesktopConfig}
                </pre>
                <Button
                  size="sm"
                  variant="ghost"
                  className="absolute top-2 right-2"
                  onClick={() => copyToClipboard(claudeDesktopConfig)}
                >
                  <Copy className="h-4 w-4" />
                </Button>
              </div>
            </div>
          )}

          {activeTab === 'cursor' && (
            <div className="space-y-4">
              <div className="space-y-2">
                <h4 className="font-medium text-foreground text-sm">One-click install:</h4>
                <Button
                  variant="outline"
                  onClick={() => window.open(cursorDeepLink, '_blank')}
                >
                  <ExternalLink className="h-4 w-4 mr-2" />
                  Connect with Cursor
                </Button>
              </div>
              <div className="space-y-2">
                <h4 className="font-medium text-foreground text-sm">Or add manually to .cursor/mcp.json:</h4>
                <div className="relative">
                  <pre className="p-4 bg-muted rounded-md text-sm overflow-x-auto pr-12">
                    {cursorManualConfig}
                  </pre>
                  <Button
                    size="sm"
                    variant="ghost"
                    className="absolute top-2 right-2"
                    onClick={() => copyToClipboard(cursorManualConfig)}
                  >
                    <Copy className="h-4 w-4" />
                  </Button>
                </div>
              </div>
            </div>
          )}

          {/* Actions */}
          <div className="space-y-3 pt-2">
            <Button onClick={() => navigate('/dashboards')} className="w-full">
              I've Connected
              <ArrowRight className="h-4 w-4 ml-2" />
            </Button>
            <p className="text-sm text-center">
              <button
                onClick={() => navigate('/settings')}
                className="text-muted-foreground hover:text-foreground transition-colors"
              >
                Or use Kyomi's built-in chat instead &rarr;
              </button>
            </p>
          </div>
        </CardContent>
      </Card>
    </div>
  );
}
