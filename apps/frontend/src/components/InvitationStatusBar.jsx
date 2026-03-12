// SPDX-License-Identifier: AGPL-3.0-or-later
import React from 'react';
import { Users } from 'lucide-react';
import { UnifiedStatusBar } from '@/components/ui/unified-status-bar';
import { Button } from '@/components/ui/button';

/**
 * Invitation Status Bar Component
 *
 * Displays a status bar at the bottom of the application when the user has pending workspace invitations.
 * Allows user to accept or decline invitations directly from the notification.
 */
const InvitationStatusBar = ({
  invitations = [],
  onAccept = () => {},
  onDecline = () => {}
}) => {
  if (invitations.length === 0) {
    return null;
  }

  // Show only the first invitation (stack them if there are multiple)
  const invitation = invitations[0];
  const hasMultiple = invitations.length > 1;

  const getMessage = () => {
    const workspaceName = invitation.workspace_name || 'a workspace';
    const inviterName = invitation.invited_by_name;
    const roleText = invitation.role === 'admin' ? 'an admin' : 'a member';

    if (hasMultiple) {
      const byClause = inviterName ? ` by ${inviterName}` : '';
      return `You have ${invitations.length} pending workspace invitations. First: You have been invited${byClause} to join "${workspaceName}" as ${roleText}.`;
    }

    const byClause = inviterName ? ` by ${inviterName}` : '';
    return `You have been invited${byClause} to join "${workspaceName}" as ${roleText}.`;
  };

  const actions = (
    <>
      <Button
        onClick={() => onAccept(invitation.invitation_id)}
        variant="default"
        size="sm"
      >
        Accept
      </Button>
      <Button
        onClick={() => onDecline(invitation.invitation_id)}
        variant="outline"
        size="sm"
      >
        Decline
      </Button>
    </>
  );

  return (
    <UnifiedStatusBar
      variant="info"
      icon={<Users className="w-5 h-5" />}
      message={getMessage()}
      actions={actions}
    />
  );
};

export default InvitationStatusBar;
