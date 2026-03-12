// SPDX-License-Identifier: AGPL-3.0-or-later
import { useState } from 'react';
import { useQueryClient } from '@tanstack/react-query';
import { Card, CardHeader, CardTitle, CardContent } from '../ui/card';
import { Button } from '../ui/button';
import { Badge } from '../ui/badge';
import { Clock, Eye, CheckCircle, Bell } from 'lucide-react';
import { ChartBarIcon } from '@heroicons/react/24/outline';
import { Spinner } from '../ui/spinner';
import apiClient from '../../api/apiClient';
import { describeCron } from '../../utils/cronUtils';

/**
 * WatchPreviewCard - Preview card shown in chat before creating/updating a watch
 *
 * Can work in two modes:
 * 1. Self-contained (onApprove): Calls API directly when approved, manages its own state
 * 2. Controlled (onConfirm): Parent controls the action, passes isCreating/created state
 *
 * @param {Object} preview - The watch preview data
 * @param {string} preview.name - Watch name
 * @param {string} preview.prompt - Monitoring instruction
 * @param {string} preview.schedule - Cron schedule
 * @param {string} preview.schedule - Cron schedule (UTC)
 * @param {Function} onApprove - Self-contained mode: called after successful creation with the watch data
 * @param {Function} onConfirm - Controlled mode: called when user clicks approve (parent handles API call)
 * @param {boolean} isCreating - Controlled mode: whether creation is in progress
 * @param {string} mode - 'create' or 'update'
 * @param {boolean} created - Controlled mode: whether the watch was successfully created
 */
export default function WatchPreviewCard({
  preview,
  onApprove,
  onConfirm,
  isCreating: externalIsCreating = false,
  mode = 'create',
  created: externalCreated = false,
}) {
  const queryClient = useQueryClient();

  // Internal state for self-contained mode
  const [internalIsCreating, setInternalIsCreating] = useState(false);
  const [internalCreated, setInternalCreated] = useState(false);
  const [error, setError] = useState(null);

  // Use external state if onConfirm is provided (controlled mode), otherwise use internal state
  // Always respect externalCreated if true (for parent-tracked accepted state)
  const isControlledMode = !!onConfirm;
  const isCreating = isControlledMode ? externalIsCreating : internalIsCreating;
  const created = externalCreated || internalCreated;

  if (!preview) return null;

  const { name, prompt, schedule, watch_id, queries, mode: watchMode = 'alert' } = preview;

  // Determine if this is an update or create based on presence of watch_id
  const isUpdate = !!watch_id;
  const actionMode = isUpdate ? 'update' : 'create';

  // Handle approval - either delegate to parent or call API directly
  const handleApprove = async () => {
    if (isControlledMode) {
      // Controlled mode - let parent handle it
      onConfirm();
      return;
    }

    // Self-contained mode - call API directly
    setInternalIsCreating(true);
    setError(null);

    try {
      let response;
      if (isUpdate) {
        // Update existing watch
        response = await apiClient.patch(`/api/v1/watches/${watch_id}`, {
          name,
          prompt,
          schedule,
          queries,
          mode: watchMode,
        });
      } else {
        // Create new watch
        response = await apiClient.post('/api/v1/watches', {
          name,
          prompt,
          schedule,
          queries,
          mode: watchMode,
        });
      }

      setInternalCreated(true);

      // Invalidate watches query to refresh the list
      queryClient.invalidateQueries(['watches']);

      // Notify parent if callback provided
      onApprove?.(response.data);
    } catch (err) {
      setError(err.response?.data?.detail || `Failed to ${actionMode} watch`);
      setInternalIsCreating(false);
    }
  };

  return (
    <Card className="border-primary/30 bg-primary/5 my-3">
      <CardHeader className="pb-2">
        <div className="flex items-center justify-between">
          <div className="flex items-center gap-2">
            <Eye className="h-4 w-4 text-primary" />
            <CardTitle className="text-base">Watch Preview</CardTitle>
          </div>
          <div className="flex items-center gap-2">
            {watchMode === 'report' ? (
              <Badge variant="default" className="text-xs gap-1">
                <ChartBarIcon className="h-3 w-3" />
                Report
              </Badge>
            ) : (
              <Badge variant="warning" className="text-xs gap-1">
                <Bell className="h-3 w-3" />
                Alert
              </Badge>
            )}
            <Badge variant="secondary" className="text-xs">
              {isUpdate ? 'Update' : 'New Watch'}
            </Badge>
          </div>
        </div>
      </CardHeader>
      <CardContent className="space-y-3">
        {/* Name */}
        <div>
          <p className="text-xs font-medium text-muted-foreground uppercase tracking-wider">Name</p>
          <p className="font-medium">{name}</p>
        </div>

        {/* Monitoring Instruction */}
        <div>
          <p className="text-xs font-medium text-muted-foreground uppercase tracking-wider">Monitoring</p>
          <p className="text-sm text-foreground whitespace-pre-wrap">{prompt}</p>
        </div>

        {/* Queries */}
        {queries && queries.length > 0 && (
          <div>
            <p className="text-xs font-medium text-muted-foreground uppercase tracking-wider mb-2">
              Reference Queries ({queries.length})
            </p>
            <div className="space-y-2 max-h-40 overflow-y-auto">
              {queries.map((q, idx) => (
                <div key={idx} className="flex items-start gap-2 p-2 rounded bg-muted border border-border">
                  <span className="text-muted-foreground mt-0.5 shrink-0 text-[10px] w-4">⚙️</span>
                  <div className="flex-1 min-w-0">
                    <p className="text-xs font-medium text-foreground break-words">{q.comment}</p>
                    <p className="text-[10px] text-muted-foreground font-mono mt-1 truncate">{q.sql}</p>
                    {q.datasource && (
                      <div className="mt-1">
                        <span className="inline-block px-1.5 py-0.5 rounded text-[9px] bg-accent text-foreground">
                          {q.datasource}
                        </span>
                      </div>
                    )}
                  </div>
                </div>
              ))}
            </div>
          </div>
        )}

        {/* Schedule */}
        <div className="flex items-center gap-2 text-sm">
          <Clock className="h-4 w-4 text-muted-foreground" />
          <span>{describeCron(schedule).description}</span>
        </div>

        {/* Error message */}
        {error && (
          <div className="text-sm text-error-foreground bg-error p-2 rounded">
            {error}
          </div>
        )}

        {/* Approve button */}
        <div className="pt-2 border-t border-border">
          <Button
            onClick={handleApprove}
            disabled={isCreating || created}
            className="w-full"
            variant={created ? 'secondary' : 'default'}
            size="sm"
          >
            {isCreating ? (
              <>
                <Spinner className="mr-2" />
                Accepting...
              </>
            ) : created ? (
              <>
                <CheckCircle className="h-4 w-4 mr-2" />
                Accepted
              </>
            ) : (
              <>
                <CheckCircle className="h-4 w-4 mr-2" />
                Accept
              </>
            )}
          </Button>
          {!created && (
            <p className="text-xs text-center text-muted-foreground mt-2">
              Or continue chatting to refine
            </p>
          )}
        </div>
      </CardContent>
    </Card>
  );
}
