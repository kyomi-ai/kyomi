// SPDX-License-Identifier: AGPL-3.0-or-later
/**
 * Feedback Wrapper Component
 *
 * Handles:
 * - Initializing the feedback context collector on mount
 * - Tracking route changes for page visit history
 *
 * Note: Feedback button is in the user menu (Sidebar.jsx)
 *
 * Usage:
 *   Wrap authenticated routes with this component to enable feedback functionality.
 */

import { useEffect } from 'react';
import { useLocation } from 'react-router-dom';
import { feedbackContext } from '../../lib/feedbackContext';

export default function FeedbackWrapper({ children }) {
  const location = useLocation();

  // Initialize feedback context once on mount
  useEffect(() => {
    feedbackContext.init();
  }, []);

  // Track page visits when route changes
  useEffect(() => {
    feedbackContext.addPageVisit(location.pathname);
  }, [location.pathname]);

  return children;
}
