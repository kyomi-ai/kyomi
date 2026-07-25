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
-- different way — see the Postgres migration
-- (20260725010000_purge_invalid_push_endpoints.sql) for the full list
-- (path-smuggling, query/fragment-smuggling, and backslash-as-slash).
-- WHATWG also strips ASCII tab/newline and trims control characters
-- anywhere in the input before parsing — behaviour this SQL does not
-- attempt to replicate. Do not extend this migration chasing further
-- delimiter-equivalence with `url::Url`; that is what the Rust sweep is for.
--
-- Host extraction is bounded at the first of "/", "?", "#", or "\" — SQLite
-- has no regex, so the CTEs below compute all four candidate positions with
-- instr() and take the minimum. "Not found" is represented per-row as
-- `length(rest) + 1` (one past the end of the remaining string) rather than
-- a fixed sentinel like 1000000: the fixed-sentinel version implicitly
-- assumed every endpoint was under the 2048-char ingress cap, which by
-- definition doesn't hold for the pre-existing, unvalidated rows this
-- migration exists to clean up. With a per-row sentinel, when none of the
-- four delimiters are present, `end_pos = length(rest) + 1` and
-- `substr(rest, 1, end_pos - 1)` naturally evaluates to the full `rest`
-- string (substr clamps rather than erroring past the string's length), so
-- no separate CASE/ELSE branch is needed.
--
-- Embedded credentials (userinfo, e.g. `https://user@fcm.googleapis.com/x`)
-- are handled by the separate `endpoint_lower LIKE 'https://%@%'` condition
-- below, which matches on the raw endpoint rather than the extracted
-- authority — so it doesn't matter whether the `@` lands before or after
-- the real host; either position is rejected.
WITH endpoint_rest AS (
    SELECT
        id,
        LOWER(endpoint) AS endpoint_lower,
        -- Everything after "https://" (8 chars, so start at position 9).
        substr(LOWER(endpoint), 9) AS rest
    FROM push_subscriptions
),
endpoint_bounds AS (
    SELECT
        id,
        endpoint_lower,
        rest,
        -- Position of the first authority-terminating character in `rest`
        -- ("/", "?", "#", or "\"), or one-past-the-end of `rest` if none of
        -- the four appear at all.
        MIN(
            CASE WHEN instr(rest, '/') > 0 THEN instr(rest, '/') ELSE length(rest) + 1 END,
            CASE WHEN instr(rest, '?') > 0 THEN instr(rest, '?') ELSE length(rest) + 1 END,
            CASE WHEN instr(rest, '#') > 0 THEN instr(rest, '#') ELSE length(rest) + 1 END,
            CASE WHEN instr(rest, '\') > 0 THEN instr(rest, '\') ELSE length(rest) + 1 END
        ) AS end_pos
    FROM endpoint_rest
),
endpoint_hosts AS (
    SELECT
        id,
        endpoint_lower,
        substr(rest, 1, end_pos - 1) AS host
    FROM endpoint_bounds
)
DELETE FROM push_subscriptions
WHERE id IN (
    SELECT id FROM endpoint_hosts
    WHERE
        endpoint_lower NOT LIKE 'https://%'
        OR endpoint_lower LIKE 'https://%@%'
        OR NOT (
            host = 'googleapis.com'
            OR host LIKE '%.googleapis.com'
            OR host = 'push.services.mozilla.com'
            OR host LIKE '%.push.services.mozilla.com'
            OR host = 'notify.windows.com'
            OR host LIKE '%.notify.windows.com'
            OR host = 'push.apple.com'
            OR host LIKE '%.push.apple.com'
        )
);
