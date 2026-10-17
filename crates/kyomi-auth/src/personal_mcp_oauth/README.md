# Outgoing personal MCP OAuth

`OAuthService` is the shared orchestration boundary for browser/provider routes
and server-function callers. It does not use datasource providers, inbound OAuth
clients, login grants or Kyomi access tokens. Route construction enforces the
default-off personal MCP feature flag. Other trusted callers must enforce the
same `personal_mcp_service::require_feature` gate before calling the service.

Every HTTP operation uses the bounded `kyomi-mcp-client` safe transport in
production, including protected-resource discovery, authorization-server
metadata, dynamic registration and token exchange. A logical operation has a
30-second total deadline. There is no automatic code-exchange retry, metadata
redirect following, scope escalation, protocol fallback or provider canary.
Injected transports are for deterministic tests or equivalent trusted adapters.

The deployment's `frontend_url` must be an HTTPS origin. The callback URI is
fixed at `/api/v1/personal-mcp/oauth/callback`; requests cannot supply it. The
owned client metadata document is served at
`/api/v1/personal-mcp/oauth/client-metadata.json`. An operator may configure
`PERSONAL_MCP_OAUTH_CLIENTS_JSON` as a map keyed by **exact issuer**, with
`client_id`, optional `client_secret`, and `token_endpoint_auth_method` (`none`,
`client_secret_post`, or `client_secret_basic`). This deployment-only input is
never a browser DTO, Debug field, HTTP response or error message.

Client selection prioritizes exact-issuer operator registration, then advertised
client metadata documents, then advertised DCR. The adapter follows RFC9728
resource metadata and RFC8414/OIDC discovery, including issuers containing path
components. Resource and issuer equality use the original advertised identifiers,
not a suffix or normalized-host comparison.

A shared callback requires advertised RFC9207 issuer-response support and an
exact `iss` on every success/error callback, checked before state consumption or
code exchange. Issuer-less providers receive an actionable unsupported result.
There is no fallback based only on state, PKCE, or pre-registration. This follows
RFC9700 sections 2.1 and 4.4.2 (issuer identification); issuer-distinct redirect registration is not
implemented. In the attack this prevents, malicious issuer A advertises honest
issuer H's authorization endpoint but A's token endpoint. H issues a code for
the stored PKCE challenge. Accepting H's issuer-less response using only the
A-bound state would send both H's code and the verifier to A, which could redeem
them at H. Mandatory response issuer comparison rejects both omitted `iss` and
H's `iss` before any exchange to A. Both database regressions exercise this
endpoint pairing, including A lying about issuer-response support.

## Trakkt acceptance dependency

**Integrated Trakkt acceptance is blocked by upstream conformance work.** Local
read-only source was inspected at Trakkt commit
`9efb657cf4dc1b7708c99d597522463900302125`:

- `/home/jason/repos/trakkt/apps/server/src/routes/mcp.rs:121` advertises the root
  protected-resource metadata URL from `/mcp`. Upstream must point that challenge
  to `/.well-known/oauth-protected-resource/mcp`, whose resource is the exact
  requested `/mcp` endpoint.
- `/home/jason/repos/trakkt/apps/server/src/routes/oauth.rs:152` returns the base
  origin as `resource` there; the path-specific handler at line 174 returns
  `base/path`. RFC9728 section 3.3 requires the challenge-discovered resource to
  match the requested `/mcp` resource. Kyomi rejects the current root response
  rather than accepting arbitrary parent or sibling resources.
- The same `oauth.rs:105` authorization metadata omits
  `authorization_response_iss_parameter_supported`. Even with the resource
  challenge corrected, Kyomi rejects this issuer-less shared-callback contract
  until Trakkt returns the exact issuer on both `/api/v1/oauth/authorize` and
  `/api/v1/oauth/authorize/continue` authorization redirects (including OAuth
  error responses). Only once both paths implement that contract should its
  metadata advertise `authorization_response_iss_parameter_supported: true`.
- `oauth.rs:865` returns a DCR response without
  `token_endpoint_auth_method`. This public-client response shape is supported
  for the explicitly requested and advertised `none` method, but does not cure
  either discovery or mix-up-defense blocker above.

The deterministic Trakkt-shaped fixture preserves those actual omissions and
root/path distinctions with synthetic hosts. It proves safe rejection on both
databases, then separately proves issuer-support rejection after only the
resource challenge is corrected. The successful generic fixture represents a
conforming issuer, not the current Trakkt implementation. Independent review's
public unauthenticated GETs returned Cloudflare HTTP403/error1010; no live
metadata payload was observed, and no authenticated/live canary was performed.
Upstream Trakkt changes and tracking are outside this ticket and must complete
before integrated acceptance can be claimed.

Discovery returns the offered scopes and selected issuer for consent. Starting
requires explicit consent and an exact issuer/generation match; selected scopes
must be a subset of those offered. An empty selection cannot silently ask for a
provider's default scopes when metadata offers specific scopes. Scope failures
are typed by `authorization_required` into reconnect/step-up states; they never
start a new flow or expand a grant automatically.

Starting reconnect advances connection generation by CAS before storing new
state. State is 32 random bytes, stored only as a hash. Its encrypted persisted
payload contains PKCE material/client credentials and binds owner, connection,
generation, canonical endpoint, exact issuer/resource and fixed redirect URI.
State expires after ten minutes and is atomically consumed before exchanging a
code. A consumed callback cannot commit after another reconnect, edit or
disconnect advances generation. Network operations never hold database locks.
Wrong users/issuers cannot consume the owner's valid state. Callback errors omit
provider prose; success redirects only to a validated relative return path.

OAuth credentials are opaque encrypted JSON in the existing secret store:
access/optional refresh token, exact issuer/resource, token endpoint and client
authentication, consented scopes, optional expiry, and random account revision.
`authorization_from_secret` and `account_revision` are trusted dispatch adapters;
call them only after live owner/workspace/generation checks and
`load_workspace_secret`. They return no browser-safe credential DTO. Refresh and
later dispatch lifecycle are owned by KYO-931; expired credentials request
reconnect instead of attempting a refresh or replay here.

The cross-site provider callback GET renders a scriptless, escaped same-origin
completion form. This preserves Kyomi's `SameSite=Strict` session cookies: the
subsequent POST authenticates the current browser user and requires the exact
same-origin Origin header before invoking the callback service. It never stores
code/state in browser storage, relaxes login cookies, executes inline script, or
exchanges a code before authentication. Its exact GET path receives the existing
scriptless OAuth CSP with `form-action 'self'`. Route contracts exercise the real
security-header middleware and cookie authentication, including malicious markup.

HTTP responses, including extraction/authentication errors, use `no-store` and
`no-referrer`. Global HTTP tracing records method and path only; callback code,
state, authorization headers and bodies are omitted. The OAuth types and
credential-bearing service/configuration have no Debug implementation.

Verification is split into persisted-flow SQLite/Postgres parity, bounded
metadata/challenge/egress tests, real production-handler start/callback contract
tests on both databases, and the installed global HTTP span's redaction test.
Postgres is required, never silently skipped. Existing CI's workspace `--lib
--bins --tests` sweep includes these tests without a browser or compiled UI.

Primary references:
- https://modelcontextprotocol.io/specification/2026-07-28/basic/authorization
- https://modelcontextprotocol.io/specification/2026-07-28/basic/authorization/authorization-server-discovery
- https://modelcontextprotocol.io/specification/2026-07-28/basic/authorization/client-registration
- https://www.rfc-editor.org/rfc/rfc9728.html
- https://www.rfc-editor.org/rfc/rfc8414.html

- https://www.rfc-editor.org/rfc/rfc9700.html#section-4.4.2
- https://www.rfc-editor.org/rfc/rfc9207.html
