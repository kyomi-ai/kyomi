/**
 * Zustand store for SQL Editor state management
 *
 * This store manages:
 * - Multiple result tabs
 * - Active tab selection
 * - Query text
 * - Table UI state (sorting, pagination) per tab
 *
 * Persisted to localStorage for state restoration on navigation/refresh
 */

import { create } from 'zustand';
import { persist } from 'zustand/middleware';
import type {
  SqlEditorState,
  ResultTab,
  TableUIState,
} from './types';

/**
 * Default table UI state
 */
const DEFAULT_TABLE_UI_STATE: TableUIState = {
  sortBy: [],
  currentPage: 1,
  pageSize: 50,
};

/**
 * Generate a unique ID for tabs
 */
const generateTabId = (): string => {
  return `tab_${Date.now()}_${Math.random().toString(36).substr(2, 9)}`;
};

/**
 * SQL Editor Store
 *
 * Usage:
 * ```ts
 * const { tabs, activeTabId, addTab, setActiveTab } = useSqlEditorStore();
 * ```
 */
export const useSqlEditorStore = create<SqlEditorState>()(
  persist(
    (set, get) => ({
      // Initial state
      tabs: [],
      activeTabId: null,
      queryText: '',
      tableUIState: {},
      nextColorIndex: 0, // Track next color index to assign (0-7, cycles)
      defaultPageSize: 50, // User's preferred page size (persisted)

      // Sidebar state (persisted)
      // Default to catalog open on desktop (≥1024px), closed on mobile/tablet
      activeRightTab: typeof window !== 'undefined' && window.innerWidth >= 1024 ? 'catalog' : null,
      rightSidebarPercentage: 30, // Default 30% width

  // Actions
  addTab: (tabData) => {
    const id = generateTabId();
    const now = Date.now();
    const state = get();

    const newTab: ResultTab = {
      ...tabData,
      id,
      pinned: false, // New tabs are unpinned by default
      colorIndex: state.nextColorIndex, // Assign stable color index
      createdAt: now,
      updatedAt: now,
    };

    set((state) => {
      // Enforce 5-tab limit for unpinned tabs
      const unpinnedTabs = state.tabs.filter(t => !t.pinned);
      let updatedTabs = [...state.tabs];

      // If we already have 5+ unpinned tabs, remove the oldest unpinned one
      if (unpinnedTabs.length >= 5) {
        const oldestUnpinned = unpinnedTabs.sort((a, b) => a.createdAt - b.createdAt)[0];
        updatedTabs = updatedTabs.filter(t => t.id !== oldestUnpinned.id);

        // Clean up UI state for removed tab
        const { [oldestUnpinned.id]: _removed, ...remainingUIState } = state.tableUIState;

        return {
          tabs: [...updatedTabs, newTab],
          activeTabId: id,
          nextColorIndex: (state.nextColorIndex + 1) % 8, // Cycle through 0-7
          tableUIState: {
            ...remainingUIState,
            [id]: { ...DEFAULT_TABLE_UI_STATE, pageSize: state.defaultPageSize },
          },
        };
      }

      return {
        tabs: [...updatedTabs, newTab],
        activeTabId: id,
        nextColorIndex: (state.nextColorIndex + 1) % 8, // Cycle through 0-7
        tableUIState: {
          ...state.tableUIState,
          [id]: { ...DEFAULT_TABLE_UI_STATE, pageSize: state.defaultPageSize },
        },
      };
    });

    return id;
  },

  updateTab: (tabId, updates) => {
    set((state) => ({
      tabs: state.tabs.map((tab) =>
        tab.id === tabId
          ? { ...tab, ...updates, updatedAt: Date.now() }
          : tab
      ),
    }));
  },

  removeTab: (tabId) => {
    set((state) => {
      const newTabs = state.tabs.filter((tab) => tab.id !== tabId);

      // If we're removing the active tab, select another one
      let newActiveTabId = state.activeTabId;
      if (state.activeTabId === tabId) {
        // Select the tab to the right, or the last tab if removing the rightmost
        const removedIndex = state.tabs.findIndex((tab) => tab.id === tabId);
        if (newTabs.length > 0) {
          const newIndex = Math.min(removedIndex, newTabs.length - 1);
          newActiveTabId = newTabs[newIndex].id;
        } else {
          newActiveTabId = null;
        }
      }

      // Clean up table UI state
      const { [tabId]: _removed, ...remainingUIState } = state.tableUIState;

      return {
        tabs: newTabs,
        activeTabId: newActiveTabId,
        tableUIState: remainingUIState,
      };
    });
  },

  setActiveTab: (tabId) => {
    set({ activeTabId: tabId });
  },

  togglePin: (tabId) => {
    set((state) => ({
      tabs: state.tabs.map((tab) =>
        tab.id === tabId ? { ...tab, pinned: !tab.pinned } : tab
      ),
    }));
  },

  setQueryText: (text) => {
    set({ queryText: text });
  },

  setTableUIState: (tabId, stateUpdates) => {
    set((state) => ({
      tableUIState: {
        ...state.tableUIState,
        [tabId]: {
          ...state.tableUIState[tabId],
          ...stateUpdates,
        },
      },
    }));
  },

  setDefaultPageSize: (pageSize) => {
    set({ defaultPageSize: pageSize });
  },

  setActiveRightTab: (tab) => {
    set({ activeRightTab: tab });
  },

  setRightSidebarPercentage: (percentage) => {
    set({ rightSidebarPercentage: percentage });
  },

  clearAllTabs: () => {
    set({
      tabs: [],
      activeTabId: null,
      tableUIState: {},
    });
  },
}),
    {
      name: 'sql-editor-storage', // localStorage key
      // Only persist these fields, with tabs stripped of row data
      partialize: (state) => ({
        tabs: state.tabs.map(tab => {
          // Only persist metadata, not the actual row data
          if (tab.result) {
            return {
              ...tab,
              result: {
                columns: tab.result.columns,
                rows: [], // Don't persist rows - they can be huge and Date objects don't serialize well
                rowCount: 0,
                totalRows: tab.result.totalRows,
                queryHandle: tab.result.queryHandle, // Keep queryHandle to re-fetch results
                executionTime: tab.result.executionTime,
                bytesProcessed: tab.result.bytesProcessed,
              },
              // Mark as needing refresh on rehydration
              needsRefresh: true,
            };
          }
          return tab;
        }),
        activeTabId: state.activeTabId,
        queryText: state.queryText,
        tableUIState: state.tableUIState,
        nextColorIndex: state.nextColorIndex,
        defaultPageSize: state.defaultPageSize,
        activeRightTab: state.activeRightTab,
        rightSidebarPercentage: state.rightSidebarPercentage,
      }),
      onRehydrateStorage: () => {
        return (state, error) => {
          if (error) {
          }
        };
      },
    }
  )
);

/**
 * Selector hooks for common use cases
 */

/**
 * Get the currently active tab
 * Uses custom equality to prevent re-renders on every tab property change
 */
export const useActiveTab = (): ResultTab | null => {
  return useSqlEditorStore((state) => {
    if (!state.activeTabId) return null;
    return state.tabs.find((tab) => tab.id === state.activeTabId) || null;
  }, (a, b) => {
    // Custom equality: only re-render when meaningful properties change
    if (a === b) return true;
    if (!a || !b) return false;

    // Re-render when status changes (running, success, error, idle)
    if (a.status !== b.status) return false;

    // Re-render when result data identity changes (but not nested content)
    if (a.result !== b.result) return false;

    // Re-render when error changes
    if (a.error !== b.error) return false;

    // Re-render when needsRefresh flag changes
    if (a.needsRefresh !== b.needsRefresh) return false;

    // Ignore other property changes (like updatedAt, visualization tweaks, etc.)
    return true;
  });
};

/**
 * Get table UI state for a specific tab
 * Only re-renders when THIS tab's UI state changes
 */
export const useTableUIState = (tabId: string | null): TableUIState => {
  return useSqlEditorStore((state) => {
    if (!tabId) return DEFAULT_TABLE_UI_STATE;
    return state.tableUIState[tabId] || DEFAULT_TABLE_UI_STATE;
  }, (a, b) => {
    // Deep equality check for TableUIState object
    if (a === b) return true;
    if (!a || !b) return false;

    // Check sortBy array
    if (a.sortBy.length !== b.sortBy.length) return false;
    if (!a.sortBy.every((sort, i) =>
      b.sortBy[i] &&
      sort.column === b.sortBy[i].column &&
      sort.direction === b.sortBy[i].direction
    )) return false;

    // Check pagination
    if (a.currentPage !== b.currentPage) return false;
    if (a.pageSize !== b.pageSize) return false;

    return true;
  });
};

/**
 * Check if a tab is active
 */
export const useIsTabActive = (tabId: string): boolean => {
  return useSqlEditorStore((state) => state.activeTabId === tabId);
};
