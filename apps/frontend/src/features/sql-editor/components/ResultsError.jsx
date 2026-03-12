// SPDX-License-Identifier: AGPL-3.0-or-later
/**
 * ResultsError Component
 *
 * Displays query execution errors with special handling for expired results.
 */
import { Alert, AlertDescription } from '@/components/ui/alert';
import { formatSQLError } from '../utils/formatSQLError';

/**
 * Check if error is due to expired query results (cache expired)
 */
const isExpiredResultsError = (errorMessage) => {
  const lower = errorMessage.toLowerCase();
  return lower.includes('expired') ||
    lower.includes('not found') ||
    lower.includes('failed to restore') ||
    (lower.includes('job') && lower.includes('not found'));
};

/**
 * ResultsError component
 *
 * Displays different UI based on error type:
 * - Expired results: Error with "Re-run Query" button
 * - SQL errors: Red error message with formatted text
 *
 * Usage:
 * ```jsx
 * <ResultsError
 *   error={{ message: 'Syntax error at line 5' }}
 *   onRerun={handleRerun}
 *   rerunning={false}
 * />
 * ```
 */
const ResultsError = ({
  error,
  onRerun,
  rerunning = false,
}) => {
  const formattedError = formatSQLError(error.message);
  const isExpired = isExpiredResultsError(error.message);

  // Expired results - show informative UI with re-run button (not an error, just needs refresh)
  if (isExpired && onRerun) {
    return (
      <div className="flex-1 flex items-center justify-center p-8">
        <div className="text-center max-w-md">
          <div className="mb-4">
            <svg
              className="h-12 w-12 text-info-foreground mx-auto"
              fill="none"
              viewBox="0 0 24 24"
              stroke="currentColor"
            >
              <path
                strokeLinecap="round"
                strokeLinejoin="round"
                strokeWidth={1.5}
                d="M13 16h-1v-4h-1m1-4h.01M21 12a9 9 0 11-18 0 9 9 0 0118 0z"
              />
            </svg>
          </div>
          <h3 className="text-base font-medium text-foreground mb-2">
            Results No Longer Available
          </h3>
          <p className="text-sm text-muted-foreground mb-6">
            Results are no longer available. Click below to re-run the query.
          </p>
          <button
            onClick={onRerun}
            disabled={rerunning}
            className="inline-flex justify-center items-center px-6 py-2.5 rounded-lg shadow-sm bg-primary text-white font-medium hover:bg-primary/90 focus:outline-none focus:ring-2 focus:ring-offset-2 focus:ring-primary transition-all duration-200 disabled:opacity-50 disabled:cursor-not-allowed"
          >
            <svg className="w-5 h-5 mr-2" fill="none" stroke="currentColor" viewBox="0 0 24 24">
              <path
                strokeLinecap="round"
                strokeLinejoin="round"
                strokeWidth={2}
                d="M4 4v5h.582m15.356 2A8.001 8.001 0 004.582 9m0 0H9m11 11v-5h-.581m0 0a8.003 8.003 0 01-15.357-2m15.357 2H15"
              />
            </svg>
            {rerunning ? 'Re-running Query...' : 'Re-run Query'}
          </button>
        </div>
      </div>
    );
  }

  // Regular error
  return (
    <div className="flex-1 p-4 overflow-auto">
      <Alert variant="error">
        <AlertDescription>{formattedError}</AlertDescription>
      </Alert>
    </div>
  );
};

export default ResultsError;
