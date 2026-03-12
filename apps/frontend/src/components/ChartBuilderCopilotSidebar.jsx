// SPDX-License-Identifier: AGPL-3.0-or-later
import { useState, useRef, useCallback, useEffect } from 'react';
import * as yaml from 'js-yaml';
import { useWebSocket } from '../context/WebSocketContext';
import { ChatInterface } from './ChatInterface';
import apiClient from '../api/apiClient';

/**
 * ChartBuilderCopilotSidebar - Conversational AI panel for chart editing
 *
 * Renders as inline tab content within ChartBuilderModal's "AI" tab.
 * Sessions are ephemeral - cleaned up when the component unmounts.
 * Uses shared ChatInterface component for core chat functionality.
 *
 * @param {Object} props.chartML - Current ChartML spec object
 * @param {Function} props.onChartUpdate - Callback when AI updates the chart spec
 */
export function ChartBuilderCopilotSidebar({
  chartML,
  onChartUpdate
}) {
  const { subscribe } = useWebSocket();
  const [copilotSessionId, setCopilotSessionId] = useState(null);
  const [chartAtLastMessage, setChartAtLastMessage] = useState(null);
  const copilotSessionIdRef = useRef(null);

  // Keep ref in sync with state
  useEffect(() => {
    copilotSessionIdRef.current = copilotSessionId;
  }, [copilotSessionId]);

  // Serialize chartML for comparison (stable string representation)
  const serializeChart = useCallback((chart) => {
    if (!chart) return null;
    try {
      return yaml.dump(chart, { noRefs: true, sortKeys: true });
    } catch {
      return JSON.stringify(chart);
    }
  }, []);

  // Cleanup copilot session
  const cleanupSession = useCallback(async () => {
    const sessionId = copilotSessionIdRef.current;
    if (sessionId) {
      try {
        await apiClient.delete(`/api/v1/chat/copilot/session/${sessionId}`);
      } catch (error) {
        // Session cleanup is best-effort
      }
      setCopilotSessionId(null);
      copilotSessionIdRef.current = null;
    }
  }, []);

  // Handle session creation
  const handleSessionCreated = useCallback((sessionId) => {
    setCopilotSessionId(sessionId);
  }, []);

  // Build API payload with chart context
  const getApiPayloadExtras = useCallback(() => {
    const currentSerialized = serializeChart(chartML);
    const lastSerialized = serializeChart(chartAtLastMessage);
    const chartChanged = currentSerialized !== lastSerialized;

    return {
      context: {
        type: 'chart_builder_copilot',
        // Send YAML string for the chart content
        chartContent: chartChanged ? currentSerialized : null
      }
    };
  }, [chartML, chartAtLastMessage, serializeChart]);

  // Handle first thinking event - update chart baseline
  const handleFirstThinkingEvent = useCallback(() => {
    const currentSerialized = serializeChart(chartML);
    const lastSerialized = serializeChart(chartAtLastMessage);
    if (currentSerialized !== lastSerialized) {
      setChartAtLastMessage(chartML);
    }
  }, [chartML, chartAtLastMessage, serializeChart]);

  // Handle custom WebSocket events (chart_update)
  const handleCustomWebSocketEvent = useCallback((subscribeFunc, sessionIdRef) => {
    const unsubscribe = subscribeFunc('chart_update', (message) => {
      if (message.data?.context_type !== 'chart_builder_copilot') return;
      if (sessionIdRef.current && message.session_id !== sessionIdRef.current) return;

      const newContent = message.data?.content;
      if (newContent && onChartUpdate) {
        try {
          // Parse YAML to ChartML object
          const newChartML = yaml.load(newContent);
          onChartUpdate(newChartML);
          setChartAtLastMessage(newChartML);
        } catch (error) {
          // Ignore parse errors from malformed AI responses
        }
      }
    });
    return unsubscribe;
  }, [onChartUpdate]);

  // Cleanup on unmount
  useEffect(() => {
    return () => {
      cleanupSession();
    };
  }, [cleanupSession]);

  // Render as inline content that fills the parent container
  return (
    <div className="flex flex-col h-full min-h-0">
      <ChatInterface
        variant="sidebar"
        contextType="chart_builder_copilot"
        apiEndpoint="/api/v1/chat/copilot/message"
        apiPayloadExtras={getApiPayloadExtras()}
        sessionId={copilotSessionId}
        onSessionCreated={handleSessionCreated}
        onFirstThinkingEvent={handleFirstThinkingEvent}
        onCustomWebSocketEvent={handleCustomWebSocketEvent}
        placeholder="Ask about your chart..."
        emptyStateMessage="Ask me anything about your chart!"
        emptyStateSubtext="I can help you change chart types, adjust styling, or fix configuration issues."
      />
    </div>
  );
}

export default ChartBuilderCopilotSidebar;
