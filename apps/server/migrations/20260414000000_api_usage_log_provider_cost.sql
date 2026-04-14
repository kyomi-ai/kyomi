-- Add `provider_cost_usd` observability column to api_usage_log.
--
-- Semantics:
--   * `cost_estimate`     — the amount billed against Kyomi bundle credits
--                           (always populated for Kyomi-mode rows, 0.0 for BYOK rows).
--   * `provider_cost_usd` — the real upstream provider cost in USD.
--                           NULL for Kyomi-mode rows (cost_estimate IS the cost).
--                           Populated for BYOK rows so we retain observability
--                           into upstream provider spend without affecting billing.
--
-- Matches the `double precision` type used by `cost_estimate`.
ALTER TABLE public.api_usage_log
    ADD COLUMN IF NOT EXISTS provider_cost_usd double precision;

COMMENT ON COLUMN public.api_usage_log.provider_cost_usd IS
    'Real upstream provider USD cost for BYOK rows (observability only). NULL for Kyomi-mode rows where cost_estimate is the cost.';
