// SPDX-License-Identifier: AGPL-3.0-or-later
import React, { useState } from 'react';
import { Tooltip, TooltipTrigger, TooltipContent } from './ui/tooltip';

/**
 * Small, discrete progress bar for data extracts
 * Designed to appear underneath loading sparkle logo
 *
 * Hidden feature: Click to show detailed stats
 *
 * @param {Object} progress - Progress info from background extract
 * @param {number} progress.rowsStreamed - Number of rows downloaded so far
 * @param {number} progress.totalRows - Total rows (if known from metadata)
 * @param {string} progress.status - Extract status
 */
const ExtractProgressBar = ({ progress }) => {
  const [showDetails, setShowDetails] = useState(false);

  // Only show during active download (running status with rows being streamed)
  if (!progress || progress.status !== 'running' || !progress.progress?.rowsStreamed) {
    return null;
  }

  const { rowsStreamed, totalRows } = progress.progress;
  const hasTotal = totalRows && totalRows > 0;

  // Calculate percentage: if we have total, use it; otherwise estimate based on rows downloaded
  let percentage;
  if (hasTotal) {
    percentage = Math.min((rowsStreamed / totalRows) * 100, 100);
  } else {
    // When we don't know total, show progress based on arbitrary milestone (e.g., every 100k rows = 10%)
    // This gives visual feedback that something is happening
    percentage = Math.min((rowsStreamed / 1000000) * 100, 95); // Max 95% until we know total
  }

  // Format large numbers with K, M suffixes
  const formatNumber = (num) => {
    if (!num) return '0';
    if (num >= 1000000) {
      return (num / 1000000).toFixed(1) + 'M';
    }
    if (num >= 1000) {
      return (num / 1000).toFixed(1) + 'K';
    }
    return num.toString();
  };

  return (
    <div className="mt-10 flex flex-col items-center">
      {/* Progress bar - narrow, thick, rounded ends - clickable */}
      <Tooltip>
        <TooltipTrigger asChild>
          <div
            className="h-1.5 w-24 bg-border rounded-full overflow-hidden cursor-pointer"
            onClick={() => setShowDetails(!showDetails)}
            aria-label="Click for details"
          >
            <div
              className="h-full bg-primary rounded-full transition-all duration-300 ease-out"
              style={{ width: `${percentage}%` }}
            />
          </div>
        </TooltipTrigger>
        <TooltipContent>Click for details</TooltipContent>
      </Tooltip>

      {/* Hidden details - shown on click - wider than bar */}
      {showDetails && (
        <div className="mt-2 text-xs text-muted-foreground text-center whitespace-nowrap">
          {hasTotal ? (
            <div>
              {formatNumber(rowsStreamed)} / {formatNumber(totalRows)} rows
              <br />
              {Math.round(percentage)}%
            </div>
          ) : (
            <div>
              {formatNumber(rowsStreamed)} rows
              <br />
              (total unknown)
            </div>
          )}
        </div>
      )}
    </div>
  );
};

export default ExtractProgressBar;
