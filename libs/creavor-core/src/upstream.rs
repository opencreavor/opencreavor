use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// A single upstream entry in the Broker local registry.
///
/// Each entry maps an upstream-id to a real API base URL with its protocol family.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct UpstreamEntry {
    /// Protocol family: "anthropic", "openai", or "gemini".
    pub protocol: String,
    /// Real upstream base URL, e.g. "https://open.bigmodel.cn/api/anthropic".
    pub upstream: String,
}

/// Local registry mapping upstream-id → UpstreamEntry.
///
/// Defined in the design document as `upstream_registry`:
/// ```json
/// {
///   "upstream_registry": {
///     "zhipu-anthropic": { "protocol": "anthropic", "upstream": "https://open.bigmodel.cn/api/anthropic" },
///     "openai-direct": { "protocol": "openai", "upstream": "https://api.openai.com/v1" }
///   }
/// }
/// ```
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(default, transparent)]
pub struct UpstreamRegistry {
    entries: HashMap<String, UpstreamEntry>,
}

impl Default for UpstreamRegistry {
    fn default() -> Self {
        Self {
            entries: HashMap::new(),
        }
    }
}

impl UpstreamRegistry {
    /// Create an empty registry.
    pub fn new() -> Self {
        Self::default()
    }

    /// Insert or update an upstream entry.
    pub fn insert(&mut self, id: impl Into<String>, entry: UpstreamEntry) {
        self.entries.insert(id.into(), entry);
    }

    /// Look up an upstream entry by its id.
    pub fn get(&self, id: &str) -> Option<&UpstreamEntry> {
        self.entries.get(id)
    }

    /// Find the first upstream entry matching a given protocol family.
    pub fn find_by_protocol(&self, protocol: &str) -> Option<(&str, &UpstreamEntry)> {
        self.entries
            .iter()
            .find(|(_, e)| e.protocol == protocol)
            .map(|(k, v)| (k.as_str(), v))
    }

    /// Find the default upstream for a given runtime name.
    /// Looks up the legacy `upstream[runtime]` mapping stored in Settings.
    /// This is used as fallback priority #4 in the routing decision.
    pub fn find_by_runtime_default<'a>(
        &'a self,
        runtime: &str,
        runtime_upstreams: &'a HashMap<String, String>,
    ) -> Option<(&'a str, &'a UpstreamEntry)> {
        runtime_upstreams
            .get(runtime)
            .and_then(|url| self.find_by_url(url))
    }

    /// Find an upstream entry whose base URL matches the given URL.
    pub fn find_by_url(&self, url: &str) -> Option<(&str, &UpstreamEntry)> {
        let normalized = url.trim_end_matches('/');
        self.entries.iter().find(|(_, e)| {
            e.upstream.trim_end_matches('/') == normalized
        }).map(|(k, v)| (k.as_str(), v))
    }

    /// Iterate over all entries.
    pub fn iter(&self) -> impl Iterator<Item = (&String, &UpstreamEntry)> {
        self.entries.iter()
    }

    /// Number of registered upstreams.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether the registry is empty.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

/// A session binding record linking a session-id to a runtime and upstream.
///
/// Defined in the design document as `session_registry`:
/// ```json
/// {
///   "session_registry": {
///     "claude-code:a3f2e1:20260409T1430": {
///       "runtime": "claude-code",
///       "upstream_id": "zhipu-anthropic"
///     }
///   }
/// }
/// ```
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct SessionBinding {
    pub runtime: String,
    pub upstream_id: String,
}

/// In-memory session registry mapping session-id → SessionBinding.
#[derive(Debug, Clone, Default)]
pub struct SessionRegistry {
    bindings: HashMap<String, SessionBinding>,
}

impl SessionRegistry {
    /// Create an empty session registry.
    pub fn new() -> Self {
        Self::default()
    }

    /// Register a session binding.
    pub fn insert(&mut self, session_id: impl Into<String>, binding: SessionBinding) {
        self.bindings.insert(session_id.into(), binding);
    }

    /// Look up a session binding by session-id.
    pub fn get(&self, session_id: &str) -> Option<&SessionBinding> {
        self.bindings.get(session_id)
    }

    /// Remove a session binding.
    pub fn remove(&mut self, session_id: &str) -> Option<SessionBinding> {
        self.bindings.remove(session_id)
    }
}

/// Result of the upstream selection process.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedUpstream {
    /// The resolved upstream-id.
    pub upstream_id: String,
    /// The resolved upstream entry.
    pub entry: UpstreamEntry,
}

/// Resolve the upstream for an incoming request following the priority order
/// defined in the design document:
///
/// 1. `X-Creavor-Upstream` header
/// 2. `X-Creavor-Session-Id` → session_registry binding
/// 3. Path protocol family → first matching upstream
/// 4. `upstream[runtime]` default
/// 5. Key routing (not implemented in P0, returns None)
pub fn resolve_upstream(
    header_upstream: Option<&str>,
    header_session_id: Option<&str>,
    path_protocol: Option<&str>,
    runtime: Option<&str>,
    registry: &UpstreamRegistry,
    session_registry: &SessionRegistry,
    runtime_upstreams: &HashMap<String, String>,
) -> Option<ResolvedUpstream> {
    // Priority 1: X-Creavor-Upstream header
    if let Some(upstream_id) = header_upstream {
        if let Some(entry) = registry.get(upstream_id) {
            return Some(ResolvedUpstream {
                upstream_id: upstream_id.to_string(),
                entry: entry.clone(),
            });
        }
    }

    // Priority 2: X-Creavor-Session-Id → session binding
    if let Some(session_id) = header_session_id {
        if let Some(binding) = session_registry.get(session_id) {
            if let Some(entry) = registry.get(&binding.upstream_id) {
                return Some(ResolvedUpstream {
                    upstream_id: binding.upstream_id.clone(),
                    entry: entry.clone(),
                });
            }
        }
    }

    // Priority 3: Path protocol family → first matching upstream
    if let Some(protocol) = path_protocol {
        if let Some((id, entry)) = registry.find_by_protocol(protocol) {
            return Some(ResolvedUpstream {
                upstream_id: id.to_string(),
                entry: entry.clone(),
            });
        }
    }

    // Priority 4: Runtime default upstream
    if let Some(rt) = runtime {
        if let Some((id, entry)) = registry.find_by_runtime_default(rt, runtime_upstreams) {
            return Some(ResolvedUpstream {
                upstream_id: id.to_string(),
                entry: entry.clone(),
            });
        }
    }

    // Priority 5: Key routing — not implemented in P0
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_registry() -> UpstreamRegistry {
        let mut reg = UpstreamRegistry::new();
        reg.insert(
            "zhipu-anthropic",
            UpstreamEntry {
                protocol: "anthropic".to_string(),
                upstream: "https://open.bigmodel.cn/api/anthropic".to_string(),
            },
        );
        reg.insert(
            "openai-direct",
            UpstreamEntry {
                protocol: "openai".to_string(),
                upstream: "https://api.openai.com/v1".to_string(),
            },
        );
        reg.insert(
            "gemini-direct",
            UpstreamEntry {
                protocol: "gemini".to_string(),
                upstream: "https://generativelanguage.googleapis.com".to_string(),
            },
        );
        reg
    }

    #[test]
    fn registry_insert_and_get() {
        let reg = sample_registry();
        let entry = reg.get("zhipu-anthropic").unwrap();
        assert_eq!(entry.protocol, "anthropic");
        assert_eq!(entry.upstream, "https://open.bigmodel.cn/api/anthropic");
    }

    #[test]
    fn registry_find_by_protocol() {
        let reg = sample_registry();
        let (id, entry) = reg.find_by_protocol("openai").unwrap();
        assert_eq!(id, "openai-direct");
        assert_eq!(entry.upstream, "https://api.openai.com/v1");
    }

    #[test]
    fn registry_find_by_protocol_returns_none_for_unknown() {
        let reg = sample_registry();
        assert!(reg.find_by_protocol("unknown").is_none());
    }

    #[test]
    fn resolve_priority_1_header_upstream() {
        let reg = sample_registry();
        let session_reg = SessionRegistry::new();
        let runtime_upstreams = HashMap::new();

        let result = resolve_upstream(
            Some("zhipu-anthropic"),
            None,
            Some("anthropic"),
            Some("claude-code"),
            &reg,
            &session_reg,
            &runtime_upstreams,
        )
        .unwrap();

        assert_eq!(result.upstream_id, "zhipu-anthropic");
    }

    #[test]
    fn resolve_priority_2_session_binding() {
        let reg = sample_registry();
        let mut session_reg = SessionRegistry::new();
        session_reg.insert(
            "session-abc",
            SessionBinding {
                runtime: "claude-code".to_string(),
                upstream_id: "openai-direct".to_string(),
            },
        );
        let runtime_upstreams = HashMap::new();

        let result = resolve_upstream(
            None,
            Some("session-abc"),
            Some("anthropic"),
            Some("claude-code"),
            &reg,
            &session_reg,
            &runtime_upstreams,
        )
        .unwrap();

        assert_eq!(result.upstream_id, "openai-direct");
    }

    #[test]
    fn resolve_priority_3_protocol_family() {
        let reg = sample_registry();
        let session_reg = SessionRegistry::new();
        let runtime_upstreams = HashMap::new();

        let result = resolve_upstream(
            None,
            None,
            Some("gemini"),
            None,
            &reg,
            &session_reg,
            &runtime_upstreams,
        )
        .unwrap();

        assert_eq!(result.upstream_id, "gemini-direct");
    }

    #[test]
    fn resolve_priority_4_runtime_default() {
        let reg = sample_registry();
        let session_reg = SessionRegistry::new();
        let mut runtime_upstreams = HashMap::new();
        runtime_upstreams.insert("claude-code".to_string(), "https://open.bigmodel.cn/api/anthropic".to_string());

        let result = resolve_upstream(
            None,
            None,
            None,
            Some("claude-code"),
            &reg,
            &session_reg,
            &runtime_upstreams,
        )
        .unwrap();

        assert_eq!(result.upstream_id, "zhipu-anthropic");
    }

    #[test]
    fn resolve_returns_none_when_all_fail() {
        let reg = UpstreamRegistry::new();
        let session_reg = SessionRegistry::new();
        let runtime_upstreams = HashMap::new();

        let result = resolve_upstream(
            None,
            None,
            None,
            None,
            &reg,
            &session_reg,
            &runtime_upstreams,
        );

        assert!(result.is_none());
    }

    #[test]
    fn session_registry_insert_and_get() {
        let mut reg = SessionRegistry::new();
        reg.insert(
            "session-1",
            SessionBinding {
                runtime: "codex".to_string(),
                upstream_id: "openai-direct".to_string(),
            },
        );
        let binding = reg.get("session-1").unwrap();
        assert_eq!(binding.runtime, "codex");
        assert_eq!(binding.upstream_id, "openai-direct");
    }
}
