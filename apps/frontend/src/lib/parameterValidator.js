// SPDX-License-Identifier: AGPL-3.0-or-later
/**
 * Parameter Validation for Dashboard Editor
 *
 * Validates ChartML params blocks for duplicate parameter IDs
 * and returns Monaco editor markers (red squiggles) for errors.
 */

/**
 * Validates markdown content for duplicate parameter IDs across params blocks
 *
 * @param {string} markdown - The markdown content to validate
 * @returns {Array} Array of Monaco editor markers for errors
 */
export function validateParameterIds(markdown) {
  const markers = [];
  const seenParamIds = new Map(); // Map of paramId -> {line, column}

  // Find all chartml code blocks
  const chartmlBlockRegex = /```chartml\s*\n([\s\S]*?)\n```/g;
  let blockMatch;

  while ((blockMatch = chartmlBlockRegex.exec(markdown)) !== null) {
    const blockContent = blockMatch[1];
    const blockStartLine = markdown.substring(0, blockMatch.index).split('\n').length;

    // Check if this block starts with "type: params"
    const typeMatch = blockContent.match(/^\s*type:\s*params/m);
    if (!typeMatch) continue; // Not a params block, skip

    // Extract all parameter IDs from this params block
    // Look for "- id: <name>" patterns
    const paramIdRegex = /^(\s*)-\s*id:\s*([a-zA-Z0-9_]+)/gm;
    let paramMatch;

    while ((paramMatch = paramIdRegex.exec(blockContent)) !== null) {
      const paramId = paramMatch[2];
      const indent = paramMatch[1];

      // Calculate line and column for this parameter
      // blockStartLine is the line of "```chartml", so add 1 for first line of content
      const linesBeforeParam = blockContent.substring(0, paramMatch.index).split('\n').length;
      const lineNumber = blockStartLine + linesBeforeParam;
      const column = indent.length + 1; // Column where "- id:" starts

      // Check if we've seen this ID before
      if (seenParamIds.has(paramId)) {
        const firstOccurrence = seenParamIds.get(paramId);

        // Add marker for this duplicate (2nd+ occurrence)
        markers.push({
          severity: 8, // monaco.MarkerSeverity.Error
          startLineNumber: lineNumber,
          startColumn: column,
          endLineNumber: lineNumber,
          endColumn: column + paramMatch[0].length,
          message: `Duplicate parameter ID "${paramId}" - first defined at line ${firstOccurrence.line}`
        });
      } else {
        // Track first occurrence
        seenParamIds.set(paramId, {
          line: lineNumber,
          column: column
        });
      }
    }
  }

  return markers;
}
