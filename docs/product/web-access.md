# Web access

Kyomi's chat agent can search the public web and read public web pages (for news, documentation, definitions and market context) when an administrator has enabled it. The agent uses these tools only for external information; numbers about your workspace always come from queries against your datasources.

Web access is **off until configured**: there is no default backend. It is available in interactive chat only. Watches and scheduled runs can use it only if the administrator opts in; copilots and Slack never do. It is an internal capability of the Kyomi agent and is **not** exposed through the [MCP server](/docs/mcp).

## What leaves Kyomi

Search queries and the URLs the agent fetches are sent to the configured provider (and, depending on the provider, to its underlying search engine). Kyomi instructs the agent never to put datasource values, customer or personal data, credentials or internal identifiers in a query or URL, but this is an instruction to the model, not a filter: do not enable web access where that is unacceptable.

Web content is treated as untrusted data. The agent is told not to follow instructions found in it, and nothing in a web page can run a query or change anything without the normal checks. For backends where the model provider itself runs the search (`anthropic`, `openrouter`), Kyomi cannot wrap the content the way it does for the other backends; the same instruction applies.

## Sources

Answers that used the web show their sources (title and link). Every backend shows each search or fetch as a card in the conversation listing the pages it used. With `tavily`, `linkup` and `builtin` the card comes from the tool call Kyomi runs. With `anthropic` and `openrouter` the provider runs the tool inside one model call, so Kyomi records that call from the provider's reply and shows the same card. Where a conversation has no card timeline (Slack, copilots, watches) the sources are listed under **Sources** at the end of the answer instead. Limits of the providers: Anthropic reports the query and pages for each call; OpenRouter reports citations for search only, as one list for the whole reply and without the query, and reports nothing for a fetch, so an OpenRouter fetch shows no card.

With `anthropic`, the search results the model used are kept with the conversation (encrypted, like the messages) and sent back to the provider exactly as it returned them on later turns, so follow-up questions can build on them; they are removed when the message is deleted or edited. Earlier results count as input tokens like the rest of the history, and very long conversations are compacted as usual. If the provider rejects a request, the conversation shows the provider's error.

## Backends

Set the backend for search and for fetch separately.

| Value | Runs | Notes |
|---|---|---|
| `tavily`, `linkup` | Kyomi calls the vendor's API | Needs `WEB_SEARCH_API_KEY` / `WEB_FETCH_API_KEY`. Works with any LLM provider. |
| `builtin` (fetch only) | Kyomi fetches the page itself | Public internet addresses only; private, loopback and cloud-metadata addresses, redirects to them and non-HTTP(S) URLs are refused. `WEB_FETCH_ALLOWED_DOMAINS` / `WEB_FETCH_BLOCKED_DOMAINS` restrict which domains it may fetch (bare domain names; an invalid entry stops the server at startup; they apply only to `builtin`, and the server warns at startup if they are set with another fetch backend). |
| `anthropic` | Anthropic runs the tool | Requires `LLM_PROVIDER=anthropic`. |
| `openrouter` | OpenRouter runs the tool | Requires an `openrouter.ai` `LLM_BASE_URL`. |
| `none` | | Disabled. |

An unsupported combination (for example `anthropic` with `LLM_PROVIDER=openai`) stops the server at startup with a message naming the variables. A key or base URL set without a provider, or `tavily`/`linkup` set without its API key, is only a startup warning: the server starts and that tool stays unavailable. `*_OPTIONS` takes a JSON object that is passed to the backend unchanged (engine, domain lists, `max_uses`, ...).

## Limits

Per conversation turn: 5 searches and 5 fetches. Per month: 200 calls per workspace and 1,000 across the instance, for each tool. At a limit, a tool Kyomi runs (`tavily`, `linkup`, `builtin`) returns "web access is unavailable" to the agent, which carries on without it. A tool the provider runs (`anthropic`, `openrouter`) is not offered to the model at all from the start of the turn in which a limit is reached; no result is returned and the conversation continues without it. All are environment-configurable (see `deploy/.env.example`); a mistyped value stops the server at startup.

The monthly limits are soft: simultaneous conversations can overshoot slightly, and with `anthropic` / `openrouter` usage is added after the provider reports it. Without Redis the monthly counters are per server instance and reset on restart.

A workspace can use its own backend and key through its workspace settings (`custom_settings.web_search` / `custom_settings.web_fetch`; the key is stored encrypted and a workspace `base_url` is ignored); there is no settings page for it yet. If a workspace's override cannot be read, web access is switched off for that workspace's conversations rather than falling back to the instance backend. The monthly limits still apply to a workspace's own key.
