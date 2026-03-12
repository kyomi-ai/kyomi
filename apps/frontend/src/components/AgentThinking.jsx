// SPDX-License-Identifier: AGPL-3.0-or-later
import React, { useState, useEffect, useRef } from 'react';
import { Card } from './ui/card';
import { Button } from './ui/button';
import { ToolSchemaRenderer } from './ToolSchemaRenderer';
import { useCapabilities } from '../context/CapabilitiesContext';

const AgentThinking = ({ thinkingEvents = [], isActive = false, variant = 'inset', tokenUsage = null }) => {
  const { capabilities } = useCapabilities();
  // variant options: 'inset', 'header-bar', 'default'
  const [isExpanded, setIsExpanded] = useState(false); // Always start collapsed
  const [elapsedTime, setElapsedTime] = useState(0);
  const scrollContainerRef = useRef(null);
  const thinkingEndRef = useRef(null);
  const startTimeRef = useRef(null);

  // Live timer - counts up while processing
  useEffect(() => {
    if (isActive) {
      if (!startTimeRef.current) {
        startTimeRef.current = Date.now();
      }
      const interval = setInterval(() => {
        setElapsedTime(Date.now() - startTimeRef.current);
      }, 100); // Update every 100ms
      return () => clearInterval(interval);
    } else {
      startTimeRef.current = null;
    }
  }, [isActive]);

  // Scroll to bottom when NEW events arrive (but not when expanding/collapsing)
  useEffect(() => {
    if (isExpanded && thinkingEvents.length > 0) {
      thinkingEndRef.current?.scrollIntoView({ behavior: "smooth" });
    }
  }, [thinkingEvents]); // Only depend on thinkingEvents, not isExpanded

  // Always show the bubble (even when empty), but collapsed
  // This gives a consistent UI presence

  const getEventIcon = (eventType) => {
    switch (eventType) {
      case 'agent_start':
        return '🤖';
      case 'agent_thought':
        return '💭';
      case 'tool_execution_start':
        return '🔧';
      case 'tool_execution_end':
        return '✅';
      case 'agent_decision':
        return '🎯';
      case 'agent_complete':
        return '🎉';
      case 'error':
        return '⚠️';
      default:
        return '📝';
    }
  };

  const formatDuration = (durationMs) => {
    if (!durationMs) return '';
    if (durationMs < 1000) return `${durationMs}ms`;
    return `${(durationMs / 1000).toFixed(1)}s`;
  };

  const formatTimestamp = (timestamp) => {
    const date = new Date(timestamp);
    return date.toLocaleTimeString('en-US', {
      hour12: false,
      hour: '2-digit',
      minute: '2-digit',
      second: '2-digit',
      fractionalSecondDigits: 1
    });
  };

  const latestEvent = thinkingEvents.length > 0 ? thinkingEvents[thinkingEvents.length - 1] : null;
  const toolExecutions = thinkingEvents.filter(e =>
    e.event_type === 'tool_execution_start' ||
    e.event_type === 'tool_execution_end' ||
    e.event_type === 'tool_start' ||
    e.event_type === 'tool_end'
  );
  const totalDuration = latestEvent?.duration_ms;

  // Get the current status title for the header
  // Only show while actively processing - the response itself indicates completion
  // Strip emojis from the title - they clash with the sparkly logo
  const stripEmojis = (text) => {
    if (!text) return null;
    return text.replace(/[\u{1F300}-\u{1F9FF}]|[\u{2600}-\u{26FF}]|[\u{2700}-\u{27BF}]|[\u{1F600}-\u{1F64F}]|[\u{1F680}-\u{1F6FF}]|[\u{1F1E0}-\u{1F1FF}]|[\u{2300}-\u{23FF}]|[\u{2B50}]|[\u{231A}-\u{231B}]|[\u{23E9}-\u{23F3}]|[\u{23F8}-\u{23FA}]|[\u{25AA}-\u{25AB}]|[\u{25B6}]|[\u{25C0}]|[\u{25FB}-\u{25FE}]|[\u{2614}-\u{2615}]|[\u{2648}-\u{2653}]|[\u{267F}]|[\u{2693}]|[\u{26A1}]|[\u{26AA}-\u{26AB}]|[\u{26BD}-\u{26BE}]|[\u{26C4}-\u{26C5}]|[\u{26CE}]|[\u{26D4}]|[\u{26EA}]|[\u{26F2}-\u{26F3}]|[\u{26F5}]|[\u{26FA}]|[\u{26FD}]|[\u{2702}]|[\u{2705}]|[\u{2708}-\u{270D}]|[\u{270F}]|[\u{2712}]|[\u{2714}]|[\u{2716}]|[\u{271D}]|[\u{2721}]|[\u{2728}]|[\u{2733}-\u{2734}]|[\u{2744}]|[\u{2747}]|[\u{274C}]|[\u{274E}]|[\u{2753}-\u{2755}]|[\u{2757}]|[\u{2763}-\u{2764}]|[\u{2795}-\u{2797}]|[\u{27A1}]|[\u{27B0}]|[\u{27BF}]|[\u{2934}-\u{2935}]|[\u{2B05}-\u{2B07}]|[\u{2B1B}-\u{2B1C}]|[\u{2B55}]|[\u{3030}]|[\u{303D}]|[\u{3297}]|[\u{3299}]|[\u{23CF}]|[\u{23ED}-\u{23EF}]|[\u{23F1}-\u{23F2}]|[\u{200D}]|⏳|✅|❌|⚠️/gu, '').trim();
  };
  const currentTitle = isActive && latestEvent ? stripEmojis(latestEvent.title) : null;

  // Different visual styles - make them VERY different
  const renderThinking = () => {
    if (variant === 'inset') {
      // Deep inset - looks like it's recessed behind the message
      return (
        <div className="mb-4 -mx-6 -mt-2 bg-accent border-l-4 border-primary shadow-inner" data-testid="agent-thinking">
          <div className="p-4 pl-6">
            {renderContent()}
          </div>
        </div>
      );
    }

    if (variant === 'header-bar') {
      // Slim bar at top that expands downward
      return (
        <div className="mb-3 -mx-6 -mt-4 bg-muted border-b border-border overflow-hidden transition-all duration-300 ease-in-out" data-testid="agent-thinking">
          <div className="py-1 px-4">
            {renderContent()}
          </div>
        </div>
      );
    }

    if (variant === 'tab') {
      // Tab sticking out from top-left
      return (
        <div className="mb-3 relative" data-testid="agent-thinking">
          <div className="absolute -top-3 left-0 bg-primary text-white px-3 py-1 rounded-t-lg text-xs font-medium shadow">
            🧠 Thinking
          </div>
          <Card className="mt-2 bg-muted border-border pt-6">
            <div className="p-3">
              {renderContent()}
            </div>
          </Card>
        </div>
      );
    }

    // default - original floating card
    return (
      <Card className="mt-2 mb-3 bg-muted border-border" data-testid="agent-thinking">
        <div className="p-2">
          {renderContent()}
        </div>
      </Card>
    );
  };

  const renderContent = () => (
    <>
        {/* Header - Always visible */}
        <div
          className="flex items-center justify-between cursor-pointer py-1"
          onClick={() => setIsExpanded(!isExpanded)}
        >
          <div className="flex items-center space-x-2 min-w-0 flex-1">
            {isActive ? (
              <img src="/kyomi_animated_logo.svg" alt="Processing" className="w-4 h-4 flex-shrink-0" />
            ) : (
              <img src="/kyomi_small_logo.svg" alt="Thinking" className="w-4 h-4 flex-shrink-0" />
            )}
            {currentTitle && (
              <span className="text-xs text-muted-foreground truncate animate-subtle-breathe">{currentTitle}</span>
            )}
          </div>
          <div className="flex items-center space-x-2 flex-shrink-0">
            <div className="text-xs text-muted-foreground font-mono whitespace-nowrap">
              {isActive ? (
                <>
                  <span>{toolExecutions.length} tools • {formatDuration(elapsedTime)}</span>
                </>
              ) : (
                <>
                  <span>{toolExecutions.length} tools • {formatDuration(totalDuration)}</span>
                </>
              )}
            </div>
            <Button variant="ghost" size="sm" className="h-6 w-6 text-muted-foreground hover:text-foreground p-0">
              <svg
                className={`w-3 h-3 transform transition-transform ${isExpanded ? 'rotate-180' : ''}`}
                fill="none"
                stroke="currentColor"
                viewBox="0 0 24 24"
              >
                <path strokeLinecap="round" strokeLinejoin="round" strokeWidth={2} d="M19 9l-7 7-7-7" />
              </svg>
            </Button>
          </div>
        </div>

        {/* Expanded Content */}
        <div
          ref={scrollContainerRef}
          className="space-y-2 overflow-y-auto transition-all duration-300 ease-in-out"
          style={{
            maxHeight: isExpanded ? '24rem' : '0',
            marginTop: isExpanded ? '0.75rem' : '0',
            opacity: isExpanded ? 1 : 0,
            scrollBehavior: 'smooth'
          }}
        >
            {thinkingEvents.map((event, index) => (
              <div key={index} className="flex items-start space-x-3 py-2 px-3 bg-card rounded-lg border border-border">
                <div className="flex-shrink-0 mt-0.5">
                  <span className="text-sm">{getEventIcon(event.event_type)}</span>
                </div>
                <div className="flex-1 min-w-0">
                  <div className="flex items-center justify-between">
                    <h4 className="text-sm font-medium text-foreground">
                      {event.title}
                    </h4>
                    <div className="flex items-center space-x-2 text-xs text-muted-foreground">
                      {event.duration_ms && (
                        <span>{formatDuration(event.duration_ms)}</span>
                      )}
                      <span>{formatTimestamp(event.timestamp)}</span>
                    </div>
                  </div>
                  {event.description && (
                    <p className="text-sm text-muted-foreground mt-1">
                      {event.description}
                    </p>
                  )}
                  {event.data?.schema && (
                    <div className="mt-2">
                      <ToolSchemaRenderer schema={event.data.schema} />
                    </div>
                  )}
                </div>
              </div>
            ))}
            <div ref={thinkingEndRef} />
        </div>
    </>
  );

  return renderThinking();
};

// Memoize to prevent re-renders when props haven't changed
export default React.memo(AgentThinking);