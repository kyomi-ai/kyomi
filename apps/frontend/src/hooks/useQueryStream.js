// SPDX-License-Identifier: AGPL-3.0-or-later
/**
 * useQueryStream — React hook for streaming query results via WebSocket.
 *
 * Subscribes to query_stream_* WebSocket messages for a given requestId,
 * accumulating rows progressively as chunks arrive.
 *
 * Usage:
 *   const { columns, rows, totalRows, status, error, executionTimeMs, startStream } = useQueryStream();
 *
 *   // Start streaming
 *   const requestId = await startStream(sql, datasource);
 *
 *   // Component renders progressively as rows arrive
 */

import { useCallback, useEffect, useRef, useState } from 'react';
import { useWebSocket } from '../context/WebSocketContext';
import apiClient from '../api/apiClient';

/**
 * @typedef {'idle' | 'streaming' | 'complete' | 'error'} StreamStatus
 */

export function useQueryStream() {
  const { subscribe } = useWebSocket();

  const [columns, setColumns] = useState([]);
  const [rows, setRows] = useState([]);
  const [totalRows, setTotalRows] = useState(null);
  const [status, setStatus] = useState('idle');
  const [error, setError] = useState(null);
  const [executionTimeMs, setExecutionTimeMs] = useState(null);
  const [bytesProcessed, setBytesProcessed] = useState(null);

  // Track the active request ID so we can filter messages
  const requestIdRef = useRef(null);
  // Track accumulated rows for chunk ordering
  const rowsRef = useRef([]);
  // Track unsubscribe functions for cleanup
  const unsubscribesRef = useRef([]);

  // Clean up subscriptions
  const cleanup = useCallback(() => {
    unsubscribesRef.current.forEach(unsub => unsub());
    unsubscribesRef.current = [];
  }, []);

  // Clean up on unmount
  useEffect(() => cleanup, [cleanup]);

  /**
   * Start a streaming query. Returns the request ID.
   *
   * @param {string} sql - SQL query to execute
   * @param {Object} datasource - { slug, type }
   * @param {Object} [options] - { limit, offset, includeTotal }
   * @returns {Promise<string>} requestId
   */
  const startStream = useCallback(async (sql, datasource, options = {}) => {
    // Clean up any previous stream
    cleanup();
    requestIdRef.current = null;
    rowsRef.current = [];
    setColumns([]);
    setRows([]);
    setTotalRows(null);
    setStatus('streaming');
    setError(null);
    setExecutionTimeMs(null);
    setBytesProcessed(null);

    // Subscribe to WebSocket messages BEFORE making the HTTP request
    // to avoid missing early messages
    const filterByRequestId = (msg, handler) => {
      const rid = msg.data?.request_id;
      if (rid && rid === requestIdRef.current) {
        handler(msg);
      }
    };

    const unsubs = [
      subscribe('query_stream_header', (msg) => filterByRequestId(msg, (m) => {
        const cols = m.data.columns || [];
        setColumns(cols);
        if (m.data.total_rows != null) {
          setTotalRows(m.data.total_rows);
        }
      })),

      subscribe('query_stream_chunk', (msg) => filterByRequestId(msg, (m) => {
        const chunkRows = m.data.rows || [];
        rowsRef.current = [...rowsRef.current, ...chunkRows];
        setRows([...rowsRef.current]);
      })),

      subscribe('query_stream_complete', (msg) => filterByRequestId(msg, (m) => {
        setStatus('complete');
        if (m.data.execution_time_ms != null) {
          setExecutionTimeMs(m.data.execution_time_ms);
        }
        if (m.data.bytes_processed != null) {
          setBytesProcessed(m.data.bytes_processed);
        }
        if (m.data.total_rows_returned != null) {
          setTotalRows(m.data.total_rows_returned);
        }
        // Clean up subscriptions after completion
        cleanup();
      })),

      subscribe('query_stream_error', (msg) => filterByRequestId(msg, (m) => {
        setStatus('error');
        setError(m.data.error || 'Stream error');
        cleanup();
      })),
    ];
    unsubscribesRef.current = unsubs;

    // Make the HTTP request to start streaming
    const response = await apiClient.post('/api/v1/datasources/query/stream', {
      sql,
      datasource: datasource.slug,
      limit: options.limit || 10000,
      offset: options.offset || 0,
      include_total: options.includeTotal !== false,
    });

    const { request_id } = response.data;
    requestIdRef.current = request_id;

    return request_id;
  }, [subscribe, cleanup]);

  return {
    columns,
    rows,
    totalRows,
    status,
    error,
    executionTimeMs,
    bytesProcessed,
    startStream,
  };
}
