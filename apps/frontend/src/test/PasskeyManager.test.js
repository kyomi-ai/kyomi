// SPDX-License-Identifier: AGPL-3.0-or-later
/**
 * @vitest-environment jsdom
 */

import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest'
import PasskeyManager from '../utils/passkeys'
import { browserSupportsWebAuthn, startRegistration, startAuthentication } from '@simplewebauthn/browser'

// Mock @simplewebauthn/browser
vi.mock('@simplewebauthn/browser', () => ({
  browserSupportsWebAuthn: vi.fn(),
  startRegistration: vi.fn(),
  startAuthentication: vi.fn(),
}))

// Mock fetch
const mockFetch = vi.fn()
global.fetch = mockFetch

describe('PasskeyManager', () => {
  beforeEach(() => {
    vi.clearAllMocks()
    
    // Default mock implementations
    browserSupportsWebAuthn.mockReturnValue(true)
    global.PublicKeyCredential = {
      isUserVerifyingPlatformAuthenticatorAvailable: vi.fn().mockResolvedValue(true)
    }
  })

  afterEach(() => {
    vi.clearAllMocks()
  })

  describe('isSupported', () => {
    it('returns true when WebAuthn is supported', () => {
      browserSupportsWebAuthn.mockReturnValue(true)
      
      expect(PasskeyManager.isSupported()).toBe(true)
    })

    it('returns false when WebAuthn is not supported', () => {
      browserSupportsWebAuthn.mockReturnValue(false)
      
      expect(PasskeyManager.isSupported()).toBe(false)
    })

    it('returns false when window is undefined (SSR)', () => {
      const originalWindow = global.window
      delete global.window
      
      expect(PasskeyManager.isSupported()).toBe(false)
      
      global.window = originalWindow
    })

    it('handles browserSupportsWebAuthn error gracefully', () => {
      browserSupportsWebAuthn.mockImplementation(() => {
        throw new Error('WebAuthn error')
      })
      
      expect(PasskeyManager.isSupported()).toBe(false)
    })
  })

  describe('isAvailable', () => {
    it('returns true when platform authenticator is available', async () => {
      browserSupportsWebAuthn.mockReturnValue(true)
      global.PublicKeyCredential.isUserVerifyingPlatformAuthenticatorAvailable.mockResolvedValue(true)
      
      const result = await PasskeyManager.isAvailable()
      expect(result).toBe(true)
    })

    it('returns false when platform authenticator is not available', async () => {
      browserSupportsWebAuthn.mockReturnValue(true)
      global.PublicKeyCredential.isUserVerifyingPlatformAuthenticatorAvailable.mockResolvedValue(false)
      
      const result = await PasskeyManager.isAvailable()
      expect(result).toBe(false)
    })

    it('returns false when WebAuthn is not supported', async () => {
      browserSupportsWebAuthn.mockReturnValue(false)
      
      const result = await PasskeyManager.isAvailable()
      expect(result).toBe(false)
    })

    it('returns false when window is undefined', async () => {
      const originalWindow = global.window
      delete global.window
      
      const result = await PasskeyManager.isAvailable()
      expect(result).toBe(false)
      
      global.window = originalWindow
    })

    it('returns false when PublicKeyCredential is not available', async () => {
      browserSupportsWebAuthn.mockReturnValue(true)
      delete global.PublicKeyCredential
      
      const result = await PasskeyManager.isAvailable()
      expect(result).toBe(false)
    })

    it('returns true as fallback when availability check fails', async () => {
      browserSupportsWebAuthn.mockReturnValue(true)
      global.PublicKeyCredential.isUserVerifyingPlatformAuthenticatorAvailable.mockRejectedValue(new Error('Check failed'))
      
      const result = await PasskeyManager.isAvailable()
      expect(result).toBe(true)
    })
  })

  describe('register', () => {
    const mockChallengeResponse = {
      challenge_id: 'test-challenge-id',
      challenge: 'test-challenge',
      rp: { id: 'localhost', name: 'Test' },
      user: { id: 'user-id', name: 'test@example.com', displayName: 'test' },
      pubKeyCredParams: [{ alg: -7, type: 'public-key' }],
      attestation: 'none',
      timeout: 60000
    }

    const mockRegistrationCredential = {
      id: 'credential-id',
      rawId: 'raw-id',
      response: {
        attestationObject: 'attestation-object',
        clientDataJSON: 'client-data-json'
      },
      type: 'public-key'
    }

    const mockTokenResponse = {
      token: 'auth-token',
      user: { email: 'test@example.com', name: 'test' },
      expires_in: 3600,
      token_type: 'Bearer'
    }

    beforeEach(() => {
      browserSupportsWebAuthn.mockReturnValue(true)
      
      mockFetch
        .mockResolvedValueOnce({
          ok: true,
          json: () => Promise.resolve(mockChallengeResponse)
        })
        .mockResolvedValueOnce({
          ok: true,
          json: () => Promise.resolve(mockTokenResponse)
        })
      
      startRegistration.mockResolvedValue(mockRegistrationCredential)
    })

    it('successfully registers a passkey', async () => {
      const result = await PasskeyManager.register('test@example.com', 'Test Device')
      
      expect(result).toEqual(mockTokenResponse)
      
      // Verify API calls
      expect(mockFetch).toHaveBeenCalledTimes(2)
      expect(mockFetch).toHaveBeenNthCalledWith(1, 
        'http://localhost:8080/api/v1/passkeys/register/start',
        expect.objectContaining({
          method: 'POST',
          headers: { 'Content-Type': 'application/json' },
          body: JSON.stringify({ email: 'test@example.com', device_name: 'Test Device' })
        })
      )
      
      expect(startRegistration).toHaveBeenCalledWith(mockChallengeResponse)
      
      expect(mockFetch).toHaveBeenNthCalledWith(2,
        'http://localhost:8080/api/v1/passkeys/register/complete?challenge_id=test-challenge-id',
        expect.objectContaining({
          method: 'POST',
          headers: { 'Content-Type': 'application/json' },
          body: JSON.stringify({
            credential: mockRegistrationCredential,
            device_name: 'Test Device'
          })
        })
      )
    })

    it('uses generated device name when none provided', async () => {
      // Mock navigator for device detection
      Object.defineProperty(navigator, 'userAgent', {
        value: 'Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) Chrome/120.0.0.0',
        configurable: true
      })
      Object.defineProperty(navigator, 'platform', {
        value: 'MacIntel',
        configurable: true
      })
      
      await PasskeyManager.register('test@example.com')
      
      expect(mockFetch).toHaveBeenNthCalledWith(2,
        expect.stringContaining('challenge_id=test-challenge-id'),
        expect.objectContaining({
          body: JSON.stringify({
            credential: mockRegistrationCredential,
            device_name: 'Mac (Chrome)'
          })
        })
      )
    })

    it('throws error when not supported', async () => {
      browserSupportsWebAuthn.mockReturnValue(false)
      
      await expect(PasskeyManager.register('test@example.com')).rejects.toThrow(
        'Your browser does not support passkeys. Please use a modern browser.'
      )
    })

    it('handles challenge request failure', async () => {
      mockFetch.mockReset()
      mockFetch.mockResolvedValueOnce({
        ok: false,
        json: () => Promise.resolve({ detail: 'Challenge failed' })
      })
      
      await expect(PasskeyManager.register('test@example.com')).rejects.toThrow('Challenge failed')
    })

    it('handles WebAuthn registration failure', async () => {
      startRegistration.mockRejectedValue(new Error('Registration failed'))
      
      await expect(PasskeyManager.register('test@example.com')).rejects.toThrow('Registration failed')
    })

    it('handles completion request failure', async () => {
      mockFetch.mockReset()
      mockFetch
        .mockResolvedValueOnce({
          ok: true,
          json: () => Promise.resolve(mockChallengeResponse)
        })
        .mockResolvedValueOnce({
          ok: false,
          json: () => Promise.resolve({ detail: 'Completion failed' })
        })
      
      await expect(PasskeyManager.register('test@example.com')).rejects.toThrow('Completion failed')
    })

    it('handles InvalidStateError with user-friendly message', async () => {
      const error = new Error('Invalid state')
      error.name = 'InvalidStateError'
      startRegistration.mockRejectedValue(error)
      
      await expect(PasskeyManager.register('test@example.com')).rejects.toThrow(
        'A passkey already exists for this device. Please try with a different device or remove the existing passkey first.'
      )
    })

    it('handles NotAllowedError with user-friendly message', async () => {
      const error = new Error('Not allowed')
      error.name = 'NotAllowedError'
      startRegistration.mockRejectedValue(error)
      
      await expect(PasskeyManager.register('test@example.com')).rejects.toThrow(
        'Passkey creation was cancelled or timed out. Please try again.'
      )
    })

    it('handles AbortError with user-friendly message', async () => {
      const error = new Error('Aborted')
      error.name = 'AbortError'
      startRegistration.mockRejectedValue(error)
      
      await expect(PasskeyManager.register('test@example.com')).rejects.toThrow(
        'Passkey creation was cancelled. Please try again.'
      )
    })

    it('handles NotSupportedError with user-friendly message', async () => {
      const error = new Error('Not supported')
      error.name = 'NotSupportedError'
      startRegistration.mockRejectedValue(error)
      
      await expect(PasskeyManager.register('test@example.com')).rejects.toThrow(
        'Your device does not support this type of passkey. Please try a different authentication method.'
      )
    })
  })

  describe('authenticate', () => {
    const mockAuthChallengeResponse = {
      challenge_id: 'auth-challenge-id',
      challenge: 'auth-challenge',
      allowCredentials: [{
        id: 'credential-id',
        type: 'public-key'
      }],
      timeout: 60000,
      rpId: 'localhost'
    }

    const mockAuthCredential = {
      id: 'credential-id',
      rawId: 'raw-id',
      response: {
        authenticatorData: 'auth-data',
        clientDataJSON: 'client-data-json',
        signature: 'signature'
      },
      type: 'public-key'
    }

    const mockAuthTokenResponse = {
      token: 'auth-token',
      user: { email: 'test@example.com', name: 'test' },
      expires_in: 3600,
      token_type: 'Bearer'
    }

    beforeEach(() => {
      browserSupportsWebAuthn.mockReturnValue(true)
      
      mockFetch
        .mockResolvedValueOnce({
          ok: true,
          json: () => Promise.resolve(mockAuthChallengeResponse)
        })
        .mockResolvedValueOnce({
          ok: true,
          json: () => Promise.resolve(mockAuthTokenResponse)
        })
      
      startAuthentication.mockResolvedValue(mockAuthCredential)
    })

    it('successfully authenticates with passkey', async () => {
      mockFetch.mockReset()
      mockFetch
        .mockResolvedValueOnce({
          ok: true,
          json: () => Promise.resolve(mockAuthChallengeResponse)
        })
        .mockResolvedValueOnce({
          ok: true,
          json: () => Promise.resolve(mockAuthTokenResponse)
        })
      
      const result = await PasskeyManager.authenticate('test@example.com')
      
      expect(result).toEqual(mockAuthTokenResponse)
      
      // Verify API calls
      expect(mockFetch).toHaveBeenCalledTimes(2)
      expect(mockFetch).toHaveBeenNthCalledWith(1,
        'http://localhost:8080/api/v1/passkeys/login/start',
        expect.objectContaining({
          method: 'POST',
          headers: { 'Content-Type': 'application/json' },
          body: JSON.stringify({ email: 'test@example.com' })
        })
      )
      
      expect(startAuthentication).toHaveBeenCalledWith(mockAuthChallengeResponse)
      
      expect(mockFetch).toHaveBeenNthCalledWith(2,
        'http://localhost:8080/api/v1/passkeys/login/complete?challenge_id=auth-challenge-id',
        expect.objectContaining({
          method: 'POST',
          headers: { 'Content-Type': 'application/json' },
          body: JSON.stringify({
            credential: mockAuthCredential
          })
        })
      )
    })

    it('authenticates without email for resident credentials', async () => {
      await PasskeyManager.authenticate(null)
      
      expect(mockFetch).toHaveBeenNthCalledWith(1,
        'http://localhost:8080/api/v1/passkeys/login/start',
        expect.objectContaining({
          body: JSON.stringify({ email: null })
        })
      )
    })

    it('throws error when not supported', async () => {
      browserSupportsWebAuthn.mockReturnValue(false)
      
      await expect(PasskeyManager.authenticate('test@example.com')).rejects.toThrow(
        'Your browser does not support passkeys. Please use password login instead.'
      )
    })

    it('handles authentication challenge failure', async () => {
      mockFetch.mockReset()
      mockFetch.mockResolvedValueOnce({
        ok: false,
        json: () => Promise.resolve({ detail: 'Auth challenge failed' })
      })
      
      await expect(PasskeyManager.authenticate('test@example.com')).rejects.toThrow('Auth challenge failed')
    })

    it('handles WebAuthn authentication failure', async () => {
      startAuthentication.mockRejectedValue(new Error('Authentication failed'))
      
      await expect(PasskeyManager.authenticate('test@example.com')).rejects.toThrow('Authentication failed')
    })

    it('handles completion failure', async () => {
      mockFetch.mockReset()
      mockFetch
        .mockResolvedValueOnce({
          ok: true,
          json: () => Promise.resolve(mockAuthChallengeResponse)
        })
        .mockResolvedValueOnce({
          ok: false,
          json: () => Promise.resolve({ detail: 'Auth completion failed' })
        })
      
      await expect(PasskeyManager.authenticate('test@example.com')).rejects.toThrow('Auth completion failed')
    })

    it('handles NotAllowedError during authentication', async () => {
      const error = new Error('Not allowed')
      error.name = 'NotAllowedError'
      startAuthentication.mockRejectedValue(error)
      
      await expect(PasskeyManager.authenticate('test@example.com')).rejects.toThrow(
        'Passkey authentication was cancelled or timed out. Please try again.'
      )
    })

    it('handles AbortError during authentication', async () => {
      const error = new Error('Aborted')
      error.name = 'AbortError'
      startAuthentication.mockRejectedValue(error)
      
      await expect(PasskeyManager.authenticate('test@example.com')).rejects.toThrow(
        'Passkey authentication was cancelled. Please try again.'
      )
    })

    it('handles InvalidStateError during authentication', async () => {
      const error = new Error('Invalid state')
      error.name = 'InvalidStateError'
      startAuthentication.mockRejectedValue(error)
      
      await expect(PasskeyManager.authenticate('test@example.com')).rejects.toThrow(
        'No passkey found for this account. Please register a passkey first.'
      )
    })
  })

  describe('listPasskeys', () => {
    const mockCredentialsList = {
      credentials: [
        {
          credential_id: 'cred1',
          device_name: 'Device 1',
          created_at: '2025-01-01T00:00:00Z'
        },
        {
          credential_id: 'cred2',
          device_name: 'Device 2',
          created_at: '2025-01-02T00:00:00Z'
        }
      ]
    }

    it('successfully lists user passkeys', async () => {
      mockFetch.mockReset()
      mockFetch.mockResolvedValue({
        ok: true,
        json: () => Promise.resolve(mockCredentialsList)
      })
      
      const result = await PasskeyManager.listPasskeys('auth-token')
      
      expect(result).toEqual(mockCredentialsList)
      expect(mockFetch).toHaveBeenCalledWith(
        'http://localhost:8080/api/v1/passkeys/list',
        expect.objectContaining({
          headers: {
            'Authorization': 'Bearer auth-token',
            'Content-Type': 'application/json'
          }
        })
      )
    })

    it('handles list request failure', async () => {
      mockFetch.mockReset()
      mockFetch.mockResolvedValue({
        ok: false,
        json: () => Promise.resolve({ detail: 'Unauthorized' })
      })
      
      await expect(PasskeyManager.listPasskeys('invalid-token')).rejects.toThrow('Unauthorized')
    })

    it('handles network error during list', async () => {
      mockFetch.mockReset()
      mockFetch.mockRejectedValue(new Error('Network error'))
      
      await expect(PasskeyManager.listPasskeys('auth-token')).rejects.toThrow('Network error')
    })
  })

  describe('_getDeviceName', () => {
    it('detects iPhone device', () => {
      Object.defineProperty(navigator, 'userAgent', {
        value: 'Mozilla/5.0 (iPhone; CPU iPhone OS 17_0 like Mac OS X) AppleWebKit/605.1.15 Safari/604.1',
        configurable: true
      })
      
      expect(PasskeyManager._getDeviceName()).toBe('iPhone/iPad (Safari)')
    })

    it('detects Android device', () => {
      Object.defineProperty(navigator, 'userAgent', {
        value: 'Mozilla/5.0 (Linux; Android 13; SM-G981B) AppleWebKit/537.36 Chrome/120.0.0.0',
        configurable: true
      })
      
      expect(PasskeyManager._getDeviceName()).toBe('Android (Chrome)')
    })

    it('detects Mac platform', () => {
      Object.defineProperty(navigator, 'userAgent', {
        value: 'Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) Chrome/120.0.0.0',
        configurable: true
      })
      Object.defineProperty(navigator, 'platform', {
        value: 'MacIntel',
        configurable: true
      })
      
      expect(PasskeyManager._getDeviceName()).toBe('Mac (Chrome)')
    })

    it('detects Windows platform', () => {
      Object.defineProperty(navigator, 'userAgent', {
        value: 'Mozilla/5.0 (Windows NT 10.0; Win64; x64) Chrome/120.0.0.0',
        configurable: true
      })
      Object.defineProperty(navigator, 'platform', {
        value: 'Win32',
        configurable: true
      })
      
      expect(PasskeyManager._getDeviceName()).toBe('Windows (Chrome)')
    })

    it('detects Linux platform', () => {
      Object.defineProperty(navigator, 'userAgent', {
        value: 'Mozilla/5.0 (X11; Linux x86_64) Chrome/120.0.0.0',
        configurable: true
      })
      Object.defineProperty(navigator, 'platform', {
        value: 'Linux x86_64',
        configurable: true
      })
      
      expect(PasskeyManager._getDeviceName()).toBe('Linux (Chrome)')
    })

    it('falls back to platform name for unknown platforms', () => {
      Object.defineProperty(navigator, 'userAgent', {
        value: 'UnknownBrowser/1.0',
        configurable: true
      })
      Object.defineProperty(navigator, 'platform', {
        value: 'UnknownOS',
        configurable: true
      })
      
      expect(PasskeyManager._getDeviceName()).toBe('UnknownOS (Browser)')
    })
  })

  describe('_getBrowser', () => {
    it('detects Chrome browser', () => {
      Object.defineProperty(navigator, 'userAgent', {
        value: 'Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/120.0.0.0',
        configurable: true
      })
      
      expect(PasskeyManager._getBrowser()).toBe('Chrome')
    })

    it('detects Firefox browser', () => {
      Object.defineProperty(navigator, 'userAgent', {
        value: 'Mozilla/5.0 (Windows NT 10.0; Win64; x64; rv:120.0) Gecko/20100101 Firefox/120.0',
        configurable: true
      })
      
      expect(PasskeyManager._getBrowser()).toBe('Firefox')
    })

    it('detects Safari browser', () => {
      Object.defineProperty(navigator, 'userAgent', {
        value: 'Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/605.1.15 Safari/605.1.15',
        configurable: true
      })
      
      expect(PasskeyManager._getBrowser()).toBe('Safari')
    })

    it('detects Edge browser', () => {
      Object.defineProperty(navigator, 'userAgent', {
        value: 'Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 Edge/120.0.0.0',
        configurable: true
      })
      
      expect(PasskeyManager._getBrowser()).toBe('Edge')
    })

    it('falls back to Browser for unknown browsers', () => {
      Object.defineProperty(navigator, 'userAgent', {
        value: 'UnknownBrowser/1.0',
        configurable: true
      })
      
      expect(PasskeyManager._getBrowser()).toBe('Browser')
    })
  })
})