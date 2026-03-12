// SPDX-License-Identifier: AGPL-3.0-or-later
/**
 * Tests for useChatState hook
 *
 * Run with: npm test useChatState.test.js
 */

import { renderHook, act } from '@testing-library/react';
import { useChatState, CHAT_STATES } from './useChatState';

describe('useChatState', () => {
  test('should start in IDLE state', () => {
    const { result } = renderHook(() => useChatState());
    expect(result.current.state).toBe(CHAT_STATES.IDLE);
    expect(result.current.canSend).toBe(true);
    expect(result.current.showStopButton).toBe(false);
  });

  test('should transition IDLE -> SENDING -> STREAMING', () => {
    const { result } = renderHook(() => useChatState());

    // Start sending
    act(() => {
      result.current.startSending('session-123');
    });
    expect(result.current.state).toBe(CHAT_STATES.SENDING);
    expect(result.current.activeSessionId).toBe('session-123');
    expect(result.current.canSend).toBe(false);

    // Start streaming
    act(() => {
      result.current.startStreaming('message-456');
    });
    expect(result.current.state).toBe(CHAT_STATES.STREAMING);
    expect(result.current.activeMessageId).toBe('message-456');
    expect(result.current.showStopButton).toBe(true);
  });

  test('should handle successful completion', () => {
    const { result } = renderHook(() => useChatState());

    act(() => {
      result.current.startSending('session-123');
      result.current.startStreaming('message-456');
      result.current.complete();
    });

    expect(result.current.state).toBe(CHAT_STATES.IDLE);
    expect(result.current.activeMessageId).toBe(null);
    expect(result.current.canSend).toBe(true);
  });

  test('should handle cancellation flow', () => {
    const { result } = renderHook(() => useChatState());

    act(() => {
      result.current.startSending('session-123');
      result.current.startStreaming('message-456');
    });

    // Request cancel
    act(() => {
      const success = result.current.requestCancel();
      expect(success).toBe(true);
    });
    expect(result.current.state).toBe(CHAT_STATES.CANCELLING);
    expect(result.current.showStopButton).toBe(true);

    // Confirm cancelled
    act(() => {
      result.current.confirmCancelled();
    });
    expect(result.current.state).toBe(CHAT_STATES.CANCELLED);

    // Should auto-reset to IDLE after timeout
    // (We can't easily test the setTimeout, but we can verify the method exists)
  });

  test('should prevent cancel when not streaming', () => {
    const { result } = renderHook(() => useChatState());

    act(() => {
      const success = result.current.requestCancel();
      expect(success).toBe(false);
    });
    expect(result.current.state).toBe(CHAT_STATES.IDLE);
  });

  test('should handle errors', () => {
    const { result } = renderHook(() => useChatState());

    act(() => {
      result.current.startSending('session-123');
      result.current.setErrorState('Network error');
    });

    expect(result.current.state).toBe(CHAT_STATES.ERROR);
    expect(result.current.error).toBe('Network error');
    expect(result.current.hasError).toBe(true);
  });

  test('should identify active messages', () => {
    const { result } = renderHook(() => useChatState());

    act(() => {
      result.current.startSending('session-123');
      result.current.startStreaming('message-456');
    });

    expect(result.current.isActiveMessage('message-456')).toBe(true);
    expect(result.current.isActiveMessage('message-789')).toBe(false);
  });

  test('should reset state', () => {
    const { result } = renderHook(() => useChatState());

    act(() => {
      result.current.startSending('session-123');
      result.current.startStreaming('message-456');
      result.current.reset('switching sessions');
    });

    expect(result.current.state).toBe(CHAT_STATES.IDLE);
    expect(result.current.activeMessageId).toBe(null);
    expect(result.current.activeSessionId).toBe(null);
  });

  test('should log state transitions', () => {
    const { result } = renderHook(() => useChatState());

    act(() => {
      result.current.startSending('session-123');
      result.current.startStreaming('message-456');
      result.current.complete();
    });

    expect(result.current.transitionLog.length).toBeGreaterThan(0);
    expect(result.current.transitionLog[0].from).toBe(CHAT_STATES.IDLE);
    expect(result.current.transitionLog[0].to).toBe(CHAT_STATES.SENDING);
  });
});
