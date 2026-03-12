// SPDX-License-Identifier: AGPL-3.0-or-later
import { useState } from 'react';
import { AlertTriangle } from 'lucide-react';
import { Button } from '@/components/ui/button';
import { Alert, AlertTitle, AlertDescription } from '@/components/ui/alert';
import { DEPLOYMENT_TABS, getTabContent } from './connectDeploymentCommands';
import CopyButton from './CopyButton';

/**
 * ConnectSetup - Tabbed deployment instructions shown after creating a Connect datasource.
 *
 * Shows the connect token with a copy button, and tabbed instructions for
 * Docker, Linux, Kubernetes, and Docker Compose deployment methods.
 *
 * @param {string} token - The connect token for this datasource
 * @param {string} datasourceName - The name of the created datasource
 * @param {string} datasourceType - The underlying database type (postgres, mysql, etc.)
 * @param {function} onDone - Callback when user clicks Done
 */
export default function ConnectSetup({ token, datasourceName, datasourceType, onDone }) {
  const [activeTab, setActiveTab] = useState('linux');

  const content = getTabContent(activeTab, token, datasourceType);

  return (
    <div className="space-y-5">
      {/* Header */}
      <div className="space-y-1">
        <h3 className="text-lg font-semibold text-foreground">
          Deploy Kyomi Connect
        </h3>
        <p className="text-sm text-muted-foreground">
          Install the Connect agent to bridge <span className="font-medium text-foreground">{datasourceName}</span> to Kyomi.
        </p>
      </div>

      {/* Token display */}
      <div className="space-y-1.5">
        <label className="block text-sm font-medium text-foreground">
          Connect Token
        </label>
        <div className="group flex items-center gap-2 rounded-lg border border-border bg-muted/50 px-3 py-2">
          <code className="flex-1 text-xs font-mono text-foreground truncate select-all">
            {token}
          </code>
          <CopyButton text={token} className="opacity-0 group-hover:opacity-100 shrink-0" />
        </div>
      </div>

      {/* Security warning */}
      <Alert variant="warning">
        <AlertTriangle className="h-4 w-4" />
        <AlertTitle>Keep this token secret</AlertTitle>
        <AlertDescription>
          This token grants access to your datasource configuration. Do not share it publicly or commit it to version control.
        </AlertDescription>
      </Alert>

      {/* Tabbed deployment instructions */}
      <DeploymentTabs token={token} datasourceType={datasourceType} activeTab={activeTab} setActiveTab={setActiveTab} />

      {/* Done button */}
      <div className="flex justify-end pt-2">
        <Button onClick={onDone}>
          Done
        </Button>
      </div>
    </div>
  );
}

/**
 * DeploymentTabs - Reusable tabbed deployment instructions.
 * Used by both ConnectSetup (creation) and ConnectStatus (after token rotation).
 */
export function DeploymentTabs({ token, datasourceType, activeTab, setActiveTab }) {
  const content = getTabContent(activeTab, token, datasourceType);

  return (
    <div>
      {/* Tab buttons */}
      <div className="flex border-b border-border">
        {DEPLOYMENT_TABS.map((tab) => (
          <button
            key={tab.id}
            onClick={() => setActiveTab(tab.id)}
            className={`px-4 py-2 text-sm font-medium border-b-2 -mb-px transition-colors ${
              activeTab === tab.id
                ? 'border-primary text-primary'
                : 'border-transparent text-muted-foreground hover:text-foreground'
            }`}
          >
            {tab.label}
          </button>
        ))}
      </div>

      {/* Tab content */}
      <div className="group relative">
        <pre className="rounded-lg border border-t-0 border-border bg-muted/50 p-4 text-sm font-mono text-foreground overflow-x-auto whitespace-pre">
          {content}
        </pre>
        <div className="absolute top-2 right-2 opacity-0 group-hover:opacity-100 transition-opacity">
          <CopyButton text={content} />
        </div>
      </div>
    </div>
  );
}
