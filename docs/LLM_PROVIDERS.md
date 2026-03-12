# LLM Provider Configuration

Kyomi supports multiple LLM providers. Self-hosters can use any supported provider by setting environment variables.

## Quick Start

Set two environment variables and restart:

```bash
# Anthropic (default if ANTHROPIC_API_KEY is set)
LLM_PROVIDER=anthropic
LLM_API_KEY=sk-ant-api03-...

# OpenAI
LLM_PROVIDER=openai
LLM_API_KEY=sk-...

# Google Gemini
LLM_PROVIDER=gemini
LLM_API_KEY=AIza...
```

## Environment Variables

| Variable | Required | Description |
|----------|----------|-------------|
| `LLM_PROVIDER` | Yes* | Provider backend: `anthropic`, `openai`, or `gemini` |
| `LLM_API_KEY` | Yes* | API key for the chosen provider |
| `LLM_MODEL` | No | Model override (provider default used if omitted) |
| `LLM_BASE_URL` | No | Custom API endpoint (for proxies, Ollama, OpenRouter, etc.) |

*If `LLM_PROVIDER` and `LLM_API_KEY` are not set, Kyomi falls back to `ANTHROPIC_API_KEY` for backwards compatibility.

## Provider Details

### Anthropic

Default model: `claude-haiku-4-5-20251001`

```bash
LLM_PROVIDER=anthropic
LLM_API_KEY=sk-ant-api03-...
LLM_MODEL=claude-sonnet-4-5-20250929  # optional override
```

Anthropic provides the best tool-use quality. Recommended for production use.

### OpenAI

Default model: `gpt-4o-mini`

```bash
LLM_PROVIDER=openai
LLM_API_KEY=sk-...
LLM_MODEL=gpt-4o  # optional override
```

The OpenAI provider works with any OpenAI-compatible API. This covers a wide range of services and self-hosted models.

### Google Gemini

Default model: `gemini-2.5-flash`

```bash
LLM_PROVIDER=gemini
LLM_API_KEY=AIza...
LLM_MODEL=gemini-2.5-pro  # optional override
```

## OpenAI-Compatible Services

The OpenAI provider supports any service that implements the OpenAI chat completions API. Set `LLM_BASE_URL` to point at the service.

### Ollama (local models)

```bash
LLM_PROVIDER=openai
LLM_API_KEY=ollama            # Ollama doesn't require a real key
LLM_BASE_URL=http://localhost:11434/v1/chat/completions
LLM_MODEL=llama3.1
```

### OpenRouter

```bash
LLM_PROVIDER=openai
LLM_API_KEY=sk-or-v1-...
LLM_BASE_URL=https://openrouter.ai/api/v1/chat/completions
LLM_MODEL=anthropic/claude-sonnet-4-5-20250929
```

### Azure OpenAI

```bash
LLM_PROVIDER=openai
LLM_API_KEY=your-azure-key
LLM_BASE_URL=https://your-resource.openai.azure.com/openai/deployments/your-deployment/chat/completions?api-version=2024-02-01
LLM_MODEL=gpt-4o
```

### Groq

```bash
LLM_PROVIDER=openai
LLM_API_KEY=gsk_...
LLM_BASE_URL=https://api.groq.com/openai/v1/chat/completions
LLM_MODEL=llama-3.3-70b-versatile
```

### Together AI

```bash
LLM_PROVIDER=openai
LLM_API_KEY=your-together-key
LLM_BASE_URL=https://api.together.xyz/v1/chat/completions
LLM_MODEL=meta-llama/Meta-Llama-3.1-70B-Instruct-Turbo
```

## Model Quality Expectations

Kyomi relies heavily on tool use (function calling). Not all models handle this equally well:

| Quality | Models |
|---------|--------|
| Excellent | Claude Sonnet 4.5, Claude Opus 4, GPT-4o, Gemini 2.5 Pro |
| Good | Claude Haiku 4.5, GPT-4o-mini, Gemini 2.5 Flash |
| Variable | Open-source models via Ollama (depends on model size and tool-use training) |

Models that struggle with tool use may produce incorrect SQL, fail to call the right tools, or generate malformed ChartML. Start with a known-good model and experiment from there.

## Backwards Compatibility

Existing deployments using `ANTHROPIC_API_KEY` continue to work without any changes. The new `LLM_*` variables take precedence when both are set.

## Cost Tracking

Each provider includes a built-in pricing table for cost estimation. Costs are tracked per-request and stored in the `api_usage_log` table. The `provider` column records which backend was used (`anthropic`, `openai`, or `gemini`).

For OpenAI-compatible services (Ollama, OpenRouter, etc.), costs are estimated using OpenAI's pricing. Actual costs may differ — check your provider's billing dashboard.
