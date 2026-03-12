// SPDX-License-Identifier: AGPL-3.0-or-later
/**
 * ResultsLoading Component
 *
 * Displays a loading animation while query is executing.
 */

/**
 * ResultsLoading component
 *
 * Shows the Kyomi animated logo with an optional message.
 *
 * Usage:
 * ```jsx
 * <ResultsLoading message="Running query..." />
 * ```
 */
const ResultsLoading = ({ message = 'Running query...' }) => {
  return (
    <div className="flex-1 min-h-0 flex flex-col items-center justify-center border border-input rounded-md bg-card">
      <img
        src="/kyomi_animated_logo.svg"
        alt="Loading"
        className="w-8 h-8 mb-2"
      />
      {message && (
        <p className="text-xs text-muted-foreground">{message}</p>
      )}
    </div>
  );
};

export default ResultsLoading;
