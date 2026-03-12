// SPDX-License-Identifier: AGPL-3.0-or-later
/**
 * Trial API Client
 *
 * Simple API client for the trial chat endpoint.
 * Does NOT use the main apiClient since trial users are not authenticated.
 */

import axios from 'axios';
import { API_CONFIG } from '../config/api.js';

const trialClient = axios.create({
  baseURL: API_CONFIG.baseURL,
  headers: {
    'Content-Type': 'application/json',
  },
});

/**
 * Get session token from localStorage or return null
 */
export function getSessionToken() {
  return localStorage.getItem('trial_session_token');
}

/**
 * Save session token to localStorage
 */
export function saveSessionToken(token) {
  localStorage.setItem('trial_session_token', token);
}

/**
 * Get trial access token from localStorage (for chart queries)
 */
export function getTrialAccessToken() {
  return localStorage.getItem('trial_access_token');
}

/**
 * Save trial access token to localStorage
 */
export function saveTrialAccessToken(token) {
  localStorage.setItem('trial_access_token', token);
}

/**
 * Clear session token from localStorage
 */
export function clearSessionToken() {
  localStorage.removeItem('trial_session_token');
  localStorage.removeItem('trial_access_token');
}

/**
 * Create or retrieve a trial session.
 * Limited to 1 session per IP per day.
 *
 * @returns {Promise<Object>} Response with session_token, trial_access_token, expires_at, queries_remaining
 */
export async function createTrialSession() {
  try {
    const response = await trialClient.post('/api/v1/trial/session');

    // Save tokens
    if (response.data.session_token) {
      saveSessionToken(response.data.session_token);
    }
    if (response.data.trial_access_token) {
      saveTrialAccessToken(response.data.trial_access_token);
    }

    return response.data;
  } catch (error) {
    if (error.response?.status === 429) {
      throw new Error(error.response.data.detail || 'Daily trial limit reached');
    }
    throw error;
  }
}

/**
 * Ensure we have a valid session, always fetching current state from backend.
 *
 * Always calls the backend to get current query count, since the session
 * endpoint is idempotent (same IP = same session within 24h).
 *
 * @returns {Promise<Object>} Session data with tokens and queries_remaining
 */
export async function ensureTrialSession() {
  // Always call backend to get current session state (idempotent - same IP = same session)
  return createTrialSession();
}

/**
 * Send a chat message to the trial endpoint.
 * Requires valid session - call ensureTrialSession() first.
 *
 * @param {string} message - User's message
 * @param {Array} conversationHistory - Previous messages [{role, content}, ...]
 * @returns {Promise<Object>} Response with response, query_count, queries_remaining, should_prompt_signup
 */
export async function sendTrialMessage(message, conversationHistory = []) {
  const sessionToken = getSessionToken();
  const accessToken = getTrialAccessToken();

  if (!sessionToken || !accessToken) {
    throw new Error('No trial session. Please refresh the page.');
  }

  const payload = {
    message,
    conversation_history: conversationHistory,
    session_token: sessionToken,
    trial_access_token: accessToken,
    current_time_user_tz: new Date().toISOString(),
  };

  try {
    const response = await trialClient.post('/api/v1/trial/chat', payload);

    // Update tokens from the response (get refreshed tokens)
    if (response.data.session_token) {
      saveSessionToken(response.data.session_token);
    }
    if (response.data.trial_access_token) {
      saveTrialAccessToken(response.data.trial_access_token);
    }

    return response.data;
  } catch (error) {
    if (error.response?.status === 401) {
      // Token expired or invalid - clear and prompt refresh
      clearSessionToken();
      throw new Error(error.response.data.detail || 'Session expired. Please refresh the page.');
    }
    if (error.response?.status === 429) {
      // Rate limit exceeded
      throw new Error(error.response.data.detail || 'Query limit reached');
    }
    throw error;
  }
}

/**
 * Execute a SQL query against the trial sample database.
 * Used by ChartML to render charts in trial mode.
 *
 * @param {string} sql - SQL query to execute
 * @param {number} limit - Max rows to return (default 10000)
 * @returns {Promise<Object>} Response with columns and rows
 */
export async function executeTrialQuery(sql, limit = 10000) {
  const accessToken = getTrialAccessToken();

  if (!accessToken) {
    throw new Error('No trial access token. Please send a message first.');
  }

  try {
    const response = await trialClient.post('/api/v1/trial/query', {
      sql,
      trial_access_token: accessToken,
      limit,
    });

    if (response.data.status === 'error') {
      throw new Error(response.data.error || 'Query failed');
    }

    return response.data;
  } catch (error) {
    if (error.response?.status === 401) {
      // Token expired or invalid - clear it
      localStorage.removeItem('trial_access_token');
      throw new Error(error.response.data.detail || 'Session expired. Please refresh the page.');
    }
    if (error.response?.status === 429) {
      throw new Error(error.response.data.detail || 'Rate limit exceeded');
    }
    throw error;
  }
}

export default {
  createTrialSession,
  ensureTrialSession,
  sendTrialMessage,
  executeTrialQuery,
  getSessionToken,
  saveSessionToken,
  getTrialAccessToken,
  saveTrialAccessToken,
  clearSessionToken,
};
