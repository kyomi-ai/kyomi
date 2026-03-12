// SPDX-License-Identifier: AGPL-3.0-or-later
import { MarkdownRenderer } from './MarkdownRenderer';

/**
 * StaticReport - Renders static ChartML reports (non-editable)
 *
 * This is a wrapper around MarkdownRenderer that accepts raw markdown/ChartML
 * content and renders it in read-only mode. Perfect for embedding reports
 * in settings pages or other UI contexts.
 *
 * @param {Object} props
 * @param {string} props.content - Markdown/ChartML content to render
 * @param {string} props.reportId - Unique identifier for this report
 * @param {Object} props.datasets - Optional datasets for ChartML resolution
 */
const StaticReport = ({ content, reportId = 'static-report', datasets = {} }) => {
  if (!content || !content.trim()) {
    return (
      <div className="text-center py-8 text-muted-foreground">
        <p>No report content available</p>
      </div>
    );
  }

  return (
    <div className="w-full">
      <MarkdownRenderer
        messageId={reportId}
        sessionId="static-report"
        isStreaming={false}
        showAddToDashboard={false}
        dashboardConfig={{
          id: reportId,
          datasets: datasets
        }}
        isChatBubble={false}
      >
        {content}
      </MarkdownRenderer>
    </div>
  );
};

export default StaticReport;
