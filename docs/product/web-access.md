# Web access

Kyomi's chat agent can search the public web and read public web pages (for news, documentation, definitions and market context) when an administrator has enabled it. The agent uses these tools only for external information; numbers about your workspace always come from queries against your datasources.

Web access is **off until configured**: there is no default backend. It is available in chat only, not in watches or scheduled runs, unless the administrator opts in. It is an internal capability of the Kyomi agent and is **not** exposed through the [MCP server](/docs/mcp).

## What leaves Kyomi

Search queries and the URLs the agent fetches are sent to the configured provider (and, depending on the provider, to its underlying search engine). Kyomi instructs the agent never to put datasource values, customer or personal data, credentials or internal identifiers in a query or URL, but this is an instruction to the model, not a filter: do not enable web access where that is unacceptable. A conversation that holds restricted personal data from an external MCP connection cannot use web access at all.

Web content is treated as untrusted data. The agent is told not to follow instructions found in it, and nothing in a web page can run a query or change anything without the normal checks. For backends where the model provider itself runs the search (`anthropic`, `openrouter`), Kyomi cannot wrap the content the way it does for the other backends; the same instruction applies.

## Sources

Answers that used the web show the sources (title and link). Each search or fetch appears in the conversation with the pages it used.

## Backends

Set the backend for search and for fetch separately.

| Value | Runs | Notes |
|---|---|---|
| `tavily`, `linkup` | Kyomi calls the vendor's API | Needs `WEB_SEARCH_API_KEY` / `WEB_FETCH_API_KEY`. Works with any LLM provider. |
| `builtin` (fetch only) | Kyomi fetches the page itself | Public internet addresses only; private, loopback and cloud-metadata addresses, redirects to them and non-HTTP(S) URLs are refused. |
| `anthropic` | Anthropic runs the tool | Requires `LLM_PROVIDER=anthropic`. |
| `openrouter` | OpenRouter runs the tool | Requires an `openrouter.ai` `LLM_BASE_URL`. |
| `none` | | Disabled. |

An unsupported combination (for example `anthropic` with `LLM_PROVIDER=openai`) stops the server at startup with a message naming the variables. `*_OPTIONS` takes a JSON object that is passed to the backend unchanged (engine, domain lists, `max_uses`, ...).

## Limits

Per conversation turn: 5 searches and 5 fetches. Per month: 200 calls per workspace and 1,000 across the instance, for each tool. At a limit the agent is told web access is unavailable and carries on without it. All are environment-configurable (see `deploy/.env.example`).

A workspace can use its own backend and key through its workspace settings (`custom_settings.web_search` / `custom_settings.web_fetch`); there is no settings page for it yet.
