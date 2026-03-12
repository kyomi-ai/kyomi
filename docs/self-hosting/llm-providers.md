# LLM Provider Setup

Kyomi uses a bring-your-own-key (BYOK) model for AI features. You provide an API key from the LLM provider of your choice, and Kyomi sends requests directly to that provider. No AI data passes through Kyomi's servers.

## Configuration

Two environment variables are required:

| Variable | Description |
|----------|-------------|
| `LLM_PROVIDER` | Provider name: `anthropic`, `openai`, or `gemini` |
| `LLM_API_KEY` | Your API key for the chosen provider |

Two optional variables for advanced configuration:

| Variable | Description |
|----------|-------------|
| `LLM_MODEL` | Override the default model (provider-specific) |
| `LLM_BASE_URL` | Custom API endpoint URL (for Ollama, Azure OpenAI, etc.) |

## Supported Providers

### Anthropic (Claude)

Claude is the provider Kyomi is optimized for. Tool use (the mechanism Kyomi uses to query your data, create charts, and manage dashboards) works best with Claude models.

1. Sign up at [console.anthropic.com](https://console.anthropic.com)
2. Go to **API Keys** and create a new key
3. Configure Kyomi:

```env
LLM_PROVIDER=anthropic
LLM_API_KEY=sk-ant-api03-...
```

**Default model:** `claude-haiku-4-5-20251001` (fast, cost-effective)

To use a higher-quality model:

```env
LLM_MODEL=claude-sonnet-4-20250514
```

Available models (in order of capability):
- `claude-haiku-4-5-20251001` -- fast, lowest cost (default)
- `claude-sonnet-4-20250514` -- balanced quality and speed
- `claude-opus-4-20250514` -- highest quality, slowest

**Legacy shortcut:** If you set `ANTHROPIC_API_KEY` without `LLM_PROVIDER`, Kyomi will automatically use Anthropic. This exists for backwards compatibility; prefer the explicit `LLM_PROVIDER` + `LLM_API_KEY` form.

---

### OpenAI (GPT)

1. Sign up at [platform.openai.com](https://platform.openai.com)
2. Go to **API Keys** and create a new key
3. Configure Kyomi:

```env
LLM_PROVIDER=openai
LLM_API_KEY=sk-...
```

**Default model:** `gpt-4o-mini`

To use a more capable model:

```env
LLM_MODEL=gpt-4o
```

---

### Google Gemini

1. Get an API key from [ai.google.dev](https://ai.google.dev)
2. Configure Kyomi:

```env
LLM_PROVIDER=gemini
LLM_API_KEY=your-gemini-api-key
```

**Default model:** `gemini-2.5-flash`

---

### Ollama (Local / Self-Hosted LLM)

Ollama lets you run LLMs entirely on your own hardware -- no API key leaves your network. Kyomi connects to Ollama through its OpenAI-compatible API.

1. Install Ollama from [ollama.com](https://ollama.com)
2. Pull a model:

```bash
ollama pull llama3.1
```

3. Configure Kyomi:

```env
LLM_PROVIDER=openai
LLM_BASE_URL=http://host.docker.internal:11434/v1
LLM_API_KEY=ollama
LLM_MODEL=llama3.1
```

**Important notes:**

- `LLM_PROVIDER` is set to `openai` because Ollama exposes an OpenAI-compatible API.
- `LLM_API_KEY` must be set to any non-empty value (Ollama does not require a real key, but Kyomi requires the variable to be present). Use `ollama` as a placeholder.
- `host.docker.internal` resolves to the host machine from inside Docker containers. If you are running Kyomi outside Docker, use `http://localhost:11434/v1` instead.
- Tool use quality varies significantly by model. Larger models (70B+) produce better results for data analysis tasks. Smaller models may struggle with complex multi-step tool use.

---

### Other OpenAI-Compatible APIs

Any service that exposes an OpenAI-compatible chat completions API works with Kyomi. Set `LLM_PROVIDER=openai` and point `LLM_BASE_URL` at the service.

#### vLLM

```env
LLM_PROVIDER=openai
LLM_BASE_URL=http://your-vllm-server:8000/v1
LLM_API_KEY=not-needed
LLM_MODEL=meta-llama/Meta-Llama-3.1-70B-Instruct
```

#### LiteLLM

```env
LLM_PROVIDER=openai
LLM_BASE_URL=http://your-litellm-proxy:4000/v1
LLM_API_KEY=your-litellm-key
LLM_MODEL=your-model-name
```

#### Azure OpenAI

```env
LLM_PROVIDER=openai
LLM_BASE_URL=https://your-resource.openai.azure.com/openai/deployments/your-deployment
LLM_API_KEY=your-azure-api-key
LLM_MODEL=gpt-4o
```

---

## Model Quality Expectations

Kyomi relies heavily on tool use (function calling) for its AI features. The model must be able to:

- Decide which tools to call based on the user's question
- Write correct SQL queries for your data warehouse
- Interpret query results and produce useful analysis
- Generate ChartML specifications for data visualization

**Recommended minimum:** A model with strong tool-use support. Claude Sonnet, GPT-4o, and Gemini 2.5 Flash all perform well. Smaller or older models may produce errors during multi-step analysis workflows.

## Verifying Your Configuration

After starting Kyomi, check the health endpoint:

```bash
curl http://localhost:8080/api/health
```

The response includes an `llm` field. If it shows the provider name and model, your LLM is configured correctly. If it shows `"not configured"`, check that `LLM_PROVIDER` and `LLM_API_KEY` are set in your environment.

You can also verify by starting a chat conversation in the Kyomi UI. If the AI responds, your provider is working.
