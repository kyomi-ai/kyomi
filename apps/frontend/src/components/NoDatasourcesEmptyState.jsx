// SPDX-License-Identifier: AGPL-3.0-or-later
import { useNavigate } from 'react-router-dom';
import { Button } from './ui/button';
import { Server } from 'lucide-react';

/**
 * NoDatasourcesEmptyState - Shown when no datasources are configured
 *
 * Provides a friendly message and button to navigate to datasource settings.
 * Used in Chat, SQL Editor, and Dashboards pages.
 *
 * @param {string} context - Optional context for customizing the message (e.g., 'chat', 'sql', 'dashboards')
 */
export default function NoDatasourcesEmptyState({ context = 'default' }) {
  const navigate = useNavigate();

  const messages = {
    chat: {
      title: 'Connect a datasource to get started',
      description: 'The AI assistant needs access to your data to answer questions. Connect a database like PostgreSQL, BigQuery, or Snowflake to begin.',
    },
    sql: {
      title: 'No datasources configured',
      description: 'Connect a database to run SQL queries. Kyomi supports PostgreSQL, BigQuery, Snowflake, ClickHouse, and more.',
    },
    dashboards: {
      title: 'Connect a datasource first',
      description: 'Dashboards display visualizations from your data. Connect a database to create charts and dashboards.',
    },
    default: {
      title: 'No datasources configured',
      description: 'Connect a database to start analyzing your data with Kyomi.',
    },
  };

  const { title, description } = messages[context] || messages.default;

  return (
    <div className="flex flex-col items-center justify-center h-full w-full p-8 bg-muted">
      <div className="max-w-md w-full bg-card border border-border rounded-xl p-8 shadow-sm text-center">
        <div className="inline-flex items-center justify-center w-16 h-16 rounded-full bg-primary/10 mx-auto mb-6">
          <Server className="w-8 h-8 text-primary" />
        </div>

        <h2 className="text-2xl font-semibold text-foreground mb-3">
          {title}
        </h2>

        <p className="text-muted-foreground mb-6">
          {description}
        </p>

        <Button
          onClick={() => navigate('/settings/datasources')}
          size="lg"
          className="w-full"
        >
          <Server className="w-5 h-5 mr-2" />
          Connect Datasource
        </Button>

        <p className="text-xs text-muted-foreground mt-4">
          You can add more datasources anytime in Settings.
        </p>
      </div>
    </div>
  );
}
