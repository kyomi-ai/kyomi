// SPDX-License-Identifier: AGPL-3.0-or-later
import { driver } from 'driver.js';
import 'driver.js/dist/driver.css';
import { useState, useEffect } from 'react';
import apiClient from '../api/apiClient';

/**
 * Product Tour System
 *
 * Manages multiple independent tours that trigger when users first encounter features.
 * Each tour is shown only once and tracked in the backend (persists across browsers).
 * Falls back to localStorage if backend is unavailable.
 */

// Tour definitions - add new tours here
const TOURS = {
  firstChart: {
    id: 'first_chart',
    storageKey: 'kyomi_tour_first_chart',
    getSteps: (messageId) => [
      {
        element: document.querySelector(`#message-${messageId}`)?.parentElement?.querySelector('.flex.items-center.gap-3'),
        popover: {
          title: 'Save Your Work 💡',
          description: `
            <div style="line-height: 1.6;">
              <p style="margin-bottom: 12px;"><strong>⭐ Pin Message:</strong> Save important responses to find them quickly later. Pinned messages appear at the top when you filter.</p>
              <p><strong>💾 Save to Dashboard:</strong> Turn this chat into an interactive dashboard you can share with your team.</p>
            </div>
          `,
          side: 'top',
          align: 'center'
        }
      }
    ]
  },

  agentThinking: {
    id: 'agent_thinking',
    storageKey: 'kyomi_tour_agent_thinking',
    getSteps: () => [
      {
        element: document.querySelector('[data-testid="agent-thinking"]'),
        popover: {
          title: 'See How I Think 🧠',
          description: `
            <div style="line-height: 1.6;">
              <p style="margin-bottom: 12px;"><strong>Click to expand</strong> and see my step-by-step reasoning process.</p>
              <p>Watch me analyze your question, query data sources, and build the answer in real-time!</p>
            </div>
          `,
          side: 'bottom',
          align: 'start'
        }
      }
    ]
  },

  sqlEditor: {
    id: 'sql_editor',
    storageKey: 'kyomi_tour_sql_editor',
    getSteps: () => [
      {
        element: document.querySelector('button[aria-label="History"]'),
        popover: {
          title: 'Query History 📜',
          description: `
            <div style="line-height: 1.6;">
              <p style="margin-bottom: 12px;"><strong>Click here</strong> to view your query history.</p>
              <p>All queries are auto-saved. <strong>⭐ Star queries</strong> to keep them from auto-deleting after 30 days.</p>
            </div>
          `,
          side: 'left',
          align: 'start'
        }
      }
    ]
  },

  dashboardEditor: {
    id: 'dashboard_editor',
    storageKey: 'kyomi_tour_dashboard_editor',
    getSteps: () => [
      {
        element: document.querySelector('button[aria-label="Split view"]'),
        popover: {
          title: 'Preview & Edit 👁️',
          description: `
            <div style="line-height: 1.6;">
              <p style="margin-bottom: 12px;">Toggle between editing, split, and preview modes.</p>
              <p>The editor supports <strong>ChartML</strong> for creating interactive charts and parameters.</p>
            </div>
          `,
          side: 'bottom',
          align: 'end'
        }
      }
    ]
  },

  dashboardChartEdit: {
    id: 'dashboard_chart_edit',
    storageKey: 'kyomi_tour_dashboard_chart_edit',
    getSteps: () => [
      {
        element: document.querySelector('button[aria-label="Edit chart"]'),
        popover: {
          title: 'Edit Charts ✏️',
          description: `
            <div style="line-height: 1.6;">
              <p style="margin-bottom: 12px;"><strong>Click to edit</strong> this chart directly in the visual editor.</p>
              <p>Change chart types, colors, and data queries without writing ChartML!</p>
            </div>
          `,
          side: 'bottom',
          align: 'end'
        }
      }
    ]
  },

  chartCopilot: {
    id: 'chart_copilot',
    storageKey: 'kyomi_tour_chart_copilot',
    getSteps: () => [
      {
        element: document.querySelector('input[placeholder*="Ask me to modify your chart"]'),
        popover: {
          title: 'AI Chart Copilot 🪄',
          description: `
            <div style="line-height: 1.6;">
              <p style="margin-bottom: 12px;"><strong>Ask the AI to modify your chart</strong> using natural language.</p>
              <p>Try: "change to line chart", "add a goal line at 100", or "make it blue"!</p>
            </div>
          `,
          side: 'top',
          align: 'start'
        }
      }
    ]
  }
};

/**
 * Hook for managing product tours
 * Returns functions to show tours and check if they've been seen
 */
export const useProductTour = () => {
  const [toursCompleted, setToursCompleted] = useState({});
  const [isLoaded, setIsLoaded] = useState(false);

  // Fetch tour status from backend on mount
  useEffect(() => {
    const fetchTourStatus = async () => {
      try {
        const response = await apiClient.get('/api/v1/users/me/tours');
        setToursCompleted(response.data.tours_completed || {});
        setIsLoaded(true);
      } catch (error) {
        // Don't show tours if backend is unavailable
        setIsLoaded(true);
        setToursCompleted({});
      }
    };

    fetchTourStatus();
  }, []);

  /**
   * Check if a specific tour has been completed
   */
  const hasSeen = (tourName) => {
    const tour = TOURS[tourName];
    if (!tour) {
      return true; // Don't show unknown tours
    }

    // While loading, don't show tours (avoid flash before backend state loads)
    if (!isLoaded) {
      return true;
    }

    // Backend is the source of truth
    return toursCompleted[tour.id] === true;
  };

  /**
   * Mark a tour as completed
   */
  const markAsSeen = async (tourName) => {
    const tour = TOURS[tourName];
    if (!tour) {
      return;
    }

    // Update local state immediately (optimistic update)
    setToursCompleted(prev => ({ ...prev, [tour.id]: true }));

    // Update backend
    try {
      await apiClient.post(`/api/v1/users/me/tours/${tour.id}`);
    } catch (error) {
      // Revert local state since backend failed
      setToursCompleted(prev => {
        const updated = { ...prev };
        delete updated[tour.id];
        return updated;
      });
    }
  };

  /**
   * Show a specific tour
   *
   * @param {string} tourName - Name of the tour from TOURS object
   * @param {any} context - Optional context data passed to getSteps()
   */
  const showTour = (tourName, context) => {
    const tour = TOURS[tourName];

    if (!tour) {
      return;
    }

    // Check if already seen
    if (hasSeen(tourName)) {
      return;
    }

    // Small delay to ensure DOM is ready
    setTimeout(() => {
      const steps = tour.getSteps(context);

      // Validate that target elements exist
      const validSteps = steps.filter(step => {
        if (!step.element) {
          return false;
        }
        return true;
      });

      if (validSteps.length === 0) {
        return;
      }

      const driverObj = driver({
        showProgress: validSteps.length > 1,
        showButtons: ['next', 'close'],
        steps: validSteps,
        overlayOpacity: 0.5, // Match modal overlay (50%)
        onDestroyStarted: () => {
          // Mark as seen when tour is closed
          markAsSeen(tourName);
          driverObj.destroy();
        }
      });

      driverObj.drive();
    }, 500);
  };

  /**
   * Reset a specific tour (for testing)
   * Note: Only resets locally, backend keeps the completed state
   */
  const resetTour = (tourName) => {
    const tour = TOURS[tourName];
    if (tour) {
      // Update local state
      setToursCompleted(prev => {
        const updated = { ...prev };
        delete updated[tour.id];
        return updated;
      });

    }
  };

  /**
   * Reset all tours (for testing)
   * Note: Only resets locally, backend keeps the completed state
   */
  const resetAllTours = () => {
    // Clear all tour state
    setToursCompleted({});

  };

  return {
    showTour,
    hasSeen,
    resetTour,
    resetAllTours,
  };
};

/**
 * Get list of all available tours
 */
export const getAvailableTours = () => {
  return Object.keys(TOURS);
};

/**
 * Tour progress for analytics/debugging
 * Note: This is a standalone function and can't access the hook's state.
 * Use the backend API directly: GET /api/v1/users/me/tours
 */
export const getTourProgress = async () => {
  try {
    const response = await apiClient.get('/api/v1/users/me/tours');
    return response.data.tours_completed || {};
  } catch (error) {
    return {};
  }
};
