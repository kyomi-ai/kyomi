// SPDX-License-Identifier: AGPL-3.0-or-later
/**
 * Centralized API configuration
 */
export const API_CONFIG = {
  baseURL: import.meta.env.VITE_API_BASE_URL || '',  // Empty for proxy mode
  endpoints: {
    // Auth endpoints
    auth: {
      login: '/api/v1/auth/login',
      register: '/api/v1/auth/register',
      verify: '/api/v1/auth/verify',
      checkEmail: '/api/v1/auth/check-email',
      profile: '/api/v1/auth/profile'
    },
    // Passkey endpoints
    passkeys: {
      registerStart: '/api/v1/auth/passkeys/register/start',
      registerComplete: '/api/v1/auth/passkeys/register/complete',
      signupComplete: '/api/v1/auth/passkeys/signup/complete',
      addStart: '/api/v1/auth/passkeys/add/start',
      addComplete: '/api/v1/auth/passkeys/add/complete',
      loginStart: '/api/v1/auth/passkeys/login/start',
      loginComplete: '/api/v1/auth/passkeys/login/complete',
      list: '/api/v1/auth/passkeys/list',
      delete: '/api/v1/auth/passkeys',  // Append /{credential_id}
      update: '/api/v1/auth/passkeys',  // Append /{credential_id}
      recoveryRequest: '/api/v1/auth/passkeys/recovery/request',
      recoveryVerify: '/api/v1/auth/passkeys/recovery/verify',
      recoveryRegister: '/api/v1/auth/passkeys/recovery/register'
    },
    // 2FA TOTP endpoints
    totp: {
      setup: '/api/v1/auth/2fa/setup',
      enable: '/api/v1/auth/2fa/enable',
      disable: '/api/v1/auth/2fa/disable',
      status: '/api/v1/auth/2fa/status',
      verify: '/api/v1/auth/2fa/verify'
    },
    // Other endpoints
    health: '/health',
    demoToken: '/demo-token'
  }
};

export default API_CONFIG;