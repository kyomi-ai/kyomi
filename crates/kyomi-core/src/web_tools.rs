// SPDX-License-Identifier: AGPL-3.0-or-later

//! Web search / web fetch configuration (KYO-969).
//!
//! The chat agent has two internal tools, `web_search` and `web_fetch`. Each
//! one is backed by a *backend* selected purely by configuration, the same way
//! the LLM provider is (`LLM_PROVIDER`):
//!
//! * `WEB_SEARCH_PROVIDER` / `WEB_FETCH_PROVIDER` (no default: unset = tool absent) — `tavily`, `linkup`,
//!   `builtin` (fetch only: Kyomi's own SSRF-hardened fetcher), `anthropic`,
//!   `openrouter` or `none`.
//! * `WEB_SEARCH_API_KEY` / `WEB_FETCH_API_KEY`, `WEB_SEARCH_BASE_URL` /
//!   `WEB_FETCH_BASE_URL` — for Kyomi-executed backends.
//! * `WEB_SEARCH_OPTIONS` / `WEB_FETCH_OPTIONS` — a JSON object passed through
//!   to the backend untouched (engine names, `max_uses`, domain lists, ...).
//!   Deliberately not a closed set: vendors change these.
//!
//! Two shapes of backend exist. *Kyomi-executed* backends (`tavily`, `linkup`,
//! `builtin`) are run by Kyomi, which applies its own caps and fencing.
//! *Provider-executed* backends (`anthropic`, `openrouter`) add a server tool to
//! the LLM request and the LLM provider runs it; they are only valid with a
//! matching LLM provider, which [`WebToolsSettings::validate`] checks at
//! startup.

use std::fmt;

/// Default for [`WebCaps::per_turn_search`] (owner-confirmed 2026-11-16).
pub const DEFAULT_SEARCHES_PER_TURN: u32 = 5;
/// Default for [`WebCaps::per_turn_fetch`] (owner-confirmed 2026-11-16).
pub const DEFAULT_FETCHES_PER_TURN: u32 = 5;
/// Default for [`WebCaps::workspace_monthly`] (owner-confirmed 2026-11-16).
pub const DEFAULT_WORKSPACE_MONTHLY: u32 = 200;
/// Default for [`WebCaps::global_monthly`] (owner-confirmed 2026-11-16).
pub const DEFAULT_GLOBAL_MONTHLY: u32 = 1000;

/// Which tool a backend serves.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum WebToolKind {
    Search,
    Fetch,
}

impl WebToolKind {
    /// The internal agent tool name.
    pub fn tool_name(self) -> &'static str {
        match self {
            Self::Search => "web_search",
            Self::Fetch => "web_fetch",
        }
    }

    /// Env-var prefix.
    fn env_prefix(self) -> &'static str {
        match self {
            Self::Search => "WEB_SEARCH",
            Self::Fetch => "WEB_FETCH",
        }
    }
}

/// A backend selection.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum WebBackendKind {
    /// Tavily search / extract API (Kyomi-executed).
    Tavily,
    /// Linkup search / fetch API (Kyomi-executed).
    Linkup,
    /// Kyomi's own fetcher (Kyomi-executed; fetch only).
    Builtin,
    /// Anthropic server tools (`LLM_PROVIDER=anthropic` only).
    Anthropic,
    /// OpenRouter server tools (OpenRouter base URL only).
    OpenRouter,
    /// Tool disabled.
    None,
}

impl fmt::Display for WebBackendKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Tavily => "tavily",
            Self::Linkup => "linkup",
            Self::Builtin => "builtin",
            Self::Anthropic => "anthropic",
            Self::OpenRouter => "openrouter",
            Self::None => "none",
        })
    }
}

impl WebBackendKind {
    /// Parse a backend name (case-insensitive) for the given tool.
    pub fn parse(tool: WebToolKind, s: &str) -> crate::Result<Self> {
        let kind = match s.trim().to_lowercase().as_str() {
            "tavily" => Self::Tavily,
            "linkup" => Self::Linkup,
            "builtin" => Self::Builtin,
            "anthropic" => Self::Anthropic,
            "openrouter" => Self::OpenRouter,
            "none" => Self::None,
            other => {
                return Err(crate::Error::Internal(format!(
                    "unknown {} backend: {other:?}. Supported: tavily, linkup, builtin (fetch only), \
                     anthropic, openrouter, none",
                    tool.env_prefix()
                )));
            }
        };
        if kind == Self::Builtin && tool == WebToolKind::Search {
            return Err(crate::Error::Internal(
                "WEB_SEARCH_PROVIDER=builtin is not supported: Kyomi has no search index of its own".into(),
            ));
        }
        Ok(kind)
    }

    /// Whether the LLM provider (not Kyomi) executes this backend.
    pub fn is_provider_executed(self) -> bool {
        matches!(self, Self::Anthropic | Self::OpenRouter)
    }

    /// Whether this backend needs an API key from Kyomi's config.
    pub fn needs_api_key(self) -> bool {
        matches!(self, Self::Tavily | Self::Linkup)
    }
}

/// Raw (unparsed) configuration of one tool's backend.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct WebBackendSettings {
    /// Backend name; `None` = use the tool's default.
    pub provider: Option<String>,
    /// API key for Kyomi-executed backends.
    pub api_key: Option<String>,
    /// Base-URL override for Kyomi-executed backends.
    pub base_url: Option<String>,
    /// Opaque JSON-object options passed through to the backend.
    pub options: Option<String>,
}

/// A resolved backend ready to be constructed.
#[derive(Debug, Clone, PartialEq)]
pub struct WebBackendConfig {
    pub kind: WebBackendKind,
    pub api_key: Option<String>,
    pub base_url: Option<String>,
    /// Opaque options; always a JSON object (empty when unset).
    pub options: serde_json::Map<String, serde_json::Value>,
}

/// Usage caps. Owner-confirmed defaults; all env-configurable.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WebCaps {
    pub per_turn_search: u32,
    pub per_turn_fetch: u32,
    pub workspace_monthly: u32,
    pub global_monthly: u32,
}

impl Default for WebCaps {
    fn default() -> Self {
        Self {
            per_turn_search: DEFAULT_SEARCHES_PER_TURN,
            per_turn_fetch: DEFAULT_FETCHES_PER_TURN,
            workspace_monthly: DEFAULT_WORKSPACE_MONTHLY,
            global_monthly: DEFAULT_GLOBAL_MONTHLY,
        }
    }
}

/// All web-tool settings of an instance.
#[derive(Debug, Clone, PartialEq)]
pub struct WebToolsSettings {
    pub search: WebBackendSettings,
    pub fetch: WebBackendSettings,
    pub caps: WebCaps,
    /// Whether watches / scheduled runs may use the web tools (chat-only by default).
    pub in_unattended_runs: bool,
}

impl Default for WebToolsSettings {
    fn default() -> Self {
        Self::from_lookup(|_| None)
    }
}

/// The LLM side of the compatibility check.
#[derive(Debug, Clone, Copy)]
pub struct LlmFacts<'a> {
    /// Lower-cased provider name (`anthropic`, `openai`, `gemini`).
    pub provider: &'a str,
    pub base_url: Option<&'a str>,
}

/// Whether a base URL points at OpenRouter.
pub fn is_openrouter_base_url(base_url: Option<&str>) -> bool {
    base_url
        .and_then(|u| url::Url::parse(u).ok())
        .and_then(|u| u.host_str().map(str::to_lowercase))
        .is_some_and(|h| h == "openrouter.ai" || h.ends_with(".openrouter.ai"))
}

impl WebToolsSettings {
    /// Read from the process environment.
    pub fn from_env() -> Self {
        Self::from_lookup(|k| std::env::var(k).ok())
    }

    /// Read from a variable lookup (the environment in production, a map in tests).
    pub fn from_lookup(get: impl Fn(&str) -> Option<String>) -> Self {
        let non_empty = |k: &str| get(k).map(|v| v.trim().to_string()).filter(|v| !v.is_empty());
        let backend = |tool: WebToolKind| {
            let p = tool.env_prefix();
            WebBackendSettings {
                provider: non_empty(&format!("{p}_PROVIDER")),
                api_key: non_empty(&format!("{p}_API_KEY")),
                base_url: non_empty(&format!("{p}_BASE_URL")),
                options: non_empty(&format!("{p}_OPTIONS")),
            }
        };
        let cap = |k: &str, default: u32| {
            non_empty(k).and_then(|v| v.parse().ok()).unwrap_or(default)
        };
        let d = WebCaps::default();
        Self {
            search: backend(WebToolKind::Search),
            fetch: backend(WebToolKind::Fetch),
            caps: WebCaps {
                per_turn_search: cap("WEB_TOOLS_SEARCHES_PER_TURN", d.per_turn_search),
                per_turn_fetch: cap("WEB_TOOLS_FETCHES_PER_TURN", d.per_turn_fetch),
                workspace_monthly: cap("WEB_TOOLS_WORKSPACE_MONTHLY_CAP", d.workspace_monthly),
                global_monthly: cap("WEB_TOOLS_GLOBAL_MONTHLY_CAP", d.global_monthly),
            },
            in_unattended_runs: non_empty("WEB_TOOLS_UNATTENDED")
                .is_some_and(|v| matches!(v.to_lowercase().as_str(), "1" | "true" | "yes")),
        }
    }

    fn settings(&self, tool: WebToolKind) -> &WebBackendSettings {
        match tool {
            WebToolKind::Search => &self.search,
            WebToolKind::Fetch => &self.fetch,
        }
    }

    /// Resolve one tool's backend against the instance's LLM provider.
    ///
    /// There is no default backend (owner decision 2026-11-16): when
    /// `*_PROVIDER` is unset the tool is [`ResolvedBackend::Disabled`] — not
    /// registered and not offered to the model. An *explicit* choice that is
    /// incompatible with the LLM provider is an error, not a disable.
    ///
    /// A Kyomi-executed backend without an API key is an error, and so is an
    /// API key / base URL without a provider (partial config), mirroring
    /// `LLM_PROVIDER` / `LLM_API_KEY`.
    pub fn resolve(&self, tool: WebToolKind, llm: LlmFacts<'_>) -> crate::Result<ResolvedBackend> {
        let s = self.settings(tool);
        let p = tool.env_prefix();
        let kind = match s.provider.as_deref() {
            Some(name) => WebBackendKind::parse(tool, name)?,
            None => {
                if s.api_key.is_some() || s.base_url.is_some() {
                    return Err(crate::Error::Internal(format!(
                        "{p}_API_KEY / {p}_BASE_URL is set but {p}_PROVIDER is not. \
                         Set {p}_PROVIDER (tavily, linkup, ...) or remove them."
                    )));
                }
                return Ok(ResolvedBackend::Disabled(format!(
                    "{p}_PROVIDER is not configured"
                )));
            }
        };
        if kind == WebBackendKind::None {
            return Ok(ResolvedBackend::Disabled(format!("{p}_PROVIDER=none")));
        }
        if kind.needs_api_key() && s.api_key.is_none() {
            return Err(crate::Error::Internal(format!(
                "{p}_PROVIDER={kind} requires {p}_API_KEY (set {p}_PROVIDER=none to disable the tool)"
            )));
        }
        Self::check_compat(p, kind, llm)?;
        let options = match s.options.as_deref() {
            None => serde_json::Map::new(),
            Some(raw) => match serde_json::from_str::<serde_json::Value>(raw) {
                Ok(serde_json::Value::Object(map)) => map,
                _ => {
                    return Err(crate::Error::Internal(format!(
                        "{p}_OPTIONS must be a JSON object"
                    )));
                }
            },
        };
        Ok(ResolvedBackend::Enabled(WebBackendConfig {
            kind,
            api_key: s.api_key.clone(),
            base_url: s.base_url.clone(),
            options,
        }))
    }

    fn check_compat(p: &str, kind: WebBackendKind, llm: LlmFacts<'_>) -> crate::Result<()> {
        match kind {
            WebBackendKind::Anthropic if llm.provider != "anthropic" => Err(crate::Error::Internal(format!(
                "{p}_PROVIDER=anthropic requires LLM_PROVIDER=anthropic (got {:?})",
                llm.provider
            ))),
            WebBackendKind::OpenRouter if !is_openrouter_base_url(llm.base_url) => {
                Err(crate::Error::Internal(format!(
                    "{p}_PROVIDER=openrouter requires LLM_BASE_URL to point at openrouter.ai (got {:?})",
                    llm.base_url
                )))
            }
            _ => Ok(()),
        }
    }

    /// Startup validation: both tools resolve (see [`resolve`](Self::resolve)),
    /// so an incompatible backend/LLM combination fails here with a message
    /// naming the variables instead of a 400 at runtime. Returns the resolved
    /// pair `(search, fetch)`.
    pub fn validate(&self, llm: LlmFacts<'_>) -> crate::Result<(ResolvedBackend, ResolvedBackend)> {
        Ok((
            self.resolve(WebToolKind::Search, llm)?,
            self.resolve(WebToolKind::Fetch, llm)?,
        ))
    }
}

/// Outcome of resolving one tool's backend.
#[derive(Debug, Clone, PartialEq)]
pub enum ResolvedBackend {
    /// Tool is on, served by this backend.
    Enabled(WebBackendConfig),
    /// Tool is off; the reason is for the startup log.
    Disabled(String),
}

impl ResolvedBackend {
    /// The backend config when enabled.
    pub fn enabled(&self) -> Option<&WebBackendConfig> {
        match self {
            Self::Enabled(c) => Some(c),
            Self::Disabled(_) => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn settings(vars: &[(&str, &str)]) -> WebToolsSettings {
        let map: std::collections::HashMap<String, String> =
            vars.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect();
        WebToolsSettings::from_lookup(|k| map.get(k).cloned())
    }
    const ANTHROPIC: LlmFacts<'static> = LlmFacts { provider: "anthropic", base_url: None };
    const OPENAI: LlmFacts<'static> = LlmFacts { provider: "openai", base_url: None };

    #[test]
    fn unconfigured_means_tools_absent_and_configured_means_present() {
        let (search, fetch) = settings(&[]).validate(ANTHROPIC).unwrap();
        for r in [search, fetch] {
            assert_eq!(r, ResolvedBackend::Disabled(r_reason(&r)));
            assert!(r.enabled().is_none());
        }
        let (search, fetch) = settings(&[("WEB_SEARCH_PROVIDER", "anthropic"), ("WEB_FETCH_PROVIDER", "anthropic")])
            .validate(ANTHROPIC).unwrap();
        assert!(search.enabled().is_some() && fetch.enabled().is_some());
    }

    fn r_reason(r: &ResolvedBackend) -> String {
        match r {
            ResolvedBackend::Disabled(s) => s.clone(),
            ResolvedBackend::Enabled(_) => String::new(),
        }
    }

    #[test]
    fn explicit_incompatible_combos_fail_loudly() {
        let err = settings(&[("WEB_SEARCH_PROVIDER", "anthropic")]).validate(OPENAI).unwrap_err();
        assert!(err.to_string().contains("WEB_SEARCH_PROVIDER=anthropic requires LLM_PROVIDER=anthropic"));
        let err = settings(&[("WEB_FETCH_PROVIDER", "openrouter")]).validate(ANTHROPIC).unwrap_err();
        assert!(err.to_string().contains("openrouter.ai"));
    }

    #[test]
    fn openrouter_requires_openrouter_base_url() {
        let or = LlmFacts { provider: "openai", base_url: Some("https://openrouter.ai/api/v1") };
        let (s, _) = settings(&[("WEB_SEARCH_PROVIDER", "OpenRouter")]).validate(or).unwrap();
        assert_eq!(s.enabled().unwrap().kind, WebBackendKind::OpenRouter);
        let spoof = LlmFacts { provider: "openai", base_url: Some("https://evil.example/openrouter.ai") };
        assert!(settings(&[("WEB_SEARCH_PROVIDER", "openrouter")]).validate(spoof).is_err());
    }

    #[test]
    fn kyomi_executed_backend_needs_key_and_switching_is_config_only() {
        assert!(settings(&[("WEB_SEARCH_PROVIDER", "tavily")]).validate(OPENAI).is_err());
        let s = settings(&[("WEB_SEARCH_PROVIDER", "tavily"), ("WEB_SEARCH_API_KEY", "k")]);
        assert_eq!(s.resolve(WebToolKind::Search, OPENAI).unwrap().enabled().unwrap().kind, WebBackendKind::Tavily);
        let s = settings(&[("WEB_SEARCH_PROVIDER", "linkup"), ("WEB_SEARCH_API_KEY", "k")]);
        assert_eq!(s.resolve(WebToolKind::Search, OPENAI).unwrap().enabled().unwrap().kind, WebBackendKind::Linkup);
    }

    #[test]
    fn partial_config_key_without_provider_is_an_error() {
        let err = settings(&[("WEB_SEARCH_API_KEY", "k")]).validate(ANTHROPIC).unwrap_err();
        assert!(err.to_string().contains("WEB_SEARCH_PROVIDER is not"));
    }

    #[test]
    fn none_disables_and_search_rejects_builtin() {
        let (s, f) = settings(&[("WEB_SEARCH_PROVIDER", "none"), ("WEB_FETCH_PROVIDER", "builtin")])
            .validate(OPENAI).unwrap();
        assert_eq!(s, ResolvedBackend::Disabled("WEB_SEARCH_PROVIDER=none".into()));
        assert_eq!(f.enabled().unwrap().kind, WebBackendKind::Builtin);
        assert!(settings(&[("WEB_SEARCH_PROVIDER", "builtin")]).validate(ANTHROPIC).is_err());
    }

    #[test]
    fn options_are_an_opaque_json_object() {
        let s = settings(&[("WEB_SEARCH_PROVIDER", "anthropic"), ("WEB_SEARCH_OPTIONS", r#"{"max_uses":3,"engine":"whatever"}"#)]);
        let cfg = s.resolve(WebToolKind::Search, ANTHROPIC).unwrap();
        assert_eq!(cfg.enabled().unwrap().options["max_uses"], 3);
        assert!(settings(&[("WEB_SEARCH_PROVIDER", "anthropic"), ("WEB_SEARCH_OPTIONS", "[1]")]).validate(ANTHROPIC).is_err());
    }

    #[test]
    fn caps_default_to_confirmed_values_and_are_overridable() {
        let d = settings(&[]);
        assert_eq!(d.caps, WebCaps { per_turn_search: 5, per_turn_fetch: 5, workspace_monthly: 200, global_monthly: 1000 });
        assert!(!d.in_unattended_runs);
        let o = settings(&[("WEB_TOOLS_WORKSPACE_MONTHLY_CAP", "7"), ("WEB_TOOLS_UNATTENDED", "true")]);
        assert_eq!(o.caps.workspace_monthly, 7);
        assert!(o.in_unattended_runs);
    }
}
