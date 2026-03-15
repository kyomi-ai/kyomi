// SPDX-License-Identifier: AGPL-3.0-or-later
import React from 'react';
import { MarkdownRenderer } from './MarkdownRenderer';
import { describeCron } from '../utils/cronUtils';

/** Safe check for 'error' key — returns false for strings/primitives instead of throwing. */
const hasKey = (obj, key) => typeof obj === 'object' && obj !== null && key in obj;

const BigQueryCostEstimateRenderer = ({ schema }) => {
  const { input, output } = schema;

  // Determine if this is success or error
  const hasError = output && (hasKey(output, 'error') || (output.status && output.status !== 'success'));
  const isSuccess = output && !hasError;

  return (
    <div className="space-y-3">
      {input && (
        <div>
          <span className="font-medium text-foreground text-xs">Query:</span>
          <div className="mt-1 bg-muted p-2 rounded">
            <MarkdownRenderer className="text-xs" compact={true}>
              {`\`\`\`sql\n${input.sql}\n\`\`\``}
            </MarkdownRenderer>
          </div>
        </div>
      )}

      {output && (
        <div>
          <span className="font-medium text-foreground text-xs">Cost Analysis:</span>
          <div className="mt-1">
            {isSuccess ? (
              <div className="grid grid-cols-3 gap-2 text-xs">
                <div className="bg-muted p-2 rounded border border-border">
                  <div className="font-medium text-foreground">Cost</div>
                  <div className="text-muted-foreground">{output.cost}</div>
                </div>
                <div className="bg-muted p-2 rounded border border-border">
                  <div className="font-medium text-foreground">Size</div>
                  <div className="text-muted-foreground">{output.size}</div>
                </div>
                <div className={`p-2 rounded border ${
                  output.safety === 'OK' ? 'bg-muted border-border' :
                  output.safety === 'WARNING' ? 'bg-warning border-warning-border' :
                  'bg-error border-error-border'
                }`}>
                  <div className={`font-medium ${
                    output.safety === 'OK' ? 'text-foreground' :
                    output.safety === 'WARNING' ? 'text-warning-foreground' :
                    'text-error-foreground'
                  }`}>Safety</div>
                  <div className={`${
                    output.safety === 'OK' ? 'text-muted-foreground' :
                    output.safety === 'WARNING' ? 'text-warning-foreground' :
                    'text-error-foreground'
                  }`}>{output.safety}</div>
                </div>
              </div>
            ) : (
              <div className="bg-error p-2 rounded border border-error-border">
                <div className="font-medium text-error-foreground text-xs">Error</div>
                <div className="text-error-foreground text-xs">
                  {(() => {
                    // Try to extract error from output.data JSON string first
                    if (output.data) {
                      try {
                        // Handle CrewAI instruction contamination - extract just the JSON part
                        let jsonString = output.data;
                        const instructionMarker = '\n\n\nYou ONLY have access to the following tools';
                        if (jsonString.includes(instructionMarker)) {
                          jsonString = jsonString.split(instructionMarker)[0].trim();
                        }

                        const parsedData = JSON.parse(jsonString);
                        if (parsedData.error) return parsedData.error;
                      } catch {
                        // Fall through to other error sources
                      }
                    }
                    // Fallback to direct error field or unknown
                    return output.error || 'Unknown error';
                  })()}
                </div>
              </div>
            )}
          </div>
        </div>
      )}
    </div>
  );
};

const BigQueryQueryRenderer = ({ schema }) => {
  const { input, output } = schema;

  return (
    <div className="space-y-3">
      {input && (
        <div>
          <span className="font-medium text-foreground text-xs">Query:</span>
          <div className="mt-1 bg-muted p-2 rounded">
            <MarkdownRenderer className="text-xs" compact={true}>
              {`\`\`\`sql\n${input.sql}\n\`\`\``}
            </MarkdownRenderer>
          </div>
          <div className="mt-1 flex gap-2 text-xs text-muted-foreground flex-wrap items-center">
            {input.datasource && (
              <span className="inline-block px-2 py-1 bg-accent rounded border border-border text-foreground">
                <span className="font-medium">Datasource:</span> {input.datasource}
              </span>
            )}
            <span>Limit: {input.limit} rows</span>
            {input.allows_large_query && (
              <span className="text-warning-foreground">• Large query allowed</span>
            )}
          </div>
          {output?.console_url && (
            <div className="mt-1">
              <a
                href={output.console_url}
                target="_blank"
                rel="noopener noreferrer"
                className="text-xs text-primary hover:text-primary/80 underline inline-flex items-center gap-1"
              >
                View in BigQuery Console
                <svg className="w-3 h-3" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                  <path strokeLinecap="round" strokeLinejoin="round" strokeWidth={2} d="M10 6H6a2 2 0 00-2 2v10a2 2 0 002 2h10a2 2 0 002-2v-4M14 4h6m0 0v6m0-6L10 14" />
                </svg>
              </a>
            </div>
          )}
        </div>
      )}

      {output && (
        <div>
          <span className="font-medium text-foreground text-xs">Results:</span>
          <div className="mt-1">
            {!(hasKey(output, 'error')) && !output.status ? (
              <div className="space-y-2">
                {/* Metrics Row - compact format only has rows and truncated */}
                <div className="grid grid-cols-2 gap-2 text-xs">
                  <div className="bg-muted p-2 rounded border border-border">
                    <div className="font-medium text-foreground">Rows Returned</div>
                    <div className="text-muted-foreground">{output.rows || 0}</div>
                  </div>
                  <div className="bg-muted p-2 rounded border border-border">
                    <div className="font-medium text-foreground">Preview Limit</div>
                    <div className="text-muted-foreground">{output.truncated ? '20 (truncated)' : 'Full result'}</div>
                  </div>
                </div>

                {/* Data Preview - convert columnar to row-based for rendering */}
                {output.data && output.cols && output.cols.length > 0 && (
                  <div>
                    <div className="font-medium text-foreground text-xs mb-1">Preview:</div>
                    <div className="bg-card border border-border rounded text-xs overflow-hidden">
                      <table className="w-full">
                        <thead className="bg-accent">
                          <tr>
                            {output.cols.map((col, idx) => {
                              const colName = typeof col === 'object' ? col.name : col;
                              return (
                                <th key={idx} className="px-2 py-1 text-left font-medium text-foreground border-r border-border last:border-r-0">
                                  {colName}
                                </th>
                              );
                            })}
                          </tr>
                        </thead>
                        <tbody>
                          {/* Convert columnar data to rows - limit to 20 if truncated */}
                          {Array.from({ length: output.truncated ? Math.min(20, output.rows || 0) : (output.rows || 0) }, (_, rowIdx) => (
                            <tr key={rowIdx} className={rowIdx % 2 === 0 ? 'bg-card' : 'bg-muted'}>
                              {output.cols.map((col, colIdx) => {
                                const colName = typeof col === 'object' ? col.name : col;
                                return (
                                  <td key={colIdx} className="px-2 py-1 text-muted-foreground border-r border-border last:border-r-0">
                                    {String(output.data[colName]?.[rowIdx] ?? '')}
                                  </td>
                                );
                              })}
                            </tr>
                          ))}
                        </tbody>
                      </table>
                    </div>
                    {output.truncated === true && (
                      <div className="text-xs text-muted-foreground mt-1">
                        💡 Query returned more than 20 rows. Create a ChartML visualization to see the full dataset.
                      </div>
                    )}
                  </div>
                )}
              </div>
            ) : (
              <div className="space-y-2">
                <div className="bg-error p-2 rounded border border-error-border">
                  <div className="font-medium text-error-foreground text-xs">Query Failed</div>
                  <div className="text-error-foreground text-xs">
                    {(() => {

                      // Try to extract error from output.data JSON string first
                      if (output.data) {
                        try {
                          // Handle CrewAI instruction contamination - extract just the JSON part
                          let jsonString = output.data;
                          const instructionMarker = '\n\n\nYou ONLY have access to the following tools';
                          if (jsonString.includes(instructionMarker)) {
                            jsonString = jsonString.split(instructionMarker)[0].trim();
                          }

                          const parsedData = JSON.parse(jsonString);
                          if (parsedData.error) {
                            return parsedData.error;
                          }
                        } catch {
                          // Ignore JSON parse errors, fall through to default
                        }
                      }

                      // Fallback to direct error field or unknown
                      return output.error || 'Unknown error';
                    })()}
                  </div>
                </div>
                <div className="text-xs text-muted-foreground">
                  Execution time: {output.execution_time}
                </div>
              </div>
            )}
          </div>
        </div>
      )}
    </div>
  );
};

const BigQueryTableInfoRenderer = ({ schema }) => {
  const { input, output } = schema;

  // Determine if this is success or error
  const hasError = output && hasKey(output, 'error');
  const isSuccess = output && !hasError;

  // Optimized flat structure: { table, desc, rows, cols: [{name, type, desc}] }
  const tableName = output?.table;
  const tableDesc = output?.desc;
  const rowCount = output?.rows;
  const columns = output?.cols || [];
  const datasource = input?.datasource;

  // Extract learnings from output
  const learnings = output?.learnings || [];

  return (
    <div className="space-y-2">
      {/* Show datasource if available */}
      {datasource && (
        <div className="inline-block px-2 py-1 bg-accent rounded border border-border text-foreground text-xs">
          <span className="font-medium">Datasource:</span> {datasource}
        </div>
      )}

      {tableName && (
        <div>
          <span className="font-medium text-foreground text-xs">Table:</span>
          <div className="mt-1 text-xs text-muted-foreground font-mono bg-muted p-1 rounded">
            {tableName}
          </div>
        </div>
      )}

      {/* Accumulated Knowledge Section */}
      {learnings.length > 0 && (
        <div>
          <div className="font-medium text-foreground text-xs mb-1 flex items-center gap-1">
            <span>💡</span>
            <span>Accumulated Knowledge ({learnings.length})</span>
          </div>
          <div className="space-y-1">
            {learnings.slice(0, 3).map((learning, idx) => (
              <div key={idx} className="bg-info border border-info-border rounded p-2 text-xs">
                <div className="flex items-start justify-between">
                  <div className="flex-1">
                    <div className="text-foreground font-medium">{learning.insight}</div>
                    {learning.context && (
                      <div className="text-muted-foreground text-xs mt-1 italic">{learning.context}</div>
                    )}
                  </div>
                  <div className="text-right ml-2 flex-shrink-0">
                    <div className="font-medium text-info-foreground text-xs">
                      {Math.round(learning.similarity * 100)}%
                    </div>
                    <div className="text-xs text-muted-foreground">
                      {learning.datasource_specific ? 'datasource' : 'global'}
                    </div>
                  </div>
                </div>
              </div>
            ))}
            {learnings.length > 3 && (
              <div className="text-xs text-muted-foreground text-center">
                ...and {learnings.length - 3} more
              </div>
            )}
          </div>
        </div>
      )}

      {isSuccess && (
        <div>
          <span className="font-medium text-foreground text-xs">Table Information:</span>
          <div className="mt-1">
            <div className="bg-muted p-3 rounded text-xs space-y-2">
              {/* Row count */}
              {rowCount != null && (
                <div className="text-muted-foreground">
                  <span className="font-medium">Rows:</span> {rowCount.toLocaleString()}
                </div>
              )}

              {/* Columns with details */}
              {columns.length > 0 && (
                <div>
                  <span className="font-medium text-foreground">Columns ({columns.length}):</span>
                  <div className="text-muted-foreground mt-1 space-y-1">
                    {columns.slice(0, 10).map((col, idx) => (
                      <div key={idx} className="flex gap-2">
                        <span className="font-mono">{col.name}</span>
                        <span className="text-muted-foreground">({col.type})</span>
                        {col.desc && <span className="text-muted-foreground">- {col.desc}</span>}
                      </div>
                    ))}
                    {columns.length > 10 && (
                      <div className="text-muted-foreground italic">
                        ...and {columns.length - 10} more columns
                      </div>
                    )}
                  </div>
                </div>
              )}

              {/* Table description */}
              {tableDesc && tableDesc.trim() && (
                <div className="border-t border-border pt-2 mt-2">
                  <span className="font-medium text-foreground">Description:</span>
                  <div className="text-muted-foreground mt-1">{tableDesc}</div>
                </div>
              )}
            </div>
          </div>
        </div>
      )}

      {hasError && (
        <div className="bg-error p-2 rounded border border-error-border">
          <div className="font-medium text-error-foreground text-xs">Error</div>
          <div className="text-error-foreground text-xs">{output.error || 'Unknown error'}</div>
        </div>
      )}
    </div>
  );
};

const BigQuerySampleRenderer = ({ schema }) => {
  const { input, output } = schema;

  return (
    <div className="space-y-3">
      {/* Show datasource if available */}
      {input?.datasource && (
        <div className="inline-block px-2 py-1 bg-accent rounded border border-border text-foreground text-xs">
          <span className="font-medium">Datasource:</span> {input.datasource}
        </div>
      )}

      {input && (
        <div>
          <span className="font-medium text-foreground text-xs">Table:</span>
          <div className="mt-1 bg-muted p-2 rounded">
            <div className="text-xs text-foreground font-mono">{input.table_name}</div>
          </div>
          {output?.console_url && (
            <div className="mt-1">
              <a
                href={output.console_url}
                target="_blank"
                rel="noopener noreferrer"
                className="text-xs text-primary hover:text-primary/80 underline inline-flex items-center gap-1"
              >
                View in BigQuery Console
                <svg className="w-3 h-3" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                  <path strokeLinecap="round" strokeLinejoin="round" strokeWidth={2} d="M10 6H6a2 2 0 00-2 2v10a2 2 0 002 2h10a2 2 0 002-2v-4M14 4h6m0 0v6m0-6L10 14" />
                </svg>
              </a>
            </div>
          )}
          <div className="mt-1 bg-muted p-2 rounded">
            <div className="flex gap-2 text-xs text-muted-foreground">
              <span>Rows: {input.sample_rows || 5}</span>
              {input.days_back && (
                <span>• Days back: {input.days_back}</span>
              )}
            </div>
          </div>
        </div>
      )}

      {output && (
        <div>
          <span className="font-medium text-foreground text-xs">Sample Data:</span>
          <div className="mt-1">
            {!(hasKey(output, 'error')) && !output.status ? (
              <div className="space-y-2">
                {/* Metrics Row */}
                <div className="grid grid-cols-2 gap-2 text-xs">
                  <div className="bg-muted p-2 rounded border border-border">
                    <div className="font-medium text-foreground">Sampled Rows</div>
                    <div className="text-muted-foreground">{output.rows || 0}</div>
                  </div>
                  <div className="bg-muted p-2 rounded border border-border">
                    <div className="font-medium text-foreground">Table Total</div>
                    <div className="text-muted-foreground">{output.table_rows?.toLocaleString() || 'Unknown'}</div>
                  </div>
                </div>

                {/* Data Preview - convert columnar to row-based for rendering */}
                {output.data && output.cols && output.cols.length > 0 && (
                  <div>
                    <div className="font-medium text-foreground text-xs mb-1">Sample Rows:</div>
                    <div className="bg-card border border-border rounded text-xs overflow-hidden">
                      <table className="w-full">
                        <thead className="bg-accent">
                          <tr>
                            {output.cols.map((col, idx) => {
                              const colName = typeof col === 'object' ? col.name : col;
                              return (
                                <th key={idx} className="px-2 py-1 text-left font-medium text-foreground border-r border-border last:border-r-0">
                                  {colName}
                                </th>
                              );
                            })}
                          </tr>
                        </thead>
                        <tbody>
                          {/* Convert columnar data {col1: [v1, v2], col2: [v3, v4]} to rows */}
                          {Array.from({ length: output.truncated ? Math.min(20, output.rows || 0) : (output.rows || 0) }, (_, rowIdx) => (
                            <tr key={rowIdx} className={rowIdx % 2 === 0 ? 'bg-card' : 'bg-muted'}>
                              {output.cols.map((col, colIdx) => {
                                const colName = typeof col === 'object' ? col.name : col;
                                return (
                                  <td key={colIdx} className="px-2 py-1 text-muted-foreground border-r border-border last:border-r-0">
                                    {String(output.data[colName]?.[rowIdx] ?? '')}
                                  </td>
                                );
                              })}
                            </tr>
                          ))}
                        </tbody>
                      </table>
                    </div>
                  </div>
                )}
              </div>
            ) : (
              <div className="bg-error p-2 rounded border border-error-border">
                <div className="font-medium text-error-foreground text-xs">
                  {output.error?.includes('exceeds') && output.error?.includes('GB limit')
                    ? '🚫 Query Blocked - Too Large'
                    : 'Sampling Failed'}
                </div>
                <div className="text-error-foreground text-xs">{output.error || 'Unknown error'}</div>
                {output.error?.includes('exceeds') && output.error?.includes('GB limit') && (
                  <div className="mt-2 text-xs text-error-foreground">
                    💡 Tip: This table is too large to sample with SELECT *. Try querying specific columns or use filters to reduce data scanned.
                  </div>
                )}
                {output.console_url && (
                  <div className="mt-1">
                    <a
                      href={output.console_url}
                      target="_blank"
                      rel="noopener noreferrer"
                      className="text-xs text-primary hover:text-primary/80 underline inline-flex items-center gap-1"
                    >
                      View in BigQuery Console
                      <svg className="w-3 h-3" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                        <path strokeLinecap="round" strokeLinejoin="round" strokeWidth={2} d="M10 6H6a2 2 0 00-2 2v10a2 2 0 002 2h10a2 2 0 002-2v-4M14 4h6m0 0v6m0-6L10 14" />
                      </svg>
                    </a>
                  </div>
                )}
              </div>
            )}
          </div>
        </div>
      )}
    </div>
  );
};

const BigQuerySearchRenderer = ({ schema }) => {
  const { input, output } = schema;

  // Handle both compact format {count, tables} and verbose format {status, results_count, results}
  const isCompactFormat = output && hasKey(output, 'tables') && !hasKey(output, 'status');
  const isVerboseFormat = output && output.status === 'success';
  const isSuccess = isCompactFormat || isVerboseFormat;

  // Normalize to verbose format for rendering
  const resultsCount = isCompactFormat ? output.count : output?.results_count;
  const results = isCompactFormat
    ? output.tables?.map(t => ({
        table_id: t.table,
        table_description: t.desc || '',
        column_count: t.cols,
        similarity_score: t.score
      }))
    : output?.results;
  const message = isCompactFormat
    ? `Found ${output.count} matching tables`
    : output?.message;

  // Extract learnings from output
  const learnings = output?.learnings || [];

  // DEBUG: Log to console to verify data
  console.log('🔍 BigQuerySearchRenderer - output keys:', output ? Object.keys(output) : 'no output');
  console.log('🔍 BigQuerySearchRenderer - learnings:', learnings);

  return (
    <div className="space-y-3">
      {input && (
        <div>
          <span className="font-medium text-foreground text-xs">Search Query:</span>
          <div className="mt-1 bg-muted p-2 rounded">
            <div className="text-xs text-foreground font-medium">"{input.query}"</div>
            <div className="mt-1 flex gap-2 text-xs text-muted-foreground">
              <span>Limit: {input.limit || 20} results</span>
              {input.datasource && (
                <span className="text-primary">• Datasource: {input.datasource}</span>
              )}
              {!input.datasource && input.include_public !== false && (
                <span className="text-primary">• Including public datasets</span>
              )}
            </div>
          </div>
        </div>
      )}

      {output && (
        <div>
          <span className="font-medium text-foreground text-xs">Search Results:</span>
          <div className="mt-1">
            {isSuccess ? (
              <div className="space-y-2">
                {/* Summary */}
                <div className="bg-muted p-2 rounded border border-border">
                  <div className="font-medium text-foreground text-xs">Found {resultsCount} tables</div>
                  <div className="text-muted-foreground text-xs">{message}</div>
                </div>

                {/* Accumulated Knowledge Section */}
                {learnings.length > 0 && (
                  <div>
                    <div className="font-medium text-foreground text-xs mb-1 flex items-center gap-1">
                      <span>💡</span>
                      <span>Accumulated Knowledge ({learnings.length})</span>
                    </div>
                    <div className="space-y-1">
                      {learnings.slice(0, 3).map((learning, idx) => (
                        <div key={idx} className="bg-info border border-info-border rounded p-2 text-xs">
                          <div className="flex items-start justify-between">
                            <div className="flex-1">
                              <div className="text-foreground font-medium">{learning.insight}</div>
                              {learning.context && (
                                <div className="text-muted-foreground text-xs mt-1 italic">{learning.context}</div>
                              )}
                            </div>
                            <div className="text-right ml-2 flex-shrink-0">
                              <div className="font-medium text-info-foreground text-xs">
                                {Math.round(learning.similarity * 100)}%
                              </div>
                              <div className="text-xs text-muted-foreground">
                                {learning.datasource_specific ? 'datasource' : 'global'}
                              </div>
                            </div>
                          </div>
                        </div>
                      ))}
                      {learnings.length > 3 && (
                        <div className="text-xs text-muted-foreground text-center">
                          ...and {learnings.length - 3} more
                        </div>
                      )}
                    </div>
                  </div>
                )}

                {/* Results List */}
                {results && results.length > 0 && (
                  <div className="space-y-2">
                    {results.slice(0, 5).map((result, idx) => (
                      <div key={idx} className="bg-card border border-border rounded p-3 text-xs">
                        <div className="flex items-start justify-between">
                          <div className="flex-1">
                            <div className="font-medium text-foreground font-mono">
                              {result.table_id}
                            </div>
                            <div className="text-muted-foreground mt-1">
                              {result.table_description || 'No description available'}
                            </div>
                          </div>
                          <div className="text-right ml-3">
                            <div className="font-medium text-foreground">
                              {Math.round(result.similarity_score * 100)}%
                            </div>
                            <div className="text-muted-foreground text-xs">
                              similarity
                            </div>
                          </div>
                        </div>
                      </div>
                    ))}
                    {results.length > 5 && (
                      <div className="text-xs text-muted-foreground text-center">
                        ...and {results.length - 5} more results
                      </div>
                    )}
                  </div>
                )}
              </div>
            ) : (
              <div className="bg-error p-2 rounded border border-error-border">
                <div className="font-medium text-error-foreground text-xs">Search Failed</div>
                <div className="text-error-foreground text-xs">
                  {(() => {
                    // Try to extract error from output.data JSON string first
                    if (output.data) {
                      try {
                        // Handle CrewAI instruction contamination - extract just the JSON part
                        let jsonString = output.data;
                        const instructionMarker = '\n\n\nYou ONLY have access to the following tools';
                        if (jsonString.includes(instructionMarker)) {
                          jsonString = jsonString.split(instructionMarker)[0].trim();
                        }

                        const parsedData = JSON.parse(jsonString);
                        if (parsedData.error) return parsedData.error;
                      } catch {
                        // Fall through to other error sources
                      }
                    }
                    // Fallback to direct error field or unknown
                    return output.error || 'Unknown error';
                  })()}
                </div>
              </div>
            )}
          </div>
        </div>
      )}
    </div>
  );
};

const ValidateChartMLRenderer = ({ schema }) => {
  const { input, output } = schema;

  return (
    <div className="space-y-3">
      {input && (
        <div>
          <span className="font-medium text-foreground text-xs">ChartML:</span>
          <div className="mt-1 bg-muted p-2 rounded">
            <MarkdownRenderer className="text-xs">
              {`\`\`\`yaml\n${input.chartml || 'No ChartML provided'}\n\`\`\``}
            </MarkdownRenderer>
          </div>
        </div>
      )}

      {output && (
        <div>
          <span className="font-medium text-foreground text-xs">Validation Result:</span>
          <div className="mt-1">
            {output.success ? (
              <div className="space-y-2">
                <div className="bg-success p-2 rounded border border-success-border">
                  <div className="font-medium text-success-foreground text-xs">✅ ChartML is valid</div>
                </div>

                {/* Show cost info if SQL query was validated */}
                {(output.query_cost !== null || output.bytes_scanned !== null) && (
                  <div className="grid grid-cols-2 gap-2 text-xs">
                    {output.query_cost !== null && (
                      <div className="bg-muted p-2 rounded border border-border">
                        <div className="font-medium text-foreground">Query Cost</div>
                        <div className="text-muted-foreground">{output.query_cost.toFixed(2)} GB</div>
                      </div>
                    )}
                    {output.bytes_scanned !== null && (
                      <div className="bg-muted p-2 rounded border border-border">
                        <div className="font-medium text-foreground">Bytes Scanned</div>
                        <div className="text-muted-foreground">
                          {output.bytes_scanned > 1_000_000
                            ? `${(output.bytes_scanned / 1_000_000).toFixed(1)} MB`
                            : output.bytes_scanned > 1_000
                            ? `${(output.bytes_scanned / 1_000).toFixed(1)} KB`
                            : `${output.bytes_scanned} B`}
                        </div>
                      </div>
                    )}
                  </div>
                )}
              </div>
            ) : (
              <div className="bg-error p-2 rounded border border-error-border">
                <div className="font-medium text-error-foreground text-xs">❌ Validation Failed</div>
                <div className="text-error-foreground text-xs mt-1 whitespace-pre-wrap">
                  {output.error_message || 'Unknown validation error'}
                </div>
              </div>
            )}
          </div>
        </div>
      )}
    </div>
  );
};

const SaveLearningRenderer = ({ schema }) => {
  const { input, output } = schema;

  // Check if save was successful — output may be a string (error message) or object
  const hasError = output && (typeof output === 'string' || hasKey(output, 'error') || output.success === false);
  const isSuccess = output && output.success === true;

  return (
    <div className="space-y-3">
      {input && (
        <div>
          <span className="font-medium text-foreground text-xs">Learning:</span>
          <div className="mt-1 bg-muted p-3 rounded border border-border">
            <div className="text-sm text-foreground mb-2">{input.insight}</div>
            {input.context && (
              <div className="text-xs text-muted-foreground italic border-l-2 border-input pl-2 mb-2">
                {input.context}
              </div>
            )}
            {input.reference_queries?.length > 0 && (
              <div className="mt-2 pt-2 border-t border-border">
                <span className="text-xs font-medium text-muted-foreground">Reference Queries ({input.reference_queries.length})</span>
                <div className="space-y-2 mt-1">
                  {input.reference_queries.map((rq, idx) => (
                    <div key={idx} className="bg-background rounded border border-border p-2">
                      {rq.comment && <div className="text-xs font-medium text-foreground mb-1">{rq.comment}</div>}
                      <pre className="font-mono text-xs text-muted-foreground whitespace-pre-wrap break-all">{rq.sql}</pre>
                      {rq.datasource && (
                        <span className="inline-block mt-1 px-1.5 py-0.5 rounded text-[10px] bg-secondary text-secondary-foreground">
                          {rq.datasource}
                        </span>
                      )}
                    </div>
                  ))}
                </div>
              </div>
            )}
          </div>
        </div>
      )}

      {output && (
        <div>
          {isSuccess ? (
            <div className="bg-success p-2 rounded border border-success-border">
              <div className="text-success-foreground text-xs flex items-center gap-1">
                <span>✓</span>
                <span>{output.message || 'Learning saved successfully'}</span>
              </div>
            </div>
          ) : hasError ? (
            <div className="bg-error p-2 rounded border border-error-border">
              <div className="font-medium text-error-foreground text-xs">
                {output.rejected ? 'Rejected' : 'Error'}
              </div>
              <div className="text-error-foreground text-xs">
                {output.reason || output.message || output.error || 'Failed to save learning'}
              </div>
            </div>
          ) : null}
        </div>
      )}
    </div>
  );
};

const SearchKnowledgeRenderer = ({ schema }) => {
  const { input, output } = schema;

  const hasError = output && hasKey(output, 'error');
  const results = output?.results || [];

  const typeLabel = (type) => {
    switch (type) {
      case 'table': return 'Table';
      case 'learning': return 'Learning';
      case 'metric': return 'Metric';
      default: return type;
    }
  };

  const typeBadgeClass = (type) => {
    switch (type) {
      case 'table': return 'bg-primary/10 text-primary';
      case 'learning': return 'bg-warning/10 text-warning-foreground';
      case 'metric': return 'bg-success/10 text-success-foreground';
      default: return 'bg-muted text-muted-foreground';
    }
  };

  return (
    <div className="space-y-2">
      {input && (
        <div>
          <span className="font-medium text-foreground text-xs">Search Query:</span>
          <div className="mt-1 bg-muted p-2 rounded border border-border">
            <div className="text-xs text-foreground font-medium">"{input.query}"</div>
            <div className="mt-1 flex gap-2 text-xs text-muted-foreground">
              {input.datasource && (
                <span className="text-primary">Datasource: {input.datasource}</span>
              )}
              {input.limit && input.limit !== 10 && (
                <span>Limit: {input.limit}</span>
              )}
            </div>
          </div>
        </div>
      )}

      {output && (
        <div>
          <span className="font-medium text-foreground text-xs">Results:</span>
          <div className="mt-1">
            {hasError ? (
              <div className="bg-error p-2 rounded border border-error-border">
                <div className="font-medium text-error-foreground text-xs">Error</div>
                <div className="text-error-foreground text-xs">{output.error}</div>
              </div>
            ) : results.length === 0 ? (
              <div className="bg-muted p-2 rounded border border-border text-xs text-muted-foreground">
                No results found
              </div>
            ) : (
              <div className="space-y-1">
                <div className="bg-muted p-2 rounded border border-border text-xs text-muted-foreground">
                  Found {output.found} result(s){output.source && ` via ${output.source}`}
                </div>
                {results.slice(0, 8).map((result, idx) => (
                  <div key={idx} className="bg-card border border-border rounded p-2 text-xs">
                    <div className="flex items-start justify-between">
                      <div className="flex-1">
                        <div className="flex items-center gap-2">
                          <span className={`px-1.5 py-0.5 rounded text-xs font-medium ${typeBadgeClass(result.type)}`}>
                            {typeLabel(result.type)}
                          </span>
                          <span className="font-mono text-foreground text-xs truncate">{result.id}</span>
                        </div>
                        <div className="text-foreground mt-1">{result.text}</div>
                        {result.matched_columns?.length > 0 && (
                          <div className="text-muted-foreground text-xs mt-1">
                            Matched columns: {result.matched_columns.map(c => c.name).join(', ')}
                          </div>
                        )}
                      </div>
                      <div className="text-right ml-2 flex-shrink-0">
                        <div className="text-xs text-muted-foreground">
                          {result.score}
                        </div>
                      </div>
                    </div>
                  </div>
                ))}
                {results.length > 8 && (
                  <div className="text-xs text-muted-foreground text-center">
                    ...and {results.length - 8} more
                  </div>
                )}
              </div>
            )}
          </div>
        </div>
      )}
    </div>
  );
};

const UpdateDashboardRenderer = ({ schema }) => {
  const { input, output } = schema;

  // Check if update was successful
  const hasError = output && (typeof output === 'string' || (typeof output === 'object' && (hasKey(output, 'error') || output.success === false)));
  const isSuccess = output && typeof output === 'object' && output.success === true;

  return (
    <div className="space-y-3">
      {input && (
        <div>
          <span className="font-medium text-foreground text-xs">Dashboard Update:</span>
          <div className="mt-1 bg-muted p-3 rounded border border-border">
            {input.summary && (
              <div className="text-sm text-foreground mb-2">{input.summary}</div>
            )}
            {input.content && (
              <details className="text-xs">
                <summary className="cursor-pointer text-muted-foreground hover:text-foreground">
                  View content ({input.content.length} chars)
                </summary>
                <pre className="mt-2 p-2 bg-accent rounded text-xs overflow-x-auto max-h-40 overflow-y-auto">
                  {input.content}
                </pre>
              </details>
            )}
          </div>
        </div>
      )}

      {output && (
        <div>
          {isSuccess ? (
            <div className="bg-success p-2 rounded border border-success-border">
              <div className="text-success-foreground text-xs flex items-center gap-1">
                <span>✓</span>
                <span>{output.message || 'Dashboard updated successfully'}</span>
              </div>
            </div>
          ) : hasError ? (
            <div className="bg-error p-2 rounded border border-error-border">
              <div className="font-medium text-error-foreground text-xs">Error</div>
              <div className="text-error-foreground text-xs">
                {typeof output === 'string' ? output : (output.error || 'Failed to update dashboard')}
              </div>
            </div>
          ) : null}
        </div>
      )}
    </div>
  );
};

const GetChartMLSpecRenderer = ({ schema }) => {
  const { input, output } = schema;

  const hasError = output && (hasKey(output, 'error') || output.success === false);
  const isSuccess = output && output.success === true;

  return (
    <div className="space-y-3">
      {input && (
        <div>
          <span className="font-medium text-foreground text-xs">ChartML Spec Lookup:</span>
          <div className="mt-1 bg-muted p-2 rounded border border-border">
            <span className="text-sm text-foreground">Section: {input.section}</span>
          </div>
        </div>
      )}

      {output && (
        <div>
          {isSuccess ? (
            <div className="bg-success p-2 rounded border border-success-border">
              <div className="text-success-foreground text-xs flex items-center gap-1 mb-2">
                <span>✓</span>
                <span>Spec loaded{output.section ? ` (${output.section})` : ''}</span>
              </div>
              {output.content && (
                <details className="text-xs">
                  <summary className="cursor-pointer text-success-foreground hover:text-success-foreground">
                    View content ({output.content.length} chars)
                  </summary>
                  <pre className="mt-2 p-2 bg-success rounded text-xs overflow-x-auto max-h-40 overflow-y-auto text-success-foreground">
                    {output.content.slice(0, 2000)}{output.content.length > 2000 ? '...' : ''}
                  </pre>
                </details>
              )}
            </div>
          ) : hasError ? (
            <div className="bg-error p-2 rounded border border-error-border">
              <div className="font-medium text-error-foreground text-xs">Error</div>
              <div className="text-error-foreground text-xs">
                {output.error || 'Failed to load ChartML spec'}
              </div>
            </div>
          ) : null}
        </div>
      )}
    </div>
  );
};

const UpdateChartRenderer = ({ schema }) => {
  const { input, output } = schema;

  // Check if update was successful
  const hasError = output && (typeof output === 'string' || (typeof output === 'object' && (hasKey(output, 'error') || output.success === false)));
  const isSuccess = output && typeof output === 'object' && output.success === true;

  return (
    <div className="space-y-3">
      {input && (
        <div>
          <span className="font-medium text-foreground text-xs">Chart Update:</span>
          <div className="mt-1 bg-muted p-3 rounded border border-border">
            {input.summary && (
              <div className="text-sm text-foreground mb-2">{input.summary}</div>
            )}
            {input.content && (
              <details className="text-xs">
                <summary className="cursor-pointer text-muted-foreground hover:text-foreground">
                  View content ({input.content.length} chars)
                </summary>
                <pre className="mt-2 p-2 bg-accent rounded text-xs overflow-x-auto max-h-40 overflow-y-auto">
                  {input.content}
                </pre>
              </details>
            )}
          </div>
        </div>
      )}

      {output && (
        <div>
          {isSuccess ? (
            <div className="bg-success p-2 rounded border border-success-border">
              <div className="text-success-foreground text-xs flex items-center gap-1">
                <span>✓</span>
                <span>{output.message || 'Chart updated successfully'}</span>
              </div>
            </div>
          ) : hasError ? (
            <div className="bg-error p-2 rounded border border-error-border">
              <div className="font-medium text-error-foreground text-xs">Error</div>
              <div className="text-error-foreground text-xs">
                {typeof output === 'string' ? output : (output.error || 'Failed to update chart')}
              </div>
            </div>
          ) : null}
        </div>
      )}
    </div>
  );
};

const ValidateSQLRenderer = ({ schema }) => {
  const { input, output } = schema;

  return (
    <div className="space-y-3">
      {input && (
        <div>
          <span className="font-medium text-foreground text-xs">SQL Query:</span>
          <div className="mt-1 bg-muted p-2 rounded">
            <MarkdownRenderer className="text-xs">
              {`\`\`\`sql\n${input.sql || 'No SQL provided'}\n\`\`\``}
            </MarkdownRenderer>
          </div>
        </div>
      )}

      {output && (
        <div>
          <span className="font-medium text-foreground text-xs">Validation Result:</span>
          <div className="mt-1">
            {output.success ? (
              <div className="space-y-2">
                <div className="bg-success p-2 rounded border border-success-border">
                  <div className="font-medium text-success-foreground text-xs">✅ SQL is valid</div>
                </div>

                {/* Show cost info if available */}
                {output.query_cost_gb !== null && output.query_cost_gb !== undefined && (
                  <div className="bg-muted p-2 rounded border border-border">
                    <div className="font-medium text-foreground text-xs">Query Cost</div>
                    <div className="text-muted-foreground text-xs">{output.query_cost_gb.toFixed(2)} GB</div>
                  </div>
                )}
              </div>
            ) : (
              <div className="bg-error p-2 rounded border border-error-border">
                <div className="font-medium text-error-foreground text-xs">❌ Validation Failed</div>
                <div className="text-error-foreground text-xs mt-1 whitespace-pre-wrap">
                  {output.error_message || 'Unknown validation error'}
                </div>
              </div>
            )}
          </div>
        </div>
      )}
    </div>
  );
};

const ListDatasourcesRenderer = ({ schema }) => {
  const { output } = schema;

  const hasError = output && hasKey(output, 'error');
  const datasources = output?.datasources || [];

  return (
    <div className="space-y-2">
      <span className="font-medium text-foreground text-xs">Available Datasources:</span>
      <div className="mt-1">
        {hasError ? (
          <div className="bg-error p-2 rounded border border-error-border">
            <div className="font-medium text-error-foreground text-xs">Error</div>
            <div className="text-error-foreground text-xs">{output.error}</div>
          </div>
        ) : datasources.length === 0 ? (
          <div className="bg-muted p-2 rounded border border-border text-xs text-muted-foreground">
            {output?.message || 'No datasources configured'}
          </div>
        ) : (
          <div className="space-y-1">
            {datasources.map((ds, idx) => (
              <div key={idx} className="bg-muted p-2 rounded border border-border text-xs flex items-center justify-between">
                <div className="flex items-center gap-2">
                  <span className="font-medium text-foreground">{ds.name}</span>
                  <span className="text-muted-foreground font-mono text-xs">({ds.slug})</span>
                </div>
                <div className="flex items-center gap-3 text-muted-foreground">
                  <span className="bg-muted px-1.5 py-0.5 rounded text-xs">{ds.type}</span>
                  <span>{ds.tables_indexed} tables</span>
                </div>
              </div>
            ))}
          </div>
        )}
      </div>
    </div>
  );
};

const CreateWatchRenderer = ({ schema }) => {
  const { input, output } = schema;

  const hasError = output && hasKey(output, 'error');
  const upgradeRequired = output?.upgrade_required;

  return (
    <div className="space-y-2">
      {input && (
        <div>
          <span className="font-medium text-foreground text-xs">Creating Watch:</span>
          <div className="mt-1 bg-muted p-2 rounded border border-border text-xs space-y-1">
            {input.name && <div><span className="text-muted-foreground">Name:</span> <span className="font-medium">{input.name}</span></div>}
            {input.schedule && <div><span className="text-muted-foreground">Schedule:</span> <span>{describeCron(input.schedule).description}</span></div>}
            {input.prompt && <div><span className="text-muted-foreground">Prompt:</span> <span className="text-foreground">{input.prompt}</span></div>}
            {input.queries && input.queries.length > 0 && (
              <div>
                <span className="text-muted-foreground">Reference Queries ({input.queries.length}):</span>
                <div className="mt-1 space-y-1">
                  {input.queries.map((q, idx) => (
                    <div key={idx} className="bg-card p-1 rounded border border-border">
                      <div className="text-foreground font-medium">{q.comment}</div>
                      {q.datasource && <div className="text-muted-foreground text-xs">Datasource: {q.datasource}</div>}
                    </div>
                  ))}
                </div>
              </div>
            )}
          </div>
        </div>
      )}
      {output && (
        <div>
          {hasError ? (
            <div className={`p-2 rounded border text-xs ${upgradeRequired ? 'bg-warning/10 border-warning' : 'bg-error/10 border-error'}`}>
              <div className={`font-medium ${upgradeRequired ? 'text-warning-foreground' : 'text-error-foreground'}`}>
                {upgradeRequired ? '⬆️ Upgrade Required' : '❌ Error'}
              </div>
              <div className={`mt-1 ${upgradeRequired ? 'text-warning-foreground' : 'text-error-foreground'}`}>
                {output.message || output.error}
              </div>
            </div>
          ) : (
            <div className="bg-success/10 p-2 rounded border border-success text-xs">
              <div className="font-medium text-success-foreground">✅ Watch Created</div>
              {output.name && <div className="mt-1 text-success-foreground">Name: {output.name}</div>}
              {output.schedule && <div className="text-success-foreground">Schedule: {describeCron(output.schedule).description}</div>}
              {output.next_run_at && <div className="text-success-foreground">Next run: {output.next_run_at}</div>}
            </div>
          )}
        </div>
      )}
    </div>
  );
};

const PreviewWatchRenderer = ({ schema }) => {
  const { input, output } = schema;

  const hasError = output && hasKey(output, 'error');
  const preview = output?.preview;

  return (
    <div className="space-y-2">
      {input && (
        <div>
          <span className="font-medium text-foreground text-xs">Watch Preview:</span>
          <div className="mt-1 bg-muted p-2 rounded border border-border text-xs space-y-1">
            {input.name && <div><span className="text-muted-foreground">Name:</span> <span className="font-medium">{input.name}</span></div>}
            {input.schedule && <div><span className="text-muted-foreground">Schedule:</span> <span>{describeCron(input.schedule).description}</span></div>}
            {input.prompt && <div><span className="text-muted-foreground">Prompt:</span> <span className="text-foreground">{input.prompt}</span></div>}
            {input.queries && input.queries.length > 0 && (
              <div>
                <span className="text-muted-foreground">Reference Queries ({input.queries.length}):</span>
                <div className="mt-1 space-y-1">
                  {input.queries.map((q, idx) => (
                    <div key={idx} className="bg-card p-1 rounded border border-border">
                      <div className="text-foreground font-medium">{q.comment}</div>
                      {q.datasource && <div className="text-muted-foreground text-xs">Datasource: {q.datasource}</div>}
                    </div>
                  ))}
                </div>
              </div>
            )}
          </div>
        </div>
      )}
      {output && (
        <div>
          {hasError ? (
            <div className="bg-error/10 p-2 rounded border border-error text-xs">
              <div className="font-medium text-error-foreground">❌ Error</div>
              <div className="mt-1 text-error-foreground">{output.error}</div>
            </div>
          ) : (
            <div className="bg-primary/10 p-2 rounded border border-primary text-xs">
              <div className="font-medium text-primary">👁️ Preview Generated</div>
              {preview?.schedule && <div className="mt-1 text-foreground">Schedule: {describeCron(preview.schedule).description}</div>}
              <div className="mt-1 text-muted-foreground text-xs">{output.message}</div>
            </div>
          )}
        </div>
      )}
    </div>
  );
};

const UpdateWatchRenderer = ({ schema }) => {
  const { input, output } = schema;

  const hasError = output && hasKey(output, 'error');

  return (
    <div className="space-y-2">
      {input && (
        <div>
          <span className="font-medium text-foreground text-xs">Updating Watch:</span>
          <div className="mt-1 bg-muted p-2 rounded border border-border text-xs space-y-1">
            {input.watch_id && <div><span className="text-muted-foreground">Watch ID:</span> <span className="font-mono text-xs">{input.watch_id}</span></div>}
            {input.name && <div><span className="text-muted-foreground">Name:</span> <span className="font-medium">{input.name}</span></div>}
            {input.schedule && <div><span className="text-muted-foreground">Schedule:</span> <span>{describeCron(input.schedule).description}</span></div>}
            {input.prompt && <div><span className="text-muted-foreground">Prompt:</span> <span className="text-foreground">{input.prompt}</span></div>}
            {input.enabled !== undefined && <div><span className="text-muted-foreground">Enabled:</span> <span>{input.enabled ? 'Yes' : 'No'}</span></div>}
          </div>
        </div>
      )}
      {output && (
        <div>
          {hasError ? (
            <div className="bg-error/10 p-2 rounded border border-error text-xs">
              <div className="font-medium text-error-foreground">❌ Error</div>
              <div className="mt-1 text-error-foreground">{output.error}</div>
            </div>
          ) : (
            <div className="bg-success/10 p-2 rounded border border-success text-xs">
              <div className="font-medium text-success-foreground">✅ Watch Updated</div>
              {output.name && <div className="mt-1 text-success-foreground">Name: {output.name}</div>}
              {output.schedule && <div className="text-success-foreground">Schedule: {describeCron(output.schedule).description}</div>}
              {output.next_run_at && <div className="text-success-foreground">Next run: {output.next_run_at}</div>}
            </div>
          )}
        </div>
      )}
    </div>
  );
};

const SearchWatchesRenderer = ({ schema }) => {
  const { input, output } = schema;

  const hasError = output && hasKey(output, 'error');
  const watches = output?.watches || [];

  return (
    <div className="space-y-2">
      {input && (
        <div>
          <span className="font-medium text-foreground text-xs">Searching Watches:</span>
          <div className="mt-1 bg-muted p-2 rounded border border-border text-xs space-y-1">
            {input.query && <div><span className="text-muted-foreground">Query:</span> <span className="font-medium">{input.query}</span></div>}
            {input.limit && <div><span className="text-muted-foreground">Limit:</span> <span>{input.limit}</span></div>}
          </div>
        </div>
      )}
      {output && (
        <div>
          {hasError ? (
            <div className="bg-error/10 p-2 rounded border border-error text-xs">
              <div className="font-medium text-error-foreground">❌ Error</div>
              <div className="mt-1 text-error-foreground">{output.error}</div>
            </div>
          ) : watches.length === 0 ? (
            <div className="bg-accent p-2 rounded border border-input text-xs">
              <div className="text-muted-foreground">No watches found</div>
              {output.message && <div className="mt-1 text-muted-foreground text-xs">{output.message}</div>}
            </div>
          ) : (
            <div className="bg-success/10 p-2 rounded border border-success text-xs space-y-2">
              <div className="font-medium text-success-foreground">✅ Found {watches.length} watch{watches.length !== 1 ? 'es' : ''}</div>
              {output.total_workspace_watches && (
                <div className="text-muted-foreground text-xs">Total watches in workspace: {output.total_workspace_watches}</div>
              )}
              <div className="space-y-2 mt-2 max-h-60 overflow-y-auto">
                {watches.map((watch, idx) => (
                  <div key={idx} className="bg-card p-2 rounded border border-border">
                    <div className="font-medium text-foreground">{watch.name}</div>
                    <div className="text-muted-foreground text-xs mt-1">{watch.prompt}</div>
                    <div className="text-muted-foreground text-xs mt-1 space-y-0.5">
                      <div>Schedule: {describeCron(watch.schedule).description}</div>
                      <div>Status: <span className={watch.status === 'active' ? 'text-success' : 'text-muted-foreground'}>{watch.status}</span></div>
                      {watch.last_executed && <div>Last executed: {new Date(watch.last_executed).toLocaleString()}</div>}
                      {watch.last_status && <div>Last status: {watch.last_status}</div>}
                      {watch.queries && watch.queries.length > 0 && <div className="text-muted-foreground">{watch.queries.length} reference queries</div>}
                    </div>
                  </div>
                ))}
              </div>
            </div>
          )}
        </div>
      )}
    </div>
  );
};

const DeleteWatchRenderer = ({ schema }) => {
  const { input, output } = schema;

  const hasError = output && hasKey(output, 'error');

  return (
    <div className="space-y-2">
      {input && (
        <div>
          <span className="font-medium text-foreground text-xs">Deleting Watch:</span>
          <div className="mt-1 bg-muted p-2 rounded border border-border text-xs space-y-1">
            {input.watch_id && <div><span className="text-muted-foreground">Watch ID:</span> <span className="font-mono text-xs">{input.watch_id}</span></div>}
          </div>
        </div>
      )}
      {output && (
        <div>
          {hasError ? (
            <div className="bg-error/10 p-2 rounded border border-error text-xs">
              <div className="font-medium text-error-foreground">❌ Error</div>
              <div className="mt-1 text-error-foreground">{output.error}</div>
            </div>
          ) : (
            <div className="bg-success/10 p-2 rounded border border-success text-xs">
              <div className="font-medium text-success-foreground">✅ Watch Deleted</div>
              {output.message && <div className="mt-1 text-success-foreground">{output.message}</div>}
            </div>
          )}
        </div>
      )}
    </div>
  );
};

const TriggerWatchRenderer = ({ schema }) => {
  const { input, output } = schema;

  const hasError = output && hasKey(output, 'error');

  return (
    <div className="space-y-2">
      {input && (
        <div>
          <span className="font-medium text-foreground text-xs">Triggering Watch:</span>
          <div className="mt-1 bg-muted p-2 rounded border border-border text-xs space-y-1">
            {input.watch_id && <div><span className="text-muted-foreground">Watch ID:</span> <span className="font-mono text-xs">{input.watch_id}</span></div>}
          </div>
        </div>
      )}
      {output && (
        <div>
          {hasError ? (
            <div className="bg-error/10 p-2 rounded border border-error text-xs">
              <div className="font-medium text-error-foreground">❌ Error</div>
              <div className="mt-1 text-error-foreground">{output.error}</div>
            </div>
          ) : (
            <div className="bg-success/10 p-2 rounded border border-success text-xs">
              <div className="font-medium text-success-foreground">✅ Watch Triggered</div>
              {output.name && <div className="mt-1 text-success-foreground">Watch: {output.name}</div>}
              {output.message && <div className="text-success-foreground">{output.message}</div>}
              {output.scheduled_for && <div className="text-success-foreground">Scheduled for: {output.scheduled_for}</div>}
            </div>
          )}
        </div>
      )}
    </div>
  );
};

const WatchInfoRenderer = ({ schema }) => {
  const { input, output } = schema;

  const hasError = output && hasKey(output, 'error');
  const watch = output?.watch;
  const executions = output?.recent_executions || [];

  return (
    <div className="space-y-2">
      {input && (
        <div>
          <span className="font-medium text-foreground text-xs">Getting Watch Info:</span>
          <div className="mt-1 bg-muted p-2 rounded border border-border text-xs">
            <span className="text-muted-foreground">Watch ID:</span> <span className="font-mono">{input.watch_id}</span>
          </div>
        </div>
      )}
      {output && (
        <div>
          {hasError ? (
            <div className="bg-error/10 p-2 rounded border border-error text-xs">
              <div className="font-medium text-error-foreground">❌ Error</div>
              <div className="mt-1 text-error-foreground">{output.error}</div>
            </div>
          ) : watch ? (
            <div className="bg-success/10 p-2 rounded border border-success text-xs space-y-3">
              <div>
                <div className="font-medium text-success-foreground">✅ Watch Found</div>
                <div className="bg-card p-2 rounded border border-border mt-2 space-y-1">
                  <div className="font-medium text-foreground">{watch.name}</div>
                  <div className="text-muted-foreground text-xs">{watch.prompt}</div>
                  <div className="text-muted-foreground text-xs space-y-0.5 mt-2">
                    <div>Mode: <span className="font-medium">{watch.mode}</span></div>
                    <div>Schedule: {describeCron(watch.schedule).description}</div>
                    <div>Status: <span className={watch.enabled ? 'text-success' : 'text-muted-foreground'}>{watch.enabled ? 'Active' : 'Paused'}</span></div>
                    {watch.last_run_at && <div>Last run: {new Date(watch.last_run_at).toLocaleString()}</div>}
                    {watch.next_run_at && <div>Next run: {new Date(watch.next_run_at).toLocaleString()}</div>}
                  </div>
                </div>
              </div>

              {executions.length > 0 && (
                <div>
                  <div className="font-medium text-foreground text-xs mb-1">Recent Executions ({executions.length})</div>
                  <div className="space-y-1 max-h-40 overflow-y-auto">
                    {executions.map((exec, idx) => (
                      <div key={idx} className="bg-card p-2 rounded border border-border text-xs">
                        <div className="flex items-center gap-2">
                          <span>{exec.status === 'success' ? '✅' : exec.status === 'error' ? '❌' : '⚪'}</span>
                          <span className="text-muted-foreground">{exec.timestamp ? new Date(exec.timestamp).toLocaleString() : 'Unknown'}</span>
                          {exec.alert_triggered && <span className="bg-primary/10 text-primary px-1 rounded text-xs">alerted</span>}
                        </div>
                        {exec.alert_title && <div className="mt-1 font-medium text-foreground">{exec.alert_title}</div>}
                        {exec.error_message && <div className="mt-1 text-error">{exec.error_message}</div>}
                      </div>
                    ))}
                  </div>
                </div>
              )}
            </div>
          ) : (
            <div className="bg-accent p-2 rounded border border-input text-xs">
              <div className="text-muted-foreground">No watch data returned</div>
            </div>
          )}
        </div>
      )}
    </div>
  );
};

// ============================================================================
// Dashboard Tool Renderers
// ============================================================================

const SearchDashboardsRenderer = ({ schema }) => {
  const { input, output } = schema;

  const hasError = output && hasKey(output, 'error');
  const dashboards = output?.dashboards || [];

  return (
    <div className="space-y-2">
      {input && (
        <div>
          <span className="font-medium text-foreground text-xs">Searching Dashboards:</span>
          <div className="mt-1 bg-muted p-2 rounded border border-border text-xs space-y-1">
            {input.query && <div><span className="text-muted-foreground">Query:</span> <span className="font-medium">{input.query}</span></div>}
            {input.sort_by && <div><span className="text-muted-foreground">Sort by:</span> <span>{input.sort_by}</span></div>}
            {input.limit && <div><span className="text-muted-foreground">Limit:</span> <span>{input.limit}</span></div>}
            {input.top_popular && <div><span className="text-muted-foreground">Mode:</span> <span>Top 10 Popular</span></div>}
          </div>
        </div>
      )}
      {output && (
        <div>
          {hasError ? (
            <div className="bg-error/10 p-2 rounded border border-error text-xs">
              <div className="font-medium text-error-foreground">❌ Error</div>
              <div className="mt-1 text-error-foreground">{output.error}</div>
            </div>
          ) : dashboards.length === 0 ? (
            <div className="bg-accent p-2 rounded border border-input text-xs">
              <div className="text-muted-foreground">No dashboards found</div>
            </div>
          ) : (
            <div className="bg-success/10 p-2 rounded border border-success text-xs space-y-2">
              <div className="font-medium text-success-foreground">✅ Found {dashboards.length} dashboard{dashboards.length !== 1 ? 's' : ''}</div>
              {output.total_workspace_dashboards && (
                <div className="text-muted-foreground text-xs">Total dashboards in workspace: {output.total_workspace_dashboards}</div>
              )}
              <div className="space-y-2 mt-2 max-h-60 overflow-y-auto">
                {dashboards.map((dashboard, idx) => (
                  <div key={idx} className="bg-card p-2 rounded border border-border">
                    <div className="font-medium text-foreground">{dashboard.title}</div>
                    {dashboard.content && (
                      <div className="text-muted-foreground text-xs mt-1 line-clamp-2">{dashboard.content}</div>
                    )}
                    <div className="text-muted-foreground text-xs mt-1 space-y-0.5">
                      <div>ID: <span className="font-mono text-xs">{dashboard.dashboard_id}</span></div>
                      {dashboard.total_views !== undefined && <div>Views: {dashboard.total_views} total, {dashboard.recent_views} recent</div>}
                      {dashboard.popularity_score !== undefined && <div>Popularity: {dashboard.popularity_score}</div>}
                      {dashboard.updated_at && <div>Updated: {new Date(dashboard.updated_at).toLocaleString()}</div>}
                    </div>
                  </div>
                ))}
              </div>
            </div>
          )}
        </div>
      )}
    </div>
  );
};

const GetDashboardInfoRenderer = ({ schema }) => {
  const { input, output } = schema;

  const hasError = output && hasKey(output, 'error');

  return (
    <div className="space-y-2">
      {input && (
        <div>
          <span className="font-medium text-foreground text-xs">Getting Dashboard Info:</span>
          <div className="mt-1 bg-muted p-2 rounded border border-border text-xs">
            <span className="text-muted-foreground">Dashboard ID:</span> <span className="font-mono">{input.dashboard_id}</span>
          </div>
        </div>
      )}
      {output && (
        <div>
          {hasError ? (
            <div className="bg-error/10 p-2 rounded border border-error text-xs">
              <div className="font-medium text-error-foreground">❌ Error</div>
              <div className="mt-1 text-error-foreground">{output.error}</div>
            </div>
          ) : output.success ? (
            <div className="bg-success/10 p-2 rounded border border-success text-xs space-y-2">
              <div className="font-medium text-success-foreground">✅ {output.message || 'Dashboard Retrieved'}</div>
              <div className="bg-card p-2 rounded border border-border space-y-2">
                <div className="font-medium text-foreground">{output.title}</div>
                <div className="text-muted-foreground text-xs space-y-0.5">
                  <div>ID: <span className="font-mono">{output.dashboard_id}</span></div>
                  {output.created_at && <div>Created: {new Date(output.created_at).toLocaleString()}</div>}
                  {output.updated_at && <div>Updated: {new Date(output.updated_at).toLocaleString()}</div>}
                  {output.last_change_summary && <div>Last change: {output.last_change_summary}</div>}
                </div>
                {output.content && (
                  <div className="bg-muted p-2 rounded border border-border text-xs max-h-40 overflow-y-auto">
                    <div className="text-muted-foreground mb-1">Content:</div>
                    <pre className="whitespace-pre-wrap font-mono text-xs">{output.content}</pre>
                  </div>
                )}
              </div>
            </div>
          ) : (
            <div className="bg-accent p-2 rounded border border-input text-xs">
              <div className="text-muted-foreground">No dashboard data returned</div>
            </div>
          )}
        </div>
      )}
    </div>
  );
};

const CreateDashboardRenderer = ({ schema }) => {
  const { input, output } = schema;

  const hasError = output && hasKey(output, 'error');
  const upgradeRequired = output?.upgrade_required;

  return (
    <div className="space-y-2">
      {input && (
        <div>
          <span className="font-medium text-foreground text-xs">Creating Dashboard:</span>
          <div className="mt-1 bg-muted p-2 rounded border border-border text-xs space-y-1">
            <div><span className="text-muted-foreground">Title:</span> <span className="font-medium">{input.title}</span></div>
            {input.content && <div><span className="text-muted-foreground">Content:</span> <span className="text-xs">{input.content.substring(0, 100)}{input.content.length > 100 ? '...' : ''}</span></div>}
            {input.verified_no_duplicates && <div className="text-success text-xs">✅ Verified no duplicates</div>}
          </div>
        </div>
      )}
      {output && (
        <div>
          {hasError ? (
            <div className={upgradeRequired ? "bg-warning/10 p-2 rounded border border-warning text-xs" : "bg-error/10 p-2 rounded border border-error text-xs"}>
              <div className={upgradeRequired ? "font-medium text-warning-foreground" : "font-medium text-error-foreground"}>
                {upgradeRequired ? '⚠️ Upgrade Required' : '❌ Error'}
              </div>
              <div className="mt-1">{output.error}</div>
            </div>
          ) : output.success ? (
            <div className="bg-success/10 p-2 rounded border border-success text-xs space-y-2">
              <div className="font-medium text-success-foreground">✅ {output.message || 'Dashboard Created'}</div>
              <div className="bg-card p-2 rounded border border-border">
                <div className="font-medium text-foreground">{output.title}</div>
                <div className="text-muted-foreground text-xs mt-1">
                  ID: <span className="font-mono">{output.dashboard_id}</span>
                </div>
              </div>
            </div>
          ) : null}
        </div>
      )}
    </div>
  );
};

const ModifyDashboardRenderer = ({ schema }) => {
  const { input, output } = schema;

  const hasError = output && hasKey(output, 'error');

  return (
    <div className="space-y-2">
      {input && (
        <div>
          <span className="font-medium text-foreground text-xs">Modifying Dashboard:</span>
          <div className="mt-1 bg-muted p-2 rounded border border-border text-xs space-y-1">
            <div><span className="text-muted-foreground">Dashboard ID:</span> <span className="font-mono">{input.dashboard_id}</span></div>
            {input.title && <div><span className="text-muted-foreground">New Title:</span> <span className="font-medium">{input.title}</span></div>}
            {input.content && <div><span className="text-muted-foreground">New Content:</span> <span className="text-xs">{input.content.substring(0, 100)}{input.content.length > 100 ? '...' : ''}</span></div>}
            {input.change_summary && <div><span className="text-muted-foreground">Summary:</span> <span>{input.change_summary}</span></div>}
          </div>
        </div>
      )}
      {output && (
        <div>
          {hasError ? (
            <div className="bg-error/10 p-2 rounded border border-error text-xs">
              <div className="font-medium text-error-foreground">❌ Error</div>
              <div className="mt-1 text-error-foreground">{output.error}</div>
            </div>
          ) : output.success ? (
            <div className="bg-success/10 p-2 rounded border border-success text-xs space-y-2">
              <div className="font-medium text-success-foreground">✅ {output.message || 'Dashboard Updated'}</div>
              <div className="bg-card p-2 rounded border border-border">
                <div className="font-medium text-foreground">{output.title}</div>
                <div className="text-muted-foreground text-xs mt-1">
                  ID: <span className="font-mono">{output.dashboard_id}</span>
                </div>
              </div>
            </div>
          ) : null}
        </div>
      )}
    </div>
  );
};

const DeleteDashboardRenderer = ({ schema }) => {
  const { input, output } = schema;

  const hasError = output && hasKey(output, 'error');

  return (
    <div className="space-y-2">
      {input && (
        <div>
          <span className="font-medium text-foreground text-xs">Deleting Dashboard:</span>
          <div className="mt-1 bg-muted p-2 rounded border border-border text-xs">
            <span className="text-muted-foreground">Dashboard ID:</span> <span className="font-mono">{input.dashboard_id}</span>
          </div>
        </div>
      )}
      {output && (
        <div>
          {hasError ? (
            <div className="bg-error/10 p-2 rounded border border-error text-xs">
              <div className="font-medium text-error-foreground">❌ Error</div>
              <div className="mt-1 text-error-foreground">{output.error}</div>
            </div>
          ) : output.success ? (
            <div className="bg-success/10 p-2 rounded border border-success text-xs">
              <div className="font-medium text-success-foreground">✅ {output.message || 'Dashboard Deleted'}</div>
              <div className="text-muted-foreground text-xs mt-1">
                ID: <span className="font-mono">{output.dashboard_id}</span>
              </div>
            </div>
          ) : null}
        </div>
      )}
    </div>
  );
};

const GetWorkspaceInfoRenderer = ({ schema }) => {
  const { input, output } = schema;

  const hasError = output && hasKey(output, 'error');

  return (
    <div className="space-y-2">
      {input && Object.keys(input).length > 0 && (
        <div>
          <span className="font-medium text-foreground text-xs">Getting Workspace Info</span>
        </div>
      )}
      {output && (
        <div>
          {hasError ? (
            <div className="bg-error/10 p-2 rounded border border-error text-xs">
              <div className="font-medium text-error-foreground">❌ Error</div>
              <div className="mt-1 text-error-foreground">{output.error}</div>
            </div>
          ) : output.workspace_name ? (
            <div className="bg-success/10 p-2 rounded border border-success text-xs space-y-2">
              <div className="font-medium text-success-foreground">✅ {output.message || 'Workspace Info Retrieved'}</div>
              <div className="bg-card p-2 rounded border border-border space-y-2">
                <div className="font-medium text-foreground">{output.workspace_name}</div>
                <div className="text-muted-foreground text-xs space-y-0.5">
                  <div>Members: {output.member_count}</div>
                  {output.current_user_email && <div>Your email: <span className="font-mono">{output.current_user_email}</span></div>}
                </div>
                {output.members && output.members.length > 0 && (
                  <div className="bg-muted p-2 rounded border border-border text-xs max-h-40 overflow-y-auto">
                    <div className="text-muted-foreground mb-1">Members:</div>
                    <div className="space-y-1">
                      {output.members.map((member, idx) => (
                        <div key={idx} className="flex items-center gap-2">
                          <span className="font-medium">{member.name}</span>
                          <span className="text-muted-foreground">({member.role})</span>
                          <span className="font-mono text-muted-foreground">{member.email}</span>
                          {member.is_current_user && <span className="text-xs bg-primary/10 text-primary px-1 rounded">you</span>}
                        </div>
                      ))}
                    </div>
                  </div>
                )}
              </div>
            </div>
          ) : (
            <div className="bg-accent p-2 rounded border border-input text-xs">
              <div className="text-muted-foreground">No workspace data returned</div>
            </div>
          )}
        </div>
      )}
    </div>
  );
};

const ForecastDataRenderer = ({ schema }) => {
  const { input, output } = schema;

  const hasError = output && hasKey(output, 'error');

  return (
    <div className="space-y-2">
      {input && (
        <div>
          <span className="font-medium text-foreground text-xs">Forecast Parameters:</span>
          <div className="mt-1 bg-muted p-2 rounded border border-border text-xs space-y-1">
            {input.datasource && <div><span className="text-muted-foreground">Datasource:</span> <span className="font-medium">{input.datasource}</span></div>}
            {input.query && (
              <div>
                <span className="text-muted-foreground">Query:</span>
                <div className="mt-1 bg-card p-1 rounded border border-border">
                  <MarkdownRenderer className="text-xs" compact={true}>
                    {`\`\`\`sql\n${input.query}\n\`\`\``}
                  </MarkdownRenderer>
                </div>
              </div>
            )}
            {input.timestamp && <div><span className="text-muted-foreground">Timestamp column:</span> <span>{input.timestamp}</span></div>}
            {input.value && <div><span className="text-muted-foreground">Value column:</span> <span>{input.value}</span></div>}
            {input.model && <div><span className="text-muted-foreground">Model:</span> <span>{input.model}</span></div>}
            {input.horizon && <div><span className="text-muted-foreground">Horizon:</span> <span>{input.horizon} periods</span></div>}
            {input.confidence_level && <div><span className="text-muted-foreground">Confidence:</span> <span>{Math.round(input.confidence_level * 100)}%</span></div>}
            {input.group_by && input.group_by.length > 0 && <div><span className="text-muted-foreground">Group by:</span> <span>{input.group_by.join(', ')}</span></div>}
          </div>
        </div>
      )}
      {output && (
        <div>
          {hasError ? (
            <div className="bg-error/10 p-2 rounded border border-error text-xs">
              <div className="font-medium text-error-foreground">Forecast Failed</div>
              <div className="mt-1 text-error-foreground">{output.error}</div>
            </div>
          ) : output.groups ? (
            <div className="bg-success/10 p-2 rounded border border-success text-xs space-y-2">
              <div className="font-medium text-success-foreground">Grouped Forecast Complete</div>
              {output.summary && <div className="text-muted-foreground">{output.summary}</div>}
              {!output.summary && <div className="text-muted-foreground">{output.total_groups || Object.keys(output.groups).length} group(s) forecasted</div>}
              {Object.entries(output.groups).slice(0, 5).map(([groupKey, groupResult], idx) => (
                <div key={idx} className="bg-card p-2 rounded border border-border">
                  <div className="font-medium text-foreground">{groupKey}</div>
                  {groupResult.error ? (
                    <div className="text-error text-xs mt-1">{groupResult.error}</div>
                  ) : (
                    <div className="text-muted-foreground text-xs mt-1">
                      Model: {groupResult.model_used} | {groupResult.data_points} data points | {groupResult.forecast?.length || 0} forecasted
                    </div>
                  )}
                </div>
              ))}
              {Object.keys(output.groups).length > 5 && (
                <div className="text-xs text-muted-foreground text-center">
                  ...and {Object.keys(output.groups).length - 5} more groups
                </div>
              )}
            </div>
          ) : (
            <div className="bg-success/10 p-2 rounded border border-success text-xs space-y-2">
              <div className="font-medium text-success-foreground">Forecast Complete</div>
              {output.summary && <div className="text-muted-foreground">{output.summary}</div>}
              <div className="grid grid-cols-2 gap-2">
                {output.model_used && (
                  <div className="bg-card p-2 rounded border border-border">
                    <div className="text-muted-foreground text-xs">Model</div>
                    <div className="font-medium text-foreground">{output.model_used}</div>
                  </div>
                )}
                {output.data_points && (
                  <div className="bg-card p-2 rounded border border-border">
                    <div className="text-muted-foreground text-xs">Data Points</div>
                    <div className="font-medium text-foreground">{output.data_points}</div>
                  </div>
                )}
              </div>
              {output.forecast && output.forecast.length > 0 && (
                <div>
                  <div className="text-muted-foreground text-xs mb-1">Predictions:</div>
                  <div className="bg-card border border-border rounded text-xs overflow-hidden">
                    <table className="w-full">
                      <thead className="bg-accent">
                        <tr>
                          <th className="px-2 py-1 text-left font-medium text-foreground">Period</th>
                          <th className="px-2 py-1 text-right font-medium text-foreground">Forecast</th>
                          <th className="px-2 py-1 text-right font-medium text-foreground">Lower</th>
                          <th className="px-2 py-1 text-right font-medium text-foreground">Upper</th>
                        </tr>
                      </thead>
                      <tbody>
                        {output.forecast.slice(0, 10).map((point, idx) => (
                          <tr key={idx} className={idx % 2 === 0 ? 'bg-card' : 'bg-muted'}>
                            <td className="px-2 py-1 text-muted-foreground">{point.timestamp || `Step ${point.step}`}</td>
                            <td className="px-2 py-1 text-right text-foreground font-medium">
                              {typeof point.forecast === 'number' ? point.forecast.toLocaleString(undefined, {maximumFractionDigits: 2}) : point.forecast}
                            </td>
                            <td className="px-2 py-1 text-right text-muted-foreground">
                              {typeof point.lower_bound === 'number' ? point.lower_bound.toLocaleString(undefined, {maximumFractionDigits: 2}) : point.lower_bound}
                            </td>
                            <td className="px-2 py-1 text-right text-muted-foreground">
                              {typeof point.upper_bound === 'number' ? point.upper_bound.toLocaleString(undefined, {maximumFractionDigits: 2}) : point.upper_bound}
                            </td>
                          </tr>
                        ))}
                      </tbody>
                    </table>
                    {output.forecast.length > 10 && (
                      <div className="text-xs text-muted-foreground text-center py-1">
                        ...and {output.forecast.length - 10} more periods
                      </div>
                    )}
                  </div>
                </div>
              )}
            </div>
          )}
        </div>
      )}
    </div>
  );
};

const BrowseResourcesRenderer = ({ schema }) => {
  return (
    <div className="flex items-center gap-2 text-sm text-muted-foreground">
      <span>Browsing available documentation resources</span>
    </div>
  );
};

const ReadResourceRenderer = ({ schema }) => {
  const uri = schema.input?.uri;
  return (
    <div className="flex items-center gap-2 text-sm text-muted-foreground">
      <span>Reading documentation{uri ? `: ${uri}` : ''}</span>
    </div>
  );
};

const TOOL_LABELS = {
  list_knowledge_files: 'Browsing knowledge files',
  read_knowledge_file: 'Reading knowledge file',
  write_knowledge_file: 'Writing knowledge file',
  edit_knowledge_file: 'Editing knowledge file',
  browse_catalog: 'Browsing catalog',
};

const GenericToolRenderer = ({ schema }) => {
  const label = TOOL_LABELS[schema.tool] || schema.tool;
  const path = schema.input?.path || schema.input?.datasource;
  return (
    <div className="flex items-center gap-2 text-sm text-muted-foreground">
      <span>{label}{path ? `: ${path}` : ''}</span>
    </div>
  );
};

export const ToolSchemaRenderer = ({ schema }) => {
  if (!schema || !schema.tool) {
    return null;
  }

  switch (schema.tool) {
    case 'estimate_query_cost':
    case 'bigquery_cost_estimate':
      return <BigQueryCostEstimateRenderer schema={schema} />;

    case 'query_datasource':
    case 'bigquery_query':
      return <BigQueryQueryRenderer schema={schema} />;

    case 'get_table_info':
    case 'bigquery_table_info':
      return <BigQueryTableInfoRenderer schema={schema} />;

    case 'sample_table':
    case 'bigquery_sample':
      return <BigQuerySampleRenderer schema={schema} />;

    case 'bigquery_search':
      return <BigQuerySearchRenderer schema={schema} />;

    case 'validate_chartml':
      return <ValidateChartMLRenderer schema={schema} />;

    case 'save_learning':
      return <SaveLearningRenderer schema={schema} />;

    case 'search_knowledge':
      return <SearchKnowledgeRenderer schema={schema} />;

    case 'search_catalog':
      return <BigQuerySearchRenderer schema={schema} />;

    case 'validate_sql':
      return <ValidateSQLRenderer schema={schema} />;

    case 'update_dashboard':
      return <UpdateDashboardRenderer schema={schema} />;

    case 'get_chartml_spec':
      return <GetChartMLSpecRenderer schema={schema} />;

    case 'update_chart':
      return <UpdateChartRenderer schema={schema} />;

    case 'list_datasources':
      return <ListDatasourcesRenderer schema={schema} />;

    case 'create_watch':
      return <CreateWatchRenderer schema={schema} />;

    case 'preview_watch':
      return <PreviewWatchRenderer schema={schema} />;

    case 'update_watch':
      return <UpdateWatchRenderer schema={schema} />;

    case 'search_watches':
      return <SearchWatchesRenderer schema={schema} />;

    case 'delete_watch':
      return <DeleteWatchRenderer schema={schema} />;

    case 'trigger_watch':
      return <TriggerWatchRenderer schema={schema} />;

    case 'get_watch_info':
      return <WatchInfoRenderer schema={schema} />;

    case 'search_dashboards':
      return <SearchDashboardsRenderer schema={schema} />;

    case 'get_dashboard_info':
      return <GetDashboardInfoRenderer schema={schema} />;

    case 'create_dashboard':
      return <CreateDashboardRenderer schema={schema} />;

    case 'modify_dashboard':
      return <ModifyDashboardRenderer schema={schema} />;

    case 'delete_dashboard':
      return <DeleteDashboardRenderer schema={schema} />;

    case 'get_workspace_info':
      return <GetWorkspaceInfoRenderer schema={schema} />;

    case 'forecast_data':
      return <ForecastDataRenderer schema={schema} />;

    case 'browse_resources':
      return <BrowseResourcesRenderer schema={schema} />;

    case 'read_resource':
      return <ReadResourceRenderer schema={schema} />;

    case 'list_knowledge_files':
    case 'read_knowledge_file':
    case 'write_knowledge_file':
    case 'edit_knowledge_file':
    case 'browse_catalog':
      return <GenericToolRenderer schema={schema} />;

    default:
      // Unknown tool - throw error so we know to add it
      throw new Error(`Unknown tool type '${schema.tool}' - needs to be added to ToolSchemaRenderer`);
  }
};