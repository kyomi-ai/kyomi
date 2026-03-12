// SPDX-License-Identifier: AGPL-3.0-or-later
import { BrowserRouter as Router, Routes, Route, Navigate, useLocation } from 'react-router-dom';
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { Toaster } from 'sonner';
import Login from './pages/Login';
import Unsubscribe from './pages/Unsubscribe';
import VerifyEmail from './pages/VerifyEmail';
import PasskeySignupComplete from './pages/PasskeySignupComplete';
import SignupComplete from './pages/SignupComplete';
import PasskeyRecovery from './pages/PasskeyRecovery';
import PasskeyRecoveryComplete from './pages/PasskeyRecoveryComplete';
import AccountRecovery from './pages/AccountRecovery';
import AccountRecoveryComplete from './pages/AccountRecoveryComplete';
import { Welcome } from './pages/Welcome';
import Chat from './pages/Chat';
import LandingRedirect from './components/LandingRedirect';
import ChatsList from './pages/ChatsList';
import DashboardEditor from './pages/DashboardEditor';
import DashboardsList from './pages/DashboardsList';
import DashboardViewer from './pages/DashboardViewer';
import SQLEditorPage from './pages/SQLEditorPage';
import Knowledge from './pages/Knowledge';
import WatchesPage from './pages/WatchesPage';
import SettingsPage from './pages/SettingsPage';
import GoogleLinkCallback from './pages/GoogleLinkCallback';
import GoogleLoginCallback from './pages/GoogleLoginCallback';
import OAuthCallback from './pages/OAuthCallback';
import OAuthComplete from './pages/OAuthComplete';
import SlackConnectCallback from './pages/SlackConnectCallback';
import ChartTestPage from './pages/ChartTestPage';
// import StyleGuide from './pages/StyleGuide'; // Excluded from production build
import AcceptOwnershipPage from './pages/AcceptOwnershipPage';
import Try from './pages/Try';
import ConnectSetupPage from './pages/ConnectSetupPage';
import DatasourceOnboarding from './pages/DatasourceOnboarding';
import { AuthProvider, useAuth } from './context/AuthContext';
import { ThemeProvider, useTheme } from './context/ThemeContext';
import { WebSocketProvider } from './context/WebSocketContext';
import { CapabilitiesProvider } from './context/CapabilitiesContext';
import { SystemConfigProvider } from './context/SystemConfigContext';
import { SidebarProvider } from './components/Sidebar';
import AppWithOAuthBar from './components/AppWithOAuthBar';
import PWAUpdatePrompt from './components/PWAUpdatePrompt';
import FeedbackWrapper from './components/feedback/FeedbackWrapper';

function ThemedToaster() {
  const { resolvedTheme } = useTheme();
  return <Toaster position="top-right" expand={false} richColors closeButton theme={resolvedTheme} />;
}

// Create a client
const queryClient = new QueryClient({
  defaultOptions: {
    queries: {
      retry: 1,
      staleTime: 5 * 60 * 1000, // 5 minutes
    },
  },
});

function ProtectedRoute({ children }) {
  const { isAuthenticated, loading } = useAuth();
  const location = useLocation();

  if (loading) {
    return <div className="min-h-screen flex items-center justify-center">Loading...</div>;
  }

  if (!isAuthenticated) {
    // Save the location they were trying to access so we can redirect back after login
    return <Navigate to="/login" state={{ from: location }} replace />;
  }

  return children;
}

function App() {
  return (
    <ThemeProvider>
    <SystemConfigProvider>
    <div className="font-sans">
      <ThemedToaster />
      <PWAUpdatePrompt />
      <QueryClientProvider client={queryClient}>
        <Router>
          <Routes>
            {/* OAuth callback routes - outside AuthProvider to prevent race conditions */}
            <Route path="/auth/google/callback" element={<GoogleLoginCallback />} />

            {/* Public routes - auth context but no sidebar */}
            <Route path="/login" element={
              <AuthProvider>
                <Login />
              </AuthProvider>
            } />
            <Route path="/verify-email" element={
              <AuthProvider>
                <VerifyEmail />
              </AuthProvider>
            } />
            <Route path="/verify" element={
              <AuthProvider>
                <VerifyEmail />
              </AuthProvider>
            } />
            <Route path="/oauth-complete" element={<OAuthComplete />} />
            {/* Signup routes */}
            <Route path="/signup/complete" element={
              <AuthProvider>
                <SignupComplete />
              </AuthProvider>
            } />
            <Route path="/auth/passkey-signup" element={
              <AuthProvider>
                <PasskeySignupComplete />
              </AuthProvider>
            } />
            <Route path="/auth/recover-passkey" element={
              <AuthProvider>
                <PasskeyRecovery />
              </AuthProvider>
            } />
            <Route path="/auth/recover-passkey/complete" element={
              <AuthProvider>
                <PasskeyRecoveryComplete />
              </AuthProvider>
            } />
            {/* Account recovery routes (password reset) */}
            <Route path="/account/recover" element={
              <AuthProvider>
                <AccountRecovery />
              </AuthProvider>
            } />
            <Route path="/account/recover/complete" element={
              <AuthProvider>
                <AccountRecoveryComplete />
              </AuthProvider>
            } />
            <Route path="/unsubscribe" element={<Unsubscribe />} />
            <Route path="/welcome" element={
              <AuthProvider>
                <Welcome />
              </AuthProvider>
            } />

            {/* Test pages - no auth required */}
            <Route path="/test/charts" element={<ChartTestPage />} />
            {/* <Route path="/styleguide" element={<StyleGuide />} /> */} {/* Excluded from production */}

            {/* Try page - anonymous trial experience */}
            <Route path="/try" element={<Try />} />

            <Route path="/auth/google/link-callback" element={
              <AuthProvider>
                <ProtectedRoute>
                  <GoogleLinkCallback />
                </ProtectedRoute>
              </AuthProvider>
            } />

            {/* Generic OAuth callback route - works with any registered provider */}
            <Route path="/auth/oauth/:provider/callback" element={
              <AuthProvider>
                <ProtectedRoute>
                  <OAuthCallback />
                </ProtectedRoute>
              </AuthProvider>
            } />

            {/* Slack connect callback from /kyomi connect command */}
            <Route path="/auth/slack-connect" element={
              <AuthProvider>
                <ProtectedRoute>
                  <SlackConnectCallback />
                </ProtectedRoute>
              </AuthProvider>
            } />

            <Route path="/accept-ownership/:transferId" element={
              <AuthProvider>
                <ProtectedRoute>
                  <AcceptOwnershipPage />
                </ProtectedRoute>
              </AuthProvider>
            } />

            {/* Connect setup - full screen, no sidebar (launched by kyomi-connect CLI) */}
            <Route path="/connect/setup" element={
              <AuthProvider>
                <ProtectedRoute>
                  <ConnectSetupPage />
                </ProtectedRoute>
              </AuthProvider>
            } />

            {/* Datasource onboarding - full screen, no sidebar */}
            <Route path="/onboarding" element={
              <AuthProvider>
                <ProtectedRoute>
                  <DatasourceOnboarding />
                </ProtectedRoute>
              </AuthProvider>
            } />

            {/* Legacy catalog onboarding route - redirect to new onboarding */}
            <Route path="/onboarding/catalog" element={<Navigate to="/onboarding" replace />} />

            {/* All other routes wrapped in auth context with sidebar */}
            <Route path="/*" element={
              <AuthProvider>
                <CapabilitiesProvider>
                  <WebSocketProvider>
                    <SidebarProvider>
                      <AppWithOAuthBar>
                      <FeedbackWrapper>
                      <Routes>
            <Route path="/" element={
              <ProtectedRoute>
                <LandingRedirect />
              </ProtectedRoute>
            } />
            <Route path="/chat" element={
              <ProtectedRoute>
                <Chat />
              </ProtectedRoute>
            } />
            <Route path="/chat/:sessionId" element={
              <ProtectedRoute>
                <Chat />
              </ProtectedRoute>
            } />
            <Route path="/chats" element={
              <ProtectedRoute>
                <ChatsList />
              </ProtectedRoute>
            } />
            <Route path="/settings" element={
              <ProtectedRoute>
                <SettingsPage />
              </ProtectedRoute>
            } />
            <Route path="/settings/:tab" element={
              <ProtectedRoute>
                <SettingsPage />
              </ProtectedRoute>
            } />
            <Route path="/dashboards" element={
              <ProtectedRoute>
                <DashboardsList />
              </ProtectedRoute>
            } />
            <Route path="/dashboard/:dashboardId/edit" element={
              <ProtectedRoute>
                <DashboardEditor />
              </ProtectedRoute>
            } />
            <Route path="/dashboard/:dashboardId" element={
              <ProtectedRoute>
                <DashboardViewer />
              </ProtectedRoute>
            } />
            <Route path="/sql-editor" element={
              <ProtectedRoute>
                <SQLEditorPage />
              </ProtectedRoute>
            } />
            <Route path="/knowledge" element={
              <ProtectedRoute>
                <Knowledge />
              </ProtectedRoute>
            } />
            <Route path="/watches" element={
              <ProtectedRoute>
                <WatchesPage />
              </ProtectedRoute>
            } />
            <Route path="/watches/:view" element={
              <ProtectedRoute>
                <WatchesPage />
              </ProtectedRoute>
            } />
                      </Routes>
                      </FeedbackWrapper>
                      </AppWithOAuthBar>
                    </SidebarProvider>
                  </WebSocketProvider>
                </CapabilitiesProvider>
              </AuthProvider>
            } />
          </Routes>
        </Router>
      </QueryClientProvider>
    </div>
    </SystemConfigProvider>
    </ThemeProvider>
  );
}

export default App;