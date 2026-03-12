// SPDX-License-Identifier: AGPL-3.0-or-later
import {
  startRegistration,
  startAuthentication,
  browserSupportsWebAuthn,
} from '@simplewebauthn/browser';
import { API_CONFIG } from '../config/api.js';

class PasskeyManager {
  /**
   * Check if the browser supports WebAuthn
   */
  static isSupported() {
    try {
      return typeof window !== 'undefined' && browserSupportsWebAuthn();
    } catch (error) {
      return false;
    }
  }

  /**
   * Register a new passkey for a user
   */
  static async register(email, name, deviceName = null) {
    if (!this.isSupported()) {
      throw new Error('Your browser does not support passkeys. Please use a modern browser.');
    }

    try {
      // Step 1: Get registration challenge from server
      const challengeResponse = await fetch(`${API_CONFIG.baseURL}${API_CONFIG.endpoints.passkeys.registerStart}`, {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ email, name, device_name: deviceName }),
      });

      if (!challengeResponse.ok) {
        const error = await challengeResponse.json();
        throw new Error(error.detail || 'Failed to start passkey registration');
      }

      const challengeData = await challengeResponse.json();

      // smtp-less self-hosted: server issues a token directly instead of a WebAuthn challenge
      if (challengeData.status === 'token_issued') {
        const err = new Error(challengeData.message || 'Token issued for passkey signup');
        err.tokenIssued = true;
        err.token = challengeData.token;
        throw err;
      }

      // Step 2: Use WebAuthn to create credential
      const { challenge_id, options } = challengeData;
      const registrationResponse = await startRegistration({
        optionsJSON: options.publicKey || options
      });

      // Step 3: Complete registration on server
      const completionResponse = await fetch(`${API_CONFIG.baseURL}${API_CONFIG.endpoints.passkeys.registerComplete}`, {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({
          challenge_id: challenge_id,
          credential: registrationResponse,
          device_name: deviceName || this._getDeviceName(),
        }),
      });

      if (!completionResponse.ok) {
        const error = await completionResponse.json();
        throw new Error(error.detail || 'Failed to complete passkey registration');
      }

      return await completionResponse.json();
    } catch (error) {
      
      // Handle specific WebAuthn errors with user-friendly messages
      if (error.name === 'InvalidStateError') {
        throw new Error('A passkey already exists for this device. Please try with a different device or remove the existing passkey first.');
      } else if (error.name === 'NotAllowedError') {
        throw new Error('Passkey creation was cancelled or timed out. Please try again.');
      } else if (error.name === 'AbortError') {
        throw new Error('Passkey creation was cancelled. Please try again.');
      } else if (error.name === 'NotSupportedError') {
        throw new Error('Your device does not support this type of passkey. Please try a different authentication method.');
      }
      
      throw error;
    }
  }

  /**
   * Authenticate with an existing passkey
   */
  static async authenticate(email = null) {
    if (!this.isSupported()) {
      throw new Error('Your browser does not support passkeys. Please use password login instead.');
    }

    try {
      // Step 1: Get authentication challenge from server
      const challengeResponse = await fetch(`${API_CONFIG.baseURL}${API_CONFIG.endpoints.passkeys.loginStart}`, {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ email }),
      });

      if (!challengeResponse.ok) {
        const error = await challengeResponse.json();
        throw new Error(error.detail || 'Failed to start passkey authentication');
      }

      const challengeData = await challengeResponse.json();

      // Step 2: Use WebAuthn to authenticate
      // Extract challenge_id and prepare WebAuthn options
      const { challenge_id, options } = challengeData;

      // Debug: log the options to see what rpId is being used

      let authenticationResponse;
      try {
        authenticationResponse = await startAuthentication({
          optionsJSON: options.publicKey || options
        });
      } catch (webauthnError) {
        throw webauthnError;
      }

      // Step 3: Complete authentication on server
      const completionResponse = await fetch(`${API_CONFIG.baseURL}${API_CONFIG.endpoints.passkeys.loginComplete}`, {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({
          challenge_id: challenge_id,
          credential: authenticationResponse,
        }),
      });

      if (!completionResponse.ok) {
        // Check for 403 with verification required header
        if (completionResponse.status === 403 && completionResponse.headers.get('x-verification-required')) {
          const errorData = await completionResponse.json().catch(() => ({}));
          const error = new Error(errorData.detail || 'Please verify your email before signing in.');
          error.verificationRequired = true;
          throw error;
        }

        const error = await completionResponse.json();
        throw new Error(error.detail || 'Failed to complete passkey authentication');
      }

      return await completionResponse.json();
    } catch (error) {

      // Propagate verification required error
      if (error.verificationRequired) {
        throw error;
      }

      // Handle specific WebAuthn errors with user-friendly messages
      if (error.name === 'NotAllowedError') {
        throw new Error('Passkey authentication was cancelled or timed out. Please try again.');
      } else if (error.name === 'AbortError') {
        throw new Error('Passkey authentication was cancelled. Please try again.');
      } else if (error.name === 'InvalidStateError') {
        throw new Error('No passkey found for this account. Please register a passkey first.');
      }

      throw error;
    }
  }

  /**
   * List user's passkeys (requires authentication)
   */
  static async listPasskeys() {
    try {

      const response = await fetch(`${API_CONFIG.baseURL}${API_CONFIG.endpoints.passkeys.list}`, {
        headers: {
          'Content-Type': 'application/json',
        },
        credentials: 'include', // Include HTTPOnly cookies
      });

      if (!response.ok) {
        const error = await response.json();
        throw new Error(error.detail || 'Failed to list passkeys');
      }

      return await response.json();
    } catch (error) {
      throw error;
    }
  }

  /**
   * Add a passkey for authenticated user
   */
  static async addPasskey(deviceName) {
    if (!(await this.isAvailable())) {
      throw new Error('Passkeys are not supported on this device/browser');
    }

    try {
      deviceName = deviceName || this._getDeviceName();

      // Step 1: Get registration challenge from server (authenticated endpoint)
      const challengeResponse = await fetch(`${API_CONFIG.baseURL}${API_CONFIG.endpoints.passkeys.addStart}`, {
        method: 'POST',
        headers: { 
          'Content-Type': 'application/json',
        },
        credentials: 'include', // Include HTTPOnly cookies
        body: JSON.stringify({ device_name: deviceName }),
      });

      if (!challengeResponse.ok) {
        const error = await challengeResponse.json();
        throw new Error(error.detail || 'Failed to start passkey registration');
      }

      const challengeData = await challengeResponse.json();

      // Step 2: Use WebAuthn to create credential
      const { challenge_id, options } = challengeData;
      const registrationResponse = await startRegistration({
        optionsJSON: options.publicKey || options
      });

      // Step 3: Send credential back to server for verification
      const completionResponse = await fetch(`${API_CONFIG.baseURL}${API_CONFIG.endpoints.passkeys.addComplete}`, {
        method: 'POST',
        headers: { 
          'Content-Type': 'application/json',
        },
        credentials: 'include', // Include HTTPOnly cookies
        body: JSON.stringify({
          challenge_id,
          credential: registrationResponse
        })
      });

      if (!completionResponse.ok) {
        const error = await completionResponse.json();
        throw new Error(error.detail || 'Failed to complete passkey registration');
      }

      return await completionResponse.json();

    } catch (error) {
      throw error;
    }
  }

  /**
   * Delete a passkey (requires authentication)
   */
  static async deletePasskey(credentialId) {
    try {
      const response = await fetch(`${API_CONFIG.baseURL}${API_CONFIG.endpoints.passkeys.delete}/${encodeURIComponent(credentialId)}`, {
        method: 'DELETE',
        headers: {
          'Content-Type': 'application/json',
        },
        credentials: 'include', // Include HTTPOnly cookies
      });

      if (!response.ok) {
        const error = await response.json();
        throw new Error(error.detail || 'Failed to delete passkey');
      }

      return await response.json();
    } catch (error) {
      throw error;
    }
  }

  /**
   * Rename a passkey (requires authentication)
   */
  static async renamePasskey(credentialId, newDeviceName) {
    try {
      const response = await fetch(`${API_CONFIG.baseURL}${API_CONFIG.endpoints.passkeys.update}/${encodeURIComponent(credentialId)}`, {
        method: 'PATCH',
        headers: {
          'Content-Type': 'application/json',
        },
        credentials: 'include', // Include HTTPOnly cookies
        body: JSON.stringify({ device_name: newDeviceName }),
      });

      if (!response.ok) {
        const error = await response.json();
        throw new Error(error.detail || 'Failed to rename passkey');
      }

      return await response.json();
    } catch (error) {
      throw error;
    }
  }

  /**
   * Get a user-friendly device name
   */
  static _getDeviceName() {
    const ua = navigator.userAgent;
    const platform = navigator.platform || 'Unknown';
    
    // Simple device detection
    if (/iPhone|iPad|iPod/.test(ua)) {
      return `iPhone/iPad (${this._getBrowser()})`;
    } else if (/Android/.test(ua)) {
      return `Android (${this._getBrowser()})`;
    } else if (/Mac/.test(platform)) {
      return `Mac (${this._getBrowser()})`;
    } else if (/Win/.test(platform)) {
      return `Windows (${this._getBrowser()})`;
    } else if (/Linux/.test(platform)) {
      return `Linux (${this._getBrowser()})`;
    }
    
    return `${platform} (${this._getBrowser()})`;
  }

  /**
   * Get browser name
   */
  static _getBrowser() {
    const ua = navigator.userAgent;
    
    if (/Chrome/.test(ua) && !/Edge/.test(ua)) return 'Chrome';
    if (/Firefox/.test(ua)) return 'Firefox';
    if (/Safari/.test(ua) && !/Chrome/.test(ua)) return 'Safari';
    if (/Edge/.test(ua)) return 'Edge';
    
    return 'Browser';
  }

  /**
   * Check if passkeys are available on this device
   */
  static async isAvailable() {
    if (!this.isSupported()) {
      return false;
    }

    // For development/testing: be permissive to allow external authenticators
    // In production, you might want to check platform authenticator availability
    try {
      if (typeof window !== 'undefined' && window.PublicKeyCredential) {
        await PublicKeyCredential.isUserVerifyingPlatformAuthenticatorAvailable();

        // Allow passkeys if either platform authenticator OR external authenticators might work
        // This enables testing with USB security keys, etc.
        return true; // Always allow for broader compatibility
      }
      return false;
    } catch (error) {
      // Fallback: assume available to allow external authenticators
      return true;
    }
  }
}

export default PasskeyManager;