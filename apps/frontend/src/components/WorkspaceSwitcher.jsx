// SPDX-License-Identifier: AGPL-3.0-or-later
import { Check } from 'lucide-react';
import { Badge } from '@/components/ui/badge';

export default function WorkspaceSwitcher({ workspaces, currentWorkspaceId, onSwitch, onClose }) {
  // Sort workspaces alphabetically by name for consistent ordering (handle null names)
  const sortedWorkspaces = [...workspaces].sort((a, b) =>
    (a.name || 'Unnamed Workspace').localeCompare(b.name || 'Unnamed Workspace')
  );

  const handleSwitch = async (workspaceId) => {
    if (workspaceId !== currentWorkspaceId) {
      await onSwitch(workspaceId);
      if (onClose) onClose();
    } else {
    }
  };

  const getTierBadgeVariant = (tier) => {
    switch (tier) {
      case 'enterprise':
        return 'default'; // primary color for enterprise
      case 'team':
        return 'secondary';
      default:
        return 'outline';
    }
  };

  const formatWorkspaceName = (workspace) => {
    return workspace.name || 'Workspace';
  };

  return (
    <>
      {/* All workspaces in alphabetical order */}
      {sortedWorkspaces.map((workspace, index) => {
        const isActive = workspace.workspace_id === currentWorkspaceId;

        return (
          <button
            key={workspace.workspace_id}
            onClick={() => handleSwitch(workspace.workspace_id)}
            className={`w-full px-4 py-2 text-left transition-colors flex items-center justify-between group ${
              isActive ? 'bg-accent/50' : 'hover:bg-accent'
            }`}
          >
            <div className="flex items-center gap-2 min-w-0 flex-1">
              {isActive ? (
                <Check className="w-4 h-4 flex-shrink-0 text-primary" />
              ) : (
                <div className="w-4 h-4 flex-shrink-0" />
              )}
              <div className="min-w-0 flex-1">
                <div className="text-sm font-medium text-foreground truncate">
                  {formatWorkspaceName(workspace)}
                </div>
                <div className="text-xs text-muted-foreground truncate">
                  {workspace.member_count} member{workspace.member_count !== 1 ? 's' : ''}
                </div>
              </div>
            </div>
            <Badge variant={getTierBadgeVariant(workspace.subscription_tier)} className="text-xs">
              {workspace.subscription_tier}
            </Badge>
          </button>
        );
      })}
      <div className="border-b border-border my-1" />
    </>
  );
}
