// SPDX-License-Identifier: AGPL-3.0-or-later
import React, { useEffect, useState } from 'react';
import AgentThinking from '../AgentThinking';
import { MarkdownRenderer } from '../MarkdownRenderer';
import { Badge } from '../ui/badge';
import { XCircle } from 'lucide-react';
import { Spinner } from '../ui/spinner';
import ExecutionSelector from './ExecutionSelector';
import apiClient from '../../api/apiClient';

/**
 * ExecutionLogViewer - Shows watch execution as a chat-like conversation
 *
 * Displays:
 * - Watch prompt as a user message bubble
 * - Agent response as an assistant message bubble with expandable thinking
 *
 * Uses the exact same styling as ChatInterface for visual consistency.
 */
export default function ExecutionLogViewer({
  executions = [],        // List of executions (without trace)
  selectedExecution,      // Full execution with trace
  onSelectExecution,      // Callback when user selects a run
  isLoading = false,
  watchPrompt,            // The watch's monitoring instruction
}) {
  const [thinkingEvents, setThinkingEvents] = useState([]);
  const [loadingEvents, setLoadingEvents] = useState(false);

  // Fetch thinking events when selectedExecution changes
  useEffect(() => {
    async function fetchThinkingEvents() {
      if (!selectedExecution?.id || !selectedExecution?.watch_id) {
        setThinkingEvents([]);
        return;
      }

      setLoadingEvents(true);
      try {
        const response = await apiClient.get(
          `/api/v1/watches/${selectedExecution.watch_id}/executions/${selectedExecution.id}/thinking-events`
        );
        setThinkingEvents(response.data.events || []);
      } catch (error) {
        console.error('Failed to fetch thinking events:', error);
        setThinkingEvents([]);
      } finally {
        setLoadingEvents(false);
      }
    }

    fetchThinkingEvents();
  }, [selectedExecution?.id, selectedExecution?.watch_id]);
  // Get status badge
  const getStatusBadge = (status) => {
    switch (status) {
      case 'success':
        return <Badge variant="success">Alert Triggered</Badge>;
      case 'no_alert':
        return <Badge variant="secondary">No Alert</Badge>;
      case 'error':
        return <Badge variant="destructive">Error</Badge>;
      case 'running':
        return <Badge variant="info"><Spinner size="xs" className="mr-1" />Running</Badge>;
      default:
        return <Badge variant="outline">{status}</Badge>;
    }
  };

  // Format duration
  const formatDuration = (startedAt, completedAt) => {
    if (!startedAt || !completedAt) return null;
    const durationMs = new Date(completedAt) - new Date(startedAt);
    if (durationMs < 1000) return `${durationMs}ms`;
    return `${(durationMs / 1000).toFixed(1)}s`;
  };

  // Format timestamp for display
  const formatTimestamp = (dateStr) => {
    if (!dateStr) return '';
    const date = new Date(dateStr);
    return date.toLocaleString(undefined, {
      month: 'short',
      day: 'numeric',
      hour: '2-digit',
      minute: '2-digit',
    });
  };

  // No executions at all
  if (executions.length === 0 && !isLoading) {
    return (
      <div className="text-center py-8 text-muted-foreground">
        No executions yet. Run the watch to see execution logs.
      </div>
    );
  }

  // Loading state
  if (isLoading && !selectedExecution) {
    return (
      <div className="flex items-center justify-center py-8 gap-2 text-muted-foreground">
        <Spinner />
        <span>Loading execution...</span>
      </div>
    );
  }

  const events = thinkingEvents; // Fetched separately from /thinking-events endpoint
  const { status, error_message, agent_response, started_at, completed_at } = selectedExecution || {};
  const duration = formatDuration(started_at, completed_at);

  // Get the prompt to display (from selectedExecution or passed as prop)
  const promptToShow = watchPrompt || selectedExecution?.watch_prompt || 'Watch monitoring instruction';

  return (
    <div className="space-y-4">
      {/* Execution Selector */}
      {executions.length > 1 && (
        <ExecutionSelector
          executions={executions}
          selectedId={selectedExecution?.id}
          onSelect={onSelectExecution}
        />
      )}

      {/* Summary Header */}
      {selectedExecution && (
        <div className="flex items-center justify-between p-3 bg-muted/50 rounded-lg">
          <div className="flex items-center gap-3">
            {getStatusBadge(status)}
            <span className="text-xs text-muted-foreground">
              {formatTimestamp(started_at)}
            </span>
          </div>
          {duration && (
            <span className="text-xs text-muted-foreground">{duration}</span>
          )}
        </div>
      )}

      {/* Error Message */}
      {error_message && (
        <div className="p-3 bg-error/10 border border-error-border rounded-lg">
          <div className="flex items-start gap-2">
            <XCircle className="h-4 w-4 text-error-foreground mt-0.5 shrink-0" />
            <div>
              <p className="text-sm font-medium text-error-foreground">Execution Error</p>
              <p className="text-sm text-error-foreground/80 mt-1">{error_message}</p>
            </div>
          </div>
        </div>
      )}

      {/* Chat-like Conversation View */}
      {selectedExecution && (
        <div className="space-y-4 py-4">
          {/* User Message - Watch Prompt */}
          <div className="flex flex-col items-end">
            <div
              className="max-w-sm sm:max-w-md lg:max-w-lg xl:max-w-2xl px-4 py-3 text-primary-foreground bg-primary rounded-2xl shadow-sm text-sm"
            >
              {promptToShow}
            </div>
          </div>

          {/* Assistant Message - Response with Thinking Bubble */}
          <div className="flex flex-col items-start">
            <div className="w-full px-6 py-4 bg-card border border-border rounded-2xl shadow-sm overflow-hidden">
              {/* Thinking Bubble - Same as chat */}
              {events.length > 0 && (
                <AgentThinking
                  thinkingEvents={events}
                  isActive={status === 'running'}
                  variant="header-bar"
                />
              )}

              {/* Agent Response */}
              {agent_response ? (
                <MarkdownRenderer className="text-sm">
                  {agent_response}
                </MarkdownRenderer>
              ) : status === 'running' ? (
                <div className="flex items-center gap-2 text-muted-foreground">
                  <Spinner />
                  <span className="text-sm">Processing...</span>
                </div>
              ) : !error_message && (
                <p className="text-sm text-muted-foreground italic">
                  No response generated
                </p>
              )}
            </div>
          </div>
        </div>
      )}
    </div>
  );
}
