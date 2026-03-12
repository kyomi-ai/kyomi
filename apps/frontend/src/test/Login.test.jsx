// SPDX-License-Identifier: AGPL-3.0-or-later
/**
 * @vitest-environment jsdom
 */

import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest'
import { render, screen, waitFor } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { BrowserRouter } from 'react-router-dom'

// Mock AuthContext first (hoisted)
vi.mock('../context/AuthContext', () => ({
  useAuth: vi.fn(),
  AuthProvider: ({ children }) => children,
}))

// Mock PasskeyManager
vi.mock('../utils/passkeys', () => ({
  default: {
    isAvailable: vi.fn(),
    register: vi.fn(),
    authenticate: vi.fn(),
  }
}))

// Now import components after mocks
import Login from '../pages/Login'
import { AuthProvider, useAuth } from '../context/AuthContext'
import PasskeyManager from '../utils/passkeys'

// Mock fetch
const mockFetch = vi.fn()
global.fetch = mockFetch

// Mock AuthContext data for Login component (unauthenticated state)
const mockAuthContext = {
  user: null,
  apiClient: null,
  isAuthenticated: false,
  loading: false,
  login: vi.fn().mockResolvedValue(true),
  logout: vi.fn()
}

// Test wrapper component
const TestWrapper = ({ children }) => (
  <BrowserRouter>
    {children}
  </BrowserRouter>
)

describe('Login Component', () => {
  beforeEach(() => {
    // Reset all mocks
    vi.clearAllMocks()
    
    // Set up AuthContext mock for unauthenticated state
    vi.mocked(useAuth).mockReturnValue(mockAuthContext)
    
    // Default mock implementations
    PasskeyManager.isAvailable.mockResolvedValue(true)
    mockFetch.mockResolvedValue({
      ok: true,
      json: () => Promise.resolve({ success: true })
    })
  })

  afterEach(() => {
    vi.clearAllMocks()
  })

  describe('Component Rendering', () => {
    it('renders login form correctly', () => {
      render(<Login />, { wrapper: TestWrapper })
      
      expect(screen.getByText('Welcome back')).toBeInTheDocument()
      expect(screen.getByLabelText('Email address')).toBeInTheDocument()
      expect(screen.getByLabelText('Password')).toBeInTheDocument()
      expect(screen.getByRole('button', { name: 'Sign in' })).toBeInTheDocument()
      expect(screen.getByText('Demo Mode')).toBeInTheDocument()
    })

    it('shows create account form when register is toggled', async () => {
      const user = userEvent.setup()
      render(<Login />, { wrapper: TestWrapper })
      
      const createAccountLink = screen.getByText('Create account')
      await user.click(createAccountLink)
      
      expect(screen.getByText('Create your account')).toBeInTheDocument()
      expect(screen.getByLabelText('Full name')).toBeInTheDocument()
      expect(screen.getByRole('button', { name: 'Create Account' })).toBeInTheDocument()
    })

    it('shows passkey options when supported', async () => {
      PasskeyManager.isAvailable.mockResolvedValue(true)
      
      render(<Login />, { wrapper: TestWrapper })
      
      await waitFor(() => {
        expect(screen.getByText('Sign in with Passkey')).toBeInTheDocument()
      })
    })

    it('does not show passkey options when not supported', async () => {
      PasskeyManager.isAvailable.mockResolvedValue(false)
      
      render(<Login />, { wrapper: TestWrapper })
      
      await waitFor(() => {
        expect(screen.queryByText('Sign in with Passkey')).not.toBeInTheDocument()
      })
    })

    it('handles passkey support check error gracefully', async () => {
      PasskeyManager.isAvailable.mockRejectedValue(new Error('Not supported'))
      
      render(<Login />, { wrapper: TestWrapper })
      
      await waitFor(() => {
        expect(screen.queryByText('Sign in with Passkey')).not.toBeInTheDocument()
      })
    })
  })

  describe('Form Validation', () => {
    it('requires email and password for login', async () => {
      const user = userEvent.setup()
      render(<Login />, { wrapper: TestWrapper })
      
      const submitButton = screen.getByRole('button', { name: 'Sign in' })
      expect(submitButton).toBeDisabled()
      
      const emailInput = screen.getByLabelText('Email address')
      const passwordInput = screen.getByLabelText('Password')
      
      await user.type(emailInput, 'test@example.com')
      expect(submitButton).toBeDisabled()
      
      await user.type(passwordInput, 'password123')
      expect(submitButton).toBeEnabled()
    })

    it('requires name, email for registration', async () => {
      const user = userEvent.setup()
      render(<Login />, { wrapper: TestWrapper })
      
      // Switch to registration
      await user.click(screen.getByText('Create account'))
      
      const submitButton = screen.getByRole('button', { name: 'Create Account' })
      expect(submitButton).toBeDisabled()
      
      const nameInput = screen.getByLabelText('Full name')
      const emailInput = screen.getByLabelText('Email address')
      const passwordInput = screen.getByLabelText('Password')
      
      await user.type(nameInput, 'Test User')
      expect(submitButton).toBeDisabled()
      
      await user.type(emailInput, 'test@example.com')
      expect(submitButton).toBeDisabled()
      
      await user.type(passwordInput, 'password123')
      expect(submitButton).toBeEnabled()
    })
  })

  describe('Login Flow', () => {
    it('handles successful login', async () => {
      const user = userEvent.setup()
      mockFetch.mockResolvedValue({
        ok: true,
        json: () => Promise.resolve({
          access_token: 'test-token',
          user: { email: 'test@example.com' }
        })
      })

      render(<Login />, { wrapper: TestWrapper })
      
      await user.type(screen.getByLabelText('Email address'), 'test@example.com')
      await user.type(screen.getByLabelText('Password'), 'password123')
      await user.click(screen.getByRole('button', { name: 'Sign in' }))
      
      expect(mockFetch).toHaveBeenCalledWith(
        'http://localhost:8080/api/v1/auth/login',
        expect.objectContaining({
          method: 'POST',
          headers: { 'Content-Type': 'application/json' },
          body: JSON.stringify({ email: 'test@example.com', password: 'password123' })
        })
      )
    })

    it('handles login failure', async () => {
      const user = userEvent.setup()
      mockFetch.mockResolvedValue({
        ok: false,
        json: () => Promise.resolve({ detail: 'Invalid credentials' })
      })

      render(<Login />, { wrapper: TestWrapper })
      
      await user.type(screen.getByLabelText('Email address'), 'test@example.com')
      await user.type(screen.getByLabelText('Password'), 'wrongpassword')
      await user.click(screen.getByRole('button', { name: 'Sign in' }))
      
      await waitFor(() => {
        expect(screen.getByText('Invalid credentials')).toBeInTheDocument()
      })
    })

    it('handles network error during login', async () => {
      const user = userEvent.setup()
      mockFetch.mockRejectedValue(new Error('Network error'))

      render(<Login />, { wrapper: TestWrapper })
      
      await user.type(screen.getByLabelText('Email address'), 'test@example.com')
      await user.type(screen.getByLabelText('Password'), 'password123')
      await user.click(screen.getByRole('button', { name: 'Sign in' }))
      
      await waitFor(() => {
        expect(screen.getByText('Login failed. Please try again.')).toBeInTheDocument()
      })
    })

    it('shows loading state during login', async () => {
      const user = userEvent.setup()
      let resolveLogin
      mockFetch.mockReturnValue(new Promise(resolve => {
        resolveLogin = resolve
      }))

      render(<Login />, { wrapper: TestWrapper })
      
      await user.type(screen.getByLabelText('Email address'), 'test@example.com')
      await user.type(screen.getByLabelText('Password'), 'password123')
      await user.click(screen.getByRole('button', { name: 'Sign in' }))
      
      expect(screen.getByText('Signing in...')).toBeInTheDocument()
      
      resolveLogin({
        ok: true,
        json: () => Promise.resolve({ access_token: 'test-token' })
      })
    })
  })

  describe('Registration Flow', () => {
    it('handles successful registration', async () => {
      const user = userEvent.setup()
      mockFetch.mockResolvedValue({
        ok: true,
        json: () => Promise.resolve({
          message: 'Registration successful! Please check your email for verification instructions. Your verification token is abc123 (for development only)',
          user_id: 'test-user-id'
        })
      })

      render(<Login />, { wrapper: TestWrapper })
      
      // Switch to registration
      await user.click(screen.getByText('Create account'))
      
      await user.type(screen.getByLabelText('Full name'), 'Test User')
      await user.type(screen.getByLabelText('Email address'), 'test@example.com')
      await user.type(screen.getByLabelText('Password'), 'password123')
      await user.click(screen.getByRole('button', { name: 'Create Account' }))
      
      expect(mockFetch).toHaveBeenCalledWith(
        'http://localhost:8080/api/v1/auth/register',
        expect.objectContaining({
          method: 'POST',
          headers: { 'Content-Type': 'application/json' },
          body: JSON.stringify({
            email: 'test@example.com',
            name: 'Test User',
            password: 'password123'
          })
        })
      )
      
      await waitFor(() => {
        expect(screen.getByText('Registration successful! Please verify your email using the link below.')).toBeInTheDocument()
        expect(screen.getByText('Click to Verify Email')).toBeInTheDocument()
      })
    })

    it('handles registration failure', async () => {
      const user = userEvent.setup()
      mockFetch.mockResolvedValue({
        ok: false,
        json: () => Promise.resolve({ detail: 'Email already exists' })
      })

      render(<Login />, { wrapper: TestWrapper })
      
      // Switch to registration
      await user.click(screen.getByText('Create account'))
      
      await user.type(screen.getByLabelText('Full name'), 'Test User')
      await user.type(screen.getByLabelText('Email address'), 'test@example.com')
      await user.type(screen.getByLabelText('Password'), 'password123')
      await user.click(screen.getByRole('button', { name: 'Create Account' }))
      
      await waitFor(() => {
        expect(screen.getByText('Email already exists')).toBeInTheDocument()
      })
    })

    it('handles email verification', async () => {
      const user = userEvent.setup()
      
      // Mock registration response
      mockFetch.mockResolvedValueOnce({
        ok: true,
        json: () => Promise.resolve({
          message: 'Registration successful! Please check your email for verification instructions. Your verification token is abc123 (for development only)',
          user_id: 'test-user-id'
        })
      })
      
      // Mock verification response
      mockFetch.mockResolvedValueOnce({
        ok: true,
        json: () => Promise.resolve({
          message: 'Email verified successfully'
        })
      })

      render(<Login />, { wrapper: TestWrapper })
      
      // Switch to registration and register
      await user.click(screen.getByText('Create account'))
      await user.type(screen.getByLabelText('Full name'), 'Test User')
      await user.type(screen.getByLabelText('Email address'), 'test@example.com')
      await user.type(screen.getByLabelText('Password'), 'password123')
      await user.click(screen.getByRole('button', { name: 'Create Account' }))
      
      // Wait for registration success and click verify
      await waitFor(() => {
        expect(screen.getByText('Click to Verify Email')).toBeInTheDocument()
      })
      
      await user.click(screen.getByText('Click to Verify Email'))
      
      expect(mockFetch).toHaveBeenCalledWith(
        'http://localhost:8080/api/v1/auth/verify-email',
        expect.objectContaining({
          method: 'POST',
          headers: { 'Content-Type': 'application/json' },
          body: JSON.stringify({ token: 'abc123 (for development only' })
        })
      )
      
      await waitFor(() => {
        expect(screen.getByText('Email verified successfully! You can now login.')).toBeInTheDocument()
      })
    })

    it('handles email verification failure', async () => {
      const user = userEvent.setup()
      
      // Mock registration response with token
      mockFetch.mockResolvedValueOnce({
        ok: true,
        json: () => Promise.resolve({
          message: 'Registration successful! Please check your email for verification instructions. Your verification token is abc123 (for development only)',
          user_id: 'test-user-id'
        })
      })
      
      // Mock verification failure response
      mockFetch.mockResolvedValueOnce({
        ok: false,
        json: () => Promise.resolve({
          detail: 'Invalid verification token'
        })
      })

      render(<Login />, { wrapper: TestWrapper })
      
      // Switch to registration and register
      await user.click(screen.getByText('Create account'))
      await user.type(screen.getByLabelText('Full name'), 'Test User')
      await user.type(screen.getByLabelText('Email address'), 'test@example.com')
      await user.type(screen.getByLabelText('Password'), 'password123')
      await user.click(screen.getByRole('button', { name: 'Create Account' }))
      
      // Wait for registration success and click verify
      await waitFor(() => {
        expect(screen.getByText('Click to Verify Email')).toBeInTheDocument()
      })
      
      await user.click(screen.getByText('Click to Verify Email'))
      
      await waitFor(() => {
        expect(screen.getByText('Verification failed. Please try again.')).toBeInTheDocument()
      })
    })

    it('handles email verification network error', async () => {
      const user = userEvent.setup()
      
      // Mock registration response with token
      mockFetch.mockResolvedValueOnce({
        ok: true,
        json: () => Promise.resolve({
          message: 'Registration successful! Please check your email for verification instructions. Your verification token is abc123 (for development only)',
          user_id: 'test-user-id'
        })
      })
      
      // Mock verification network error
      mockFetch.mockRejectedValueOnce(new Error('Network error'))

      render(<Login />, { wrapper: TestWrapper })
      
      // Switch to registration and register
      await user.click(screen.getByText('Create account'))
      await user.type(screen.getByLabelText('Full name'), 'Test User')
      await user.type(screen.getByLabelText('Email address'), 'test@example.com')
      await user.type(screen.getByLabelText('Password'), 'password123')
      await user.click(screen.getByRole('button', { name: 'Create Account' }))
      
      // Wait for registration success and click verify
      await waitFor(() => {
        expect(screen.getByText('Click to Verify Email')).toBeInTheDocument()
      })
      
      await user.click(screen.getByText('Click to Verify Email'))
      
      await waitFor(() => {
        expect(screen.getByText('Verification failed. Please try again.')).toBeInTheDocument()
      })
    })
  })

  describe('Demo Login', () => {
    it('handles successful demo login', async () => {
      const user = userEvent.setup()
      mockFetch.mockResolvedValue({
        ok: true,
        json: () => Promise.resolve({
          access_token: 'demo-token',
          user: { email: 'admin@kyomi.dev' }
        })
      })

      render(<Login />, { wrapper: TestWrapper })
      
      await user.click(screen.getByText('Demo Mode'))
      
      expect(mockFetch).toHaveBeenCalledWith(
        'http://localhost:8080/api/v1/auth/login',
        expect.objectContaining({
          method: 'POST',
          headers: { 'Content-Type': 'application/json' },
          body: JSON.stringify({ email: 'admin@kyomi.dev', password: 'admin123' })
        })
      )
    })

    it('handles demo login failure', async () => {
      const user = userEvent.setup()
      mockFetch.mockResolvedValue({
        ok: false,
        json: () => Promise.resolve({ detail: 'Demo mode not available' })
      })

      render(<Login />, { wrapper: TestWrapper })
      
      await user.click(screen.getByText('Demo Mode'))
      
      await waitFor(() => {
        expect(screen.getByText('Demo mode failed - admin credentials not found.')).toBeInTheDocument()
      })
    })
  })

  describe('Passkey Authentication', () => {
    it('handles successful passkey login', async () => {
      const user = userEvent.setup()
      PasskeyManager.authenticate.mockResolvedValue({
        token: 'passkey-token',
        user: { email: 'test@example.com' }
      })

      render(<Login />, { wrapper: TestWrapper })
      
      await waitFor(() => {
        expect(screen.getByText('Sign in with Passkey')).toBeInTheDocument()
      })
      
      await user.type(screen.getByLabelText('Email address'), 'test@example.com')
      await user.click(screen.getByText('Sign in with Passkey'))
      
      expect(PasskeyManager.authenticate).toHaveBeenCalledWith('test@example.com')
    })

    it('handles passkey login with session creation failure', async () => {
      const user = userEvent.setup()
      
      // Mock passkey authenticate to succeed
      PasskeyManager.authenticate.mockResolvedValue({
        token: 'passkey-token',
        user: { email: 'test@example.com' }
      })
      
      // Mock login to return false (session creation fails)
      vi.mocked(useAuth).mockReset()
      const mockLogin = vi.fn().mockResolvedValue(false)
      vi.mocked(useAuth).mockReturnValue({
        user: null,
        apiClient: null,
        isAuthenticated: false,
        loading: false,
        login: mockLogin,
        logout: vi.fn()
      })
      
      render(<Login />, { wrapper: TestWrapper })
      
      // Enter email first (for passkey login with email)
      await user.type(screen.getByLabelText('Email address'), 'test@example.com')
      
      await waitFor(() => {
        expect(screen.getByText('Sign in with Passkey')).toBeInTheDocument()
      })
      
      await user.click(screen.getByText('Sign in with Passkey'))
      
      expect(PasskeyManager.authenticate).toHaveBeenCalledWith('test@example.com')
      
      await waitFor(() => {
        expect(screen.getByText('Passkey login succeeded but session creation failed. Please try again.')).toBeInTheDocument()
      })
    })

    it('handles passkey login failure', async () => {
      const user = userEvent.setup()
      PasskeyManager.authenticate.mockRejectedValue(new Error('Passkey authentication failed'))

      render(<Login />, { wrapper: TestWrapper })
      
      await waitFor(() => {
        expect(screen.getByText('Sign in with Passkey')).toBeInTheDocument()
      })
      
      await user.type(screen.getByLabelText('Email address'), 'test@example.com')
      await user.click(screen.getByText('Sign in with Passkey'))
      
      await waitFor(() => {
        expect(screen.getByText('Passkey authentication failed')).toBeInTheDocument()
      })
    })

    it('shows loading state during passkey authentication', async () => {
      const user = userEvent.setup()
      let resolvePasskey
      PasskeyManager.authenticate.mockReturnValue(new Promise(resolve => {
        resolvePasskey = resolve
      }))

      render(<Login />, { wrapper: TestWrapper })
      
      await waitFor(() => {
        expect(screen.getByText('Sign in with Passkey')).toBeInTheDocument()
      })
      
      await user.click(screen.getByText('Sign in with Passkey'))
      
      expect(screen.getByText('Authenticating...')).toBeInTheDocument()
      
      resolvePasskey({ token: 'passkey-token' })
    })

    it('handles successful passkey registration', async () => {
      const user = userEvent.setup()
      
      // Set up mocks before rendering
      PasskeyManager.register.mockResolvedValue({
        token: 'passkey-registration-token',
        user: { email: 'test@example.com' }
      })
      
      // Reset the default mock and set up our custom one
      vi.mocked(useAuth).mockReset()
      const mockLogin = vi.fn().mockResolvedValue(true)
      vi.mocked(useAuth).mockReturnValue({
        user: null,
        apiClient: null,
        isAuthenticated: false,
        loading: false,
        login: mockLogin,
        logout: vi.fn()
      })
      
      render(<Login />, { wrapper: TestWrapper })
      
      // Switch to registration
      await user.click(screen.getByText('Create account'))
      
      await waitFor(() => {
        expect(screen.getByText('Create Account with Passkey')).toBeInTheDocument()
      })
      
      await user.type(screen.getByLabelText('Full name'), 'Test User')
      await user.type(screen.getByLabelText('Email address'), 'test@example.com')
      await user.click(screen.getByText('Create Account with Passkey'))
      
      expect(PasskeyManager.register).toHaveBeenCalledWith('test@example.com', 'Test User')
      
      // Wait for login to be called with the token
      await waitFor(() => {
        expect(mockLogin).toHaveBeenCalledWith('passkey-registration-token')
      })
      
      // After successful login, the user should be authenticated and redirected
      // Since we can't test actual navigation in unit tests, we can verify that
      // login was called successfully and the component would redirect
      // The success message won't be shown because the component redirects immediately
      // So we just verify the flow completed successfully
      expect(mockLogin).toHaveBeenCalledTimes(1)
      expect(PasskeyManager.register).toHaveBeenCalledTimes(1)
    })

    it('handles passkey registration failure', async () => {
      const user = userEvent.setup()
      PasskeyManager.register.mockRejectedValue(new Error('Passkey registration failed'))

      render(<Login />, { wrapper: TestWrapper })
      
      // Switch to registration
      await user.click(screen.getByText('Create account'))
      
      await waitFor(() => {
        expect(screen.getByText('Create Account with Passkey')).toBeInTheDocument()
      })
      
      await user.type(screen.getByLabelText('Full name'), 'Test User')
      await user.type(screen.getByLabelText('Email address'), 'test@example.com')
      await user.click(screen.getByText('Create Account with Passkey'))
      
      await waitFor(() => {
        expect(screen.getByText('Passkey registration failed')).toBeInTheDocument()
      })
    })

    it('shows error when trying to register passkey without email or name', async () => {
      const user = userEvent.setup()
      render(<Login />, { wrapper: TestWrapper })
      
      // Switch to registration
      await user.click(screen.getByText('Create account'))
      
      // Wait for passkey button to appear
      await waitFor(() => {
        expect(screen.getByText('Create Account with Passkey')).toBeInTheDocument()
      })
      
      // Try to click without filling form
      const passkeyButton = screen.getByRole('button', { name: /Create Account with Passkey/i })
      
      // Button should be disabled when fields are empty
      expect(passkeyButton).toBeDisabled()
    })

    it('handles passkey registration with login failure', async () => {
      const user = userEvent.setup()
      
      // Set up mocks
      PasskeyManager.register.mockResolvedValue({
        token: 'passkey-registration-token',
        user: { email: 'test@example.com' }
      })
      
      // Mock login to return false (failure)
      vi.mocked(useAuth).mockReset()
      const mockLogin = vi.fn().mockResolvedValue(false)
      vi.mocked(useAuth).mockReturnValue({
        user: null,
        apiClient: null,
        isAuthenticated: false,
        loading: false,
        login: mockLogin,
        logout: vi.fn()
      })
      
      render(<Login />, { wrapper: TestWrapper })
      
      // Switch to registration
      await user.click(screen.getByText('Create account'))
      
      await waitFor(() => {
        expect(screen.getByText('Create Account with Passkey')).toBeInTheDocument()
      })
      
      await user.type(screen.getByLabelText('Full name'), 'Test User')
      await user.type(screen.getByLabelText('Email address'), 'test@example.com')
      await user.click(screen.getByText('Create Account with Passkey'))
      
      await waitFor(() => {
        expect(screen.getByText('Passkey registration succeeded but session creation failed. Please try again.')).toBeInTheDocument()
      })
    })

    it('requires name and email for passkey registration', async () => {
      const user = userEvent.setup()
      render(<Login />, { wrapper: TestWrapper })
      
      // Switch to registration
      await user.click(screen.getByText('Create account'))
      
      // Wait for passkey button to appear (after async passkey support check)
      await waitFor(() => {
        expect(screen.getByText('Create Account with Passkey')).toBeInTheDocument()
      })
      
      // Initially the button should be disabled (no name or email)
      // Use getByRole to get the actual button element, not the text span
      const passkeyButton = screen.getByRole('button', { name: /Create Account with Passkey/i })
      expect(passkeyButton).toBeDisabled()
      
      // Add name only - button should still be disabled
      await user.type(screen.getByLabelText('Full name'), 'Test User')
      expect(passkeyButton).toBeDisabled()
      
      // Add email - now button should be enabled
      await user.type(screen.getByLabelText('Email address'), 'test@example.com')
      
      await waitFor(() => {
        expect(passkeyButton).toBeEnabled()
      })
    })
  })

  describe('Form State Management', () => {
    it('clears form when switching between login and register', async () => {
      const user = userEvent.setup()
      render(<Login />, { wrapper: TestWrapper })
      
      // Fill login form
      await user.type(screen.getByLabelText('Email address'), 'test@example.com')
      await user.type(screen.getByLabelText('Password'), 'password123')
      
      // Switch to registration
      await user.click(screen.getByText('Create account'))
      
      // Switch back to login
      await user.click(screen.getByText('Sign in'))
      
      // Form should be cleared
      expect(screen.getByLabelText('Email address')).toHaveValue('')
      expect(screen.getByLabelText('Password')).toHaveValue('')
    })

    it('clears error messages when switching forms', async () => {
      const user = userEvent.setup()
      mockFetch.mockResolvedValue({
        ok: false,
        json: () => Promise.resolve({ detail: 'Login failed' })
      })

      render(<Login />, { wrapper: TestWrapper })
      
      // Trigger login error
      await user.type(screen.getByLabelText('Email address'), 'test@example.com')
      await user.type(screen.getByLabelText('Password'), 'wrongpassword')
      await user.click(screen.getByRole('button', { name: 'Sign in' }))
      
      await waitFor(() => {
        expect(screen.getByText('Login failed')).toBeInTheDocument()
      })
      
      // Switch to registration
      await user.click(screen.getByText('Create account'))
      
      // Error should be cleared
      expect(screen.queryByText('Login failed')).not.toBeInTheDocument()
    })

    it('maintains registration success state when switching to login', async () => {
      const user = userEvent.setup()
      mockFetch.mockResolvedValue({
        ok: true,
        json: () => Promise.resolve({
          message: 'Registration successful! Please check your email for verification instructions. Your verification token is abc123 (for development only)',
          user_id: 'test-user-id'
        })
      })

      render(<Login />, { wrapper: TestWrapper })
      
      // Register successfully
      await user.click(screen.getByText('Create account'))
      await user.type(screen.getByLabelText('Full name'), 'Test User')
      await user.type(screen.getByLabelText('Email address'), 'test@example.com')
      await user.type(screen.getByLabelText('Password'), 'password123')
      await user.click(screen.getByRole('button', { name: 'Create Account' }))
      
      await waitFor(() => {
        expect(screen.getByText('Registration successful! Please verify your email using the link below.')).toBeInTheDocument()
      })
      
      // Switch to login - success message should persist
      await user.click(screen.getByText('Sign in'))
      
      expect(screen.getByText('Registration successful! Please verify your email using the link below.')).toBeInTheDocument()
    })
  })
})