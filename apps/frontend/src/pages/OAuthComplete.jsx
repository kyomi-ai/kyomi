// SPDX-License-Identifier: AGPL-3.0-or-later
/**
 * OAuth Complete Page
 *
 * Shown after MCP OAuth flow completes. The user can close this tab/window.
 */
export default function OAuthComplete() {
  const handleClose = () => {
    // window.close() only works on windows opened by JavaScript
    // For regular tabs, it won't work - that's a browser security feature
    window.close();
  };

  return (
    <div className="min-h-screen bg-background flex items-center justify-center p-8">
      <div className="text-center max-w-md">
        <img src="/kyomi_oauth_logo.png" alt="Kyomi" className="h-24 mx-auto mb-6" />
        <h1 className="text-2xl font-bold text-foreground mb-2">Authentication Complete</h1>
        <p className="text-muted-foreground mb-6">
          You've successfully connected to Kyomi. You can close this tab and return to your application.
        </p>
        <button
          onClick={handleClose}
          className="px-6 py-3 bg-primary text-white font-semibold rounded-xl hover:opacity-90 transition-opacity"
        >
          Close Tab
        </button>
        <p className="text-sm text-muted-foreground mt-3">
          If the button doesn't work, you can manually close this tab.
        </p>
      </div>
    </div>
  );
}
