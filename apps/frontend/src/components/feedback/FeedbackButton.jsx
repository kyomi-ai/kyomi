// SPDX-License-Identifier: AGPL-3.0-or-later
/**
 * Feedback Button Component
 *
 * A floating button that appears in the bottom-right corner of the app,
 * allowing users to quickly open the feedback modal.
 *
 * Only visible when the user is authenticated.
 */

import { useState } from 'react';
import { MessageSquareMore } from 'lucide-react';
import { Button } from '../ui/button';
import { useAuth } from '../../context/AuthContext';
import FeedbackModal from './FeedbackModal';

export default function FeedbackButton() {
  const [isOpen, setIsOpen] = useState(false);
  const { isAuthenticated } = useAuth();

  // Only show feedback button for authenticated users
  if (!isAuthenticated) {
    return null;
  }

  return (
    <>
      <Button
        variant="outline"
        size="sm"
        className="feedback-button fixed bottom-4 right-4 z-50 shadow-lg bg-background hover:bg-accent"
        onClick={() => setIsOpen(true)}
        aria-label="Send feedback"
      >
        <MessageSquareMore className="h-4 w-4 mr-2" />
        Feedback
      </Button>

      <FeedbackModal open={isOpen} onOpenChange={setIsOpen} />
    </>
  );
}
