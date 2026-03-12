// SPDX-License-Identifier: AGPL-3.0-or-later
import { expect, vi } from 'vitest'
import '@testing-library/jest-dom'

// Set up global expect for jest-dom matchers
global.expect = expect

// Set environment variables for tests
import.meta.env.VITE_USE_DIRECT_BIGQUERY = 'true'

// Mock IntersectionObserver
global.IntersectionObserver = class IntersectionObserver {
  constructor() {}
  observe() {
    return null;
  }
  disconnect() {
    return null;
  }
  unobserve() {
    return null;
  }
};

// Mock ResizeObserver
global.ResizeObserver = class ResizeObserver {
  constructor() {}
  observe() {
    return null;
  }
  disconnect() {
    return null;
  }
  unobserve() {
    return null;
  }
};

// Mock matchMedia
Object.defineProperty(window, 'matchMedia', {
  writable: true,
  value: vi.fn().mockImplementation(query => ({
    matches: false,
    media: query,
    onchange: null,
    addListener: vi.fn(),
    removeListener: vi.fn(),
    addEventListener: vi.fn(),
    removeEventListener: vi.fn(),
    dispatchEvent: vi.fn(),
  })),
});

// Mock WebAuthn APIs
global.PublicKeyCredential = {
  isUserVerifyingPlatformAuthenticatorAvailable: vi.fn().mockResolvedValue(true),
  isConditionalMediationAvailable: vi.fn().mockResolvedValue(true),
};

global.navigator.credentials = {
  create: vi.fn(),
  get: vi.fn(),
};

// Mock console methods to reduce test noise
global.console = {
  ...console,
  log: vi.fn(),
  warn: vi.fn(),
  error: vi.fn(),
};