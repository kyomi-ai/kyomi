// SPDX-License-Identifier: AGPL-3.0-or-later
/**
 * @vitest-environment jsdom
 */

import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest'
import { render, screen, waitFor } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { BrowserRouter } from 'react-router-dom'

// Mock AuthContext
vi.mock('../context/AuthContext', () => ({
  useAuth: vi.fn(),
}))

// Mock heroicons
vi.mock('@heroicons/react/24/outline', () => ({
  PlusIcon: () => <span>PlusIcon</span>,
  KeyIcon: () => <span>KeyIcon</span>,
}))

// Import components after mocks
import Users from '../pages/Users'
import { useAuth } from '../context/AuthContext'

// Mock data
const mockUsers = [
  {
    email: 'admin@example.com',
    name: 'Admin User',
    roles: ['admin'],
    created_at: '2025-01-01T00:00:00Z'
  },
  {
    email: 'user@example.com',
    name: 'Regular User',
    roles: ['user'],
    created_at: '2025-01-02T00:00:00Z'
  },
  {
    email: 'noname@example.com',
    name: null,
    roles: ['user'],
    created_at: null
  }
]

// Mock API client
const mockApiClient = {
  getUsers: vi.fn(),
  createUser: vi.fn(),
  createToken: vi.fn(),
}

// Test wrapper with React Query
const createTestWrapper = () => {
  const queryClient = new QueryClient({
    defaultOptions: {
      queries: { retry: false },
      mutations: { retry: false },
    },
  })

  return ({ children }) => (
    <QueryClientProvider client={queryClient}>
      <BrowserRouter>
        {children}
      </BrowserRouter>
    </QueryClientProvider>
  )
}

describe('Users Component', () => {
  let TestWrapper

  beforeEach(() => {
    vi.clearAllMocks()
    TestWrapper = createTestWrapper()
    
    // Set up default mock for useAuth
    vi.mocked(useAuth).mockReturnValue({
      apiClient: mockApiClient,
      user: { email: 'admin@example.com' },
      isAuthenticated: true,
    })

    // Default mock implementations
    mockApiClient.getUsers.mockResolvedValue(mockUsers)
    mockApiClient.createUser.mockResolvedValue({ 
      email: 'newuser@example.com',
      name: 'New User',
      roles: ['user']
    })
    mockApiClient.createToken.mockResolvedValue({
      token: 'test-token-123',
      expires_at: '2025-02-01T00:00:00Z'
    })
  })

  afterEach(() => {
    vi.clearAllMocks()
  })

  describe('Component Loading', () => {
    it('shows loading state while fetching users', () => {
      // Mock a pending promise
      mockApiClient.getUsers.mockImplementation(() => new Promise(() => {}))
      
      render(<Users />, { wrapper: TestWrapper })
      
      // Check for loading animation elements (no text in loading state)
      expect(document.querySelectorAll('.animate-pulse').length).toBeGreaterThan(0)
      expect(document.querySelector('.bg-gradient-to-r.from-gray-200.to-gray-300')).toBeInTheDocument()
    })

    it('renders user management page after loading', async () => {
      render(<Users />, { wrapper: TestWrapper })
      
      await waitFor(() => {
        expect(screen.getByText('Admin User')).toBeInTheDocument()
      })
      
      expect(screen.getByText('User Management')).toBeInTheDocument()
      expect(screen.getByText('Add User')).toBeInTheDocument()
      expect(screen.getByText('admin@example.com')).toBeInTheDocument()
      expect(screen.getByText('Regular User')).toBeInTheDocument()
    })
  })

  describe('User Display', () => {
    it('displays all user information correctly', async () => {
      render(<Users />, { wrapper: TestWrapper })
      
      await waitFor(() => {
        expect(screen.getByText('Admin User')).toBeInTheDocument()
      })
      
      // Check first user (admin)
      expect(screen.getByText('admin@example.com')).toBeInTheDocument()
      expect(screen.getByText('ADMIN')).toBeInTheDocument()
      expect(screen.getByText('Created 01/01/2025')).toBeInTheDocument()
      
      // Check second user
      expect(screen.getByText('user@example.com')).toBeInTheDocument()
      expect(screen.getByText('Regular User')).toBeInTheDocument()
      // There are multiple USER badges, use getAllByText
      const userBadges = screen.getAllByText('USER')
      expect(userBadges.length).toBeGreaterThan(0)
    })

    it('displays user initials in avatar', async () => {
      render(<Users />, { wrapper: TestWrapper })
      
      await waitFor(() => {
        expect(screen.getByText('Admin User')).toBeInTheDocument()
      })
      
      // Check avatar initials
      expect(screen.getByText('A')).toBeInTheDocument() // Admin User
      expect(screen.getByText('R')).toBeInTheDocument() // Regular User
    })

    it('handles user without name correctly', async () => {
      render(<Users />, { wrapper: TestWrapper })
      
      await waitFor(() => {
        expect(screen.getByText('noname@example.com')).toBeInTheDocument()
      })
      
      // Should use email initial when name is null
      expect(screen.getByText('N')).toBeInTheDocument() // noname@example.com
    })

    it('handles user without created_at date', async () => {
      render(<Users />, { wrapper: TestWrapper })
      
      await waitFor(() => {
        expect(screen.getByText('noname@example.com')).toBeInTheDocument()
      })
      
      // Should display Created N/A for missing date
      expect(screen.getByText('Created N/A')).toBeInTheDocument()
    })

    it('displays multiple roles correctly', async () => {
      const multiRoleUsers = [{
        email: 'multi@example.com',
        name: 'Multi Role User',
        roles: ['admin', 'user', 'developer'],
        created_at: '2025-01-01T00:00:00Z'
      }]
      
      mockApiClient.getUsers.mockResolvedValue(multiRoleUsers)
      
      render(<Users />, { wrapper: TestWrapper })
      
      await waitFor(() => {
        expect(screen.getByText('Multi Role User')).toBeInTheDocument()
      })
      
      // Check all roles are displayed (might have multiple of each)
      expect(screen.getAllByText('ADMIN').length).toBeGreaterThan(0)
      expect(screen.getAllByText('USER').length).toBeGreaterThan(0)
      expect(screen.getByText('DEVELOPER')).toBeInTheDocument()
    })
  })

  describe('Create User Modal', () => {
    it('opens create user modal when Add User button is clicked', async () => {
      const user = userEvent.setup()
      render(<Users />, { wrapper: TestWrapper })
      
      await waitFor(() => {
        expect(screen.getByText('Admin User')).toBeInTheDocument()
      })
      
      await user.click(screen.getByText('Add User'))
      
      expect(screen.getByText('Create New User')).toBeInTheDocument()
      expect(screen.getByText('Add a new team member to your platform')).toBeInTheDocument()
      expect(screen.getByPlaceholderText('name@company.com')).toBeInTheDocument()
      expect(screen.getByPlaceholderText('John Doe')).toBeInTheDocument()
      expect(screen.getByRole('combobox')).toBeInTheDocument()
    })

    it('closes create user modal when Cancel is clicked', async () => {
      const user = userEvent.setup()
      render(<Users />, { wrapper: TestWrapper })
      
      await waitFor(() => {
        expect(screen.getByText('Admin User')).toBeInTheDocument()
      })
      
      await user.click(screen.getByText('Add User'))
      expect(screen.getByText('Create New User')).toBeInTheDocument()
      
      await user.click(screen.getByText('Cancel'))
      
      expect(screen.queryByText('Create New User')).not.toBeInTheDocument()
    })

    it('creates a new user successfully', async () => {
      const user = userEvent.setup()
      render(<Users />, { wrapper: TestWrapper })
      
      await waitFor(() => {
        expect(screen.getByText('Admin User')).toBeInTheDocument()
      })
      
      await user.click(screen.getByText('Add User'))
      
      // Fill form
      await user.type(screen.getByPlaceholderText('name@company.com'), 'newuser@example.com')
      await user.type(screen.getByPlaceholderText('John Doe'), 'New User')
      await user.selectOptions(screen.getByRole('combobox'), 'admin')
      
      // Submit form
      await user.click(screen.getByRole('button', { name: 'Create User' }))
      
      await waitFor(() => {
        expect(mockApiClient.createUser).toHaveBeenCalledWith({
          email: 'newuser@example.com',
          name: 'New User',
          roles: ['admin']
        })
      })
      
      // Modal should close after success
      await waitFor(() => {
        expect(screen.queryByText('Create New User')).not.toBeInTheDocument()
      })
    })

    it('shows loading state while creating user', async () => {
      const user = userEvent.setup()
      
      // Mock a slow API call
      let resolveCreate
      mockApiClient.createUser.mockImplementation(() => 
        new Promise(resolve => { resolveCreate = resolve })
      )
      
      render(<Users />, { wrapper: TestWrapper })
      
      await waitFor(() => {
        expect(screen.getByText('Admin User')).toBeInTheDocument()
      })
      
      await user.click(screen.getByText('Add User'))
      
      // Fill and submit form
      await user.type(screen.getByPlaceholderText('name@company.com'), 'test@example.com')
      await user.type(screen.getByPlaceholderText('John Doe'), 'Test User')
      await user.click(screen.getByRole('button', { name: 'Create User' }))
      
      // Check loading state
      expect(screen.getByText('Creating...')).toBeInTheDocument()
      expect(screen.getByRole('button', { name: /Creating/ })).toBeDisabled()
      
      // Resolve the promise
      resolveCreate({ email: 'test@example.com', name: 'Test User', roles: ['user'] })
    })
  })

  describe('Create Token Modal', () => {
    it('opens create token modal when Create Token button is clicked', async () => {
      const user = userEvent.setup()
      render(<Users />, { wrapper: TestWrapper })
      
      await waitFor(() => {
        expect(screen.getByText('Admin User')).toBeInTheDocument()
      })
      
      // Click create token for first user
      const createTokenButtons = screen.getAllByText('Create Token')
      await user.click(createTokenButtons[0])
      
      expect(screen.getByText('Create API Token')).toBeInTheDocument()
      // The email appears in both the user list and modal, so just check modal title is shown
      // and that the description input is present (which indicates modal is open)
      expect(screen.getByPlaceholderText('e.g., Mobile app token')).toBeInTheDocument()
      expect(screen.getByRole('spinbutton')).toBeInTheDocument()
    })

    it('closes create token modal when Cancel is clicked', async () => {
      const user = userEvent.setup()
      render(<Users />, { wrapper: TestWrapper })
      
      await waitFor(() => {
        expect(screen.getByText('Admin User')).toBeInTheDocument()
      })
      
      const createTokenButtons = screen.getAllByText('Create Token')
      await user.click(createTokenButtons[0])
      expect(screen.getByText('Create API Token')).toBeInTheDocument()
      
      await user.click(screen.getByText('Cancel'))
      
      expect(screen.queryByText('Create API Token')).not.toBeInTheDocument()
    })

    it('creates a token successfully', async () => {
      const user = userEvent.setup()
      render(<Users />, { wrapper: TestWrapper })
      
      await waitFor(() => {
        expect(screen.getByText('Admin User')).toBeInTheDocument()
      })
      
      const createTokenButtons = screen.getAllByText('Create Token')
      await user.click(createTokenButtons[0])
      
      // Fill form
      await user.type(screen.getByPlaceholderText('e.g., Mobile app token'), 'Test Token')
      const expiryInput = screen.getByRole('spinbutton')
      await user.clear(expiryInput)
      await user.type(expiryInput, '60')
      
      // Submit form
      await user.click(screen.getByRole('button', { name: 'Create Token' }))
      
      await waitFor(() => {
        expect(mockApiClient.createToken).toHaveBeenCalledWith({
          user_email: 'admin@example.com',
          description: 'Test Token',
          expires_in_days: 60
        })
      })
      
      // Modal should close after success
      await waitFor(() => {
        expect(screen.queryByText('Create API Token')).not.toBeInTheDocument()
      })
    })

    it('shows loading state while creating token', async () => {
      const user = userEvent.setup()
      
      // Mock a slow API call
      let resolveCreate
      mockApiClient.createToken.mockImplementation(() => 
        new Promise(resolve => { resolveCreate = resolve })
      )
      
      render(<Users />, { wrapper: TestWrapper })
      
      await waitFor(() => {
        expect(screen.getByText('Admin User')).toBeInTheDocument()
      })
      
      const createTokenButtons = screen.getAllByText('Create Token')
      await user.click(createTokenButtons[0])
      
      // Fill and submit form
      await user.type(screen.getByPlaceholderText('e.g., Mobile app token'), 'Test Token')
      await user.click(screen.getByRole('button', { name: 'Create Token' }))
      
      // Check loading state
      expect(screen.getByText('Creating...')).toBeInTheDocument()
      expect(screen.getByRole('button', { name: /Creating/ })).toBeDisabled()
      
      // Resolve the promise
      resolveCreate({ token: 'test-token', expires_at: '2025-02-01T00:00:00Z' })
    })

    it('uses default expiry value of 30 days', async () => {
      const user = userEvent.setup()
      render(<Users />, { wrapper: TestWrapper })
      
      await waitFor(() => {
        expect(screen.getByText('Admin User')).toBeInTheDocument()
      })
      
      const createTokenButtons = screen.getAllByText('Create Token')
      await user.click(createTokenButtons[0])
      
      // Check default value
      const expiryInput = screen.getByRole('spinbutton')
      expect(expiryInput).toHaveValue(30)
    })
  })

  describe('Error Handling', () => {
    it('handles user fetch error gracefully', async () => {
      mockApiClient.getUsers.mockRejectedValue(new Error('Failed to fetch users'))
      
      // Suppress console.error for this test
      const consoleErrorSpy = vi.spyOn(console, 'error').mockImplementation(() => {})
      
      render(<Users />, { wrapper: TestWrapper })
      
      // Wait for the error to be handled
      await waitFor(() => {
        // Component should still render title without crashing
        expect(screen.getByText('User Management')).toBeInTheDocument()
      })
      
      consoleErrorSpy.mockRestore()
    })

    it('handles create user error gracefully', async () => {
      const user = userEvent.setup()
      mockApiClient.createUser.mockRejectedValue(new Error('Failed to create user'))
      
      // Suppress console.error for this test
      const consoleErrorSpy = vi.spyOn(console, 'error').mockImplementation(() => {})
      
      render(<Users />, { wrapper: TestWrapper })
      
      await waitFor(() => {
        expect(screen.getByText('Admin User')).toBeInTheDocument()
      })
      
      await user.click(screen.getByText('Add User'))
      await user.type(screen.getByPlaceholderText('name@company.com'), 'test@example.com')
      await user.type(screen.getByPlaceholderText('John Doe'), 'Test User')
      await user.click(screen.getByRole('button', { name: 'Create User' }))
      
      // Modal should remain open on error
      await waitFor(() => {
        expect(screen.getByText('Create New User')).toBeInTheDocument()
      })
      
      consoleErrorSpy.mockRestore()
    })

    it('handles create token error gracefully', async () => {
      const user = userEvent.setup()
      mockApiClient.createToken.mockRejectedValue(new Error('Failed to create token'))
      
      // Suppress console.error for this test
      const consoleErrorSpy = vi.spyOn(console, 'error').mockImplementation(() => {})
      
      render(<Users />, { wrapper: TestWrapper })
      
      await waitFor(() => {
        expect(screen.getByText('Admin User')).toBeInTheDocument()
      })
      
      const createTokenButtons = screen.getAllByText('Create Token')
      await user.click(createTokenButtons[0])
      await user.type(screen.getByPlaceholderText('e.g., Mobile app token'), 'Test Token')
      await user.click(screen.getByRole('button', { name: 'Create Token' }))
      
      // Modal should remain open on error
      await waitFor(() => {
        expect(screen.getByText('Create API Token')).toBeInTheDocument()
      })
      
      consoleErrorSpy.mockRestore()
    })
  })

  describe('Empty State', () => {
    it('handles empty user list', async () => {
      mockApiClient.getUsers.mockResolvedValue([])
      
      render(<Users />, { wrapper: TestWrapper })
      
      await waitFor(() => {
        // Should still show the header and add button
        expect(screen.getByText('User Management')).toBeInTheDocument()
        expect(screen.getByText('Add User')).toBeInTheDocument()
      })
      
      // Should not show any user cards
      expect(screen.queryByText('Create Token')).not.toBeInTheDocument()
    })
  })

  describe('Form Validation', () => {
    it('requires all fields in create user form', async () => {
      const user = userEvent.setup()
      render(<Users />, { wrapper: TestWrapper })
      
      await waitFor(() => {
        expect(screen.getByText('Admin User')).toBeInTheDocument()
      })
      
      await user.click(screen.getByText('Add User'))
      
      const emailInput = screen.getByPlaceholderText('name@company.com')
      const nameInput = screen.getByPlaceholderText('John Doe')
      
      // Check required attributes
      expect(emailInput).toHaveAttribute('required')
      expect(nameInput).toHaveAttribute('required')
      expect(screen.getByRole('combobox')).toHaveAttribute('required')
    })

    it('validates email format in create user form', async () => {
      const user = userEvent.setup()
      render(<Users />, { wrapper: TestWrapper })
      
      await waitFor(() => {
        expect(screen.getByText('Admin User')).toBeInTheDocument()
      })
      
      await user.click(screen.getByText('Add User'))
      
      const emailInput = screen.getByPlaceholderText('name@company.com')
      expect(emailInput).toHaveAttribute('type', 'email')
    })

    it('validates expiry days range in create token form', async () => {
      const user = userEvent.setup()
      render(<Users />, { wrapper: TestWrapper })
      
      await waitFor(() => {
        expect(screen.getByText('Admin User')).toBeInTheDocument()
      })
      
      const createTokenButtons = screen.getAllByText('Create Token')
      await user.click(createTokenButtons[0])
      
      const expiryInput = screen.getByRole('spinbutton')
      expect(expiryInput).toHaveAttribute('min', '1')
      expect(expiryInput).toHaveAttribute('max', '365')
      expect(expiryInput).toHaveAttribute('type', 'number')
    })
  })
})