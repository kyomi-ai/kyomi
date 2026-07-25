-- SPDX-License-Identifier: AGPL-3.0-or-later
--
-- KYO-219: any authenticated user could register an arbitrary `endpoint` URL
-- via POST /api/v1/push/subscribe with no scheme/host validation. The server
-- later POSTs to that URL carrying a VAPID-signed Authorization header, so an
-- unvalidated stored endpoint is a live SSRF + credential-leak primitive that
-- fires asynchronously from the notification scheduler.
--
-- ============================================================================
-- THIS MIGRATION IS A COARSE BEST-EFFORT FIRST PASS. IT IS NOT AUTHORITATIVE.
-- ============================================================================
--
-- The authoritative purge is the Rust startup sweep,
-- `kyomi_auth::push_service::purge_invalid_subscriptions`, run once at every
-- boot from `apps/server/src/main.rs` right after this migration applies. It
-- calls `validate_push_endpoint` directly — the exact predicate enforced at
-- subscribe-time and send-time — so a row survives it iff it would be
-- accepted by a fresh subscribe call today. This SQL exists only because the
-- ticket's acceptance criteria require the purge to happen via migration in
-- both dialects; treat it as a rough net cast before the Rust sweep, not as
-- a formally-equivalent reimplementation of `url::Url`.
--
-- That distinction is not academic. `url::Url` implements the WHATWG URL
-- Standard, not RFC 3986, and hand-written SQL has been proven wrong against
-- it three separate times across three rounds of review, each caught a
-- different way:
--   1. `endpoint ILIKE 'https://%.googleapis.com/%'` matched the suffix
--      ANYWHERE in the string, including the path — so
--      `https://attacker.example/.googleapis.com/x` (real host:
--      attacker.example) survived.
--   2. `substring(... from '^https://([^/]+)')` bounded the authority only
--      at `/`, so a URL with no `/` but a crafted query or fragment survived
--      instead: `https://evil.com?x=y.googleapis.com` and
--      `https://evil.com#y.googleapis.com` both extracted a "host" that
--      still ended in `.googleapis.com`.
--   3. WHATWG (unlike RFC 3986) treats `\` as equivalent to `/` for special
--      schemes including `https` — a legacy IE quirk with no RFC 3986
--      analog. `substring(... from '^https://([^/?#]+)')` didn't stop at
--      `\`, so `https://evil.com\.googleapis.com/x` (real host per
--      `url::Url`: evil.com) extracted "evil.com\.googleapis.com" and
--      survived.
-- WHATWG also strips ASCII tab/newline and trims control characters
-- anywhere in the input before parsing — behaviour this SQL does not
-- attempt to replicate. Do not extend this migration chasing further
-- delimiter-equivalence with `url::Url`; that is what the Rust sweep is for.
--
-- Host extraction here is bounded at the first of `/`, `?`, `#`, or `\`
-- (character class `[^/?#\\]`, i.e. the literal characters `/`, `?`, `#`,
-- and one backslash — written as two backslash characters in this string
-- literal because Postgres ARE bracket expressions treat `\` as an escape
-- character, so a single trailing backslash immediately before `]` would
-- instead escape the `]` and unbalance the expression).
--
-- Embedded credentials (userinfo, e.g. `https://user@fcm.googleapis.com/x`)
-- are handled by the separate `x.endpoint ILIKE 'https://%@%'` condition
-- below, which matches on the raw endpoint rather than the extracted
-- authority — so it doesn't matter whether the `@` lands before or after
-- the real host; either position is rejected.
--
-- Caveat: this migration cannot see the self-hosted PUSH_ALLOWED_ENDPOINT_HOSTS
-- env var escape hatch (a one-shot SQL migration has no access to process
-- env), so it will purge a self-hosted deployment's legitimate custom-relay
-- subscriptions too if the env var isn't already set when this migration
-- runs — set it BEFORE upgrading to avoid that one-time loss (see
-- .env.example). The Rust sweep that runs immediately after DOES see the
-- env var, so once configured, custom-relay rows are correctly preserved on
-- every subsequent boot — it just can't restore what this migration already
-- deleted on a run where the env var wasn't set yet.
DELETE FROM push_subscriptions p
USING (
    SELECT
        id,
        endpoint,
        -- Authority component only: everything after "https://" up to (not
        -- including) the first of "/", "?", "#", or "\". NULL if the
        -- endpoint doesn't start with "https://" at all.
        substring(lower(endpoint) from '^https://([^/?#\\]+)') AS host
    FROM push_subscriptions
) x
WHERE p.id = x.id
AND (
    x.endpoint NOT ILIKE 'https://%'
    OR x.endpoint ILIKE 'https://%@%'
    OR x.host IS NULL
    OR NOT (
        x.host = 'googleapis.com'
        OR x.host LIKE '%.googleapis.com'
        OR x.host = 'push.services.mozilla.com'
        OR x.host LIKE '%.push.services.mozilla.com'
        OR x.host = 'notify.windows.com'
        OR x.host LIKE '%.notify.windows.com'
        OR x.host = 'push.apple.com'
        OR x.host LIKE '%.push.apple.com'
    )
);
