// SPDX-License-Identifier: AGPL-3.0-or-later
import React from 'react';
import { Crown } from 'lucide-react';
import { UnifiedStatusBar } from '@/components/ui/unified-status-bar';
import { Button } from '@/components/ui/button';
import { useNavigate } from 'react-router-dom';

/**
 * Ownership Transfer Status Bar Component
 *
 * Displays a status bar at the bottom of the application when the user has pending ownership transfer offers.
 * Directs user to Team settings page to review and accept/decline the transfer.
 */
const OwnershipTransferStatusBar = ({
  transfers = [],
  onDismiss = () => {}
}) => {
  const navigate = useNavigate();

  if (transfers.length === 0) {
    return null;
  }

  // Show only the first transfer (stack them if there are multiple)
  const transfer = transfers[0];
  const hasMultiple = transfers.length > 1;

  const getMessage = () => {
    const workspaceName = transfer.workspace_name || 'a workspace';
    const fromUserEmail = transfer.from_user_email;

    if (hasMultiple) {
      const byClause = fromUserEmail ? ` from ${fromUserEmail}` : '';
      return `You have ${transfers.length} pending ownership transfer offers. First: You have been offered ownership${byClause} of workspace "${workspaceName}".`;
    }

    const byClause = fromUserEmail ? ` from ${fromUserEmail}` : '';
    return `You have been offered ownership${byClause} of workspace "${workspaceName}".`;
  };

  const actions = (
    <>
      <Button
        onClick={() => navigate(`/accept-ownership/${transfer.transfer_id}`)}
        variant="default"
        size="sm"
      >
        Review & Accept
      </Button>
      <Button
        onClick={() => onDismiss(transfer.transfer_id)}
        variant="outline"
        size="sm"
      >
        Dismiss
      </Button>
    </>
  );

  return (
    <UnifiedStatusBar
      variant="warning"
      icon={<Crown className="w-5 h-5" />}
      message={getMessage()}
      actions={actions}
    />
  );
};

export default OwnershipTransferStatusBar;
