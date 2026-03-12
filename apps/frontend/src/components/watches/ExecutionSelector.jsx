// SPDX-License-Identifier: AGPL-3.0-or-later
import React from 'react';
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from '../ui/select';
import { Badge } from '../ui/badge';

/**
 * ExecutionSelector - Dropdown to select which watch execution run to view
 *
 * Shows all executions in reverse chronological order with status badge and timestamp.
 */
export default function ExecutionSelector({ executions, selectedId, onSelect }) {
  if (!executions?.length) return null;

  const formatDate = (dateStr) => {
    if (!dateStr) return 'Unknown';
    const date = new Date(dateStr);
    return date.toLocaleString(undefined, {
      month: 'short',
      day: 'numeric',
      hour: '2-digit',
      minute: '2-digit',
    });
  };

  const getStatusVariant = (status) => {
    switch (status) {
      case 'success':
        return 'success';
      case 'error':
        return 'destructive';
      case 'no_alert':
        return 'secondary';
      case 'running':
        return 'info';
      default:
        return 'outline';
    }
  };

  const getStatusLabel = (status) => {
    switch (status) {
      case 'success':
        return 'Alert';
      case 'no_alert':
        return 'No Alert';
      case 'error':
        return 'Error';
      case 'running':
        return 'Running';
      default:
        return status;
    }
  };

  // Use first execution if none selected
  const currentId = selectedId || executions[0]?.id;

  return (
    <div className="flex items-center gap-2">
      <span className="text-sm text-muted-foreground">Execution run:</span>
      <Select
        value={String(currentId)}
        onValueChange={(v) => onSelect(parseInt(v))}
      >
        <SelectTrigger className="w-[280px]">
          <SelectValue placeholder="Select execution" />
        </SelectTrigger>
        <SelectContent>
          {executions.map((exec, index) => (
            <SelectItem key={exec.id} value={String(exec.id)}>
              <div className="flex items-center gap-2">
                <Badge
                  variant={getStatusVariant(exec.status)}
                  className="text-xs px-1.5 py-0"
                >
                  {getStatusLabel(exec.status)}
                </Badge>
                <span className="text-sm">{formatDate(exec.started_at)}</span>
                {index === 0 && (
                  <span className="text-xs text-muted-foreground">(latest)</span>
                )}
              </div>
            </SelectItem>
          ))}
        </SelectContent>
      </Select>
    </div>
  );
}
