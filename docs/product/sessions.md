# Sessions and logout

Browser access tokens last 15 minutes. Refresh tokens renew a session and rotate on use.

Password recovery, passkey recovery, and **Log out all devices** immediately invalidate earlier browser session access tokens on their next authenticated request and revoke all refresh tokens. Recovery creates a fresh session after revocation commits; a fresh login also works immediately after logout-all. Logout-all clears the current browser cookies.

Logging out normally or revoking one device revokes that refresh-token family. Its existing access token can remain valid until its 15-minute expiry. Other devices keep their sessions.

Browser tokens carry standard second-resolution `iat` plus signed `session_iat_us` (Unix microseconds). The nullable per-user `sessions_valid_from` cutoff uses the same integer precision on PostgreSQL and SQLite. Tokens at or before the cutoff are rejected. Legacy tokens with only `iat` are conservatively rejected for the entire second containing the event. Users without a cutoff retain existing token behavior. Newly minted sessions use fresh revocation state and an issuance strictly after the cutoff, including repeated revocations or a clock moving backwards.

Revocation and refresh rotation lock the user before modifying refresh tokens, then commit atomically. Rotation rechecks whether the old token is active while holding that lock. A refresh verified before revocation cannot insert a usable replacement afterward; a rotation committed first has both tokens invalidated by revocation. Login refresh-token persistence also rechecks the signed issuance under this lock.

This cutoff applies to browser/user session JWTs, including tokens minted for WebSocket connections and browser session tokens presented to MCP. MCP OAuth access tokens, API tokens, datasource-connect tokens, and restricted passkey recovery tokens retain their own authorization rules. Existing already-authenticated requests and open connections are not forcibly terminated.
