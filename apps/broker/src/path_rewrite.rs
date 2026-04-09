/// Path rewrite and URL join logic for the Broker.
///
/// Implements the design document's "Path Rewrite & URL Join Rules":
/// - Incoming path: `/v1/{protocol}/{upstream-id}/{tail}`
/// - Forward URL: `normalize_join(upstream_base_url, tail)`
///
/// Also supports the simplified path for Claude Code:
/// - Incoming path: `/v1/{protocol}/{tail}` (no upstream-id in path)
/// - Upstream resolved from header or fallback

/// Parsed result from an incoming request path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedPath {
    /// Protocol family: "anthropic", "openai", or "gemini".
    pub protocol: String,
    /// Upstream ID extracted from the path, if present.
    /// For paths like `/v1/anthropic/zhipu-anthropic/messages`, this is `Some("zhipu-anthropic")`.
    /// For simplified paths like `/v1/anthropic/messages`, this is `None`.
    pub upstream_id: Option<String>,
    /// Remaining tail path after protocol and optional upstream-id.
    /// e.g., `/messages`, `/chat/completions`, `/models/gemini-2.5-pro:generateContent`
    pub tail: String,
}

/// Parse an incoming request path into protocol, optional upstream-id, and tail.
///
/// Supports two formats:
/// 1. Full path: `/v1/{protocol}/{upstream-id}/{tail}` — 4+ segments
/// 2. Simplified path: `/v1/{protocol}/{tail}` — 3 segments (no upstream-id)
pub fn parse_request_path(path: &str) -> Option<ParsedPath> {
    let path_and_query = path.split('?').next().unwrap_or(path);

    // Must start with /v1/
    let rest = path_and_query.strip_prefix("/v1/")?;

    // Extract protocol
    let (protocol, remainder) = rest.split_once('/')?;

    // Validate protocol
    if !matches!(protocol, "anthropic" | "openai" | "gemini") {
        return None;
    }

    // Try to extract upstream-id from the remaining path.
    // We need to determine if the next segment is an upstream-id or part of the API tail.
    // Strategy: check if remainder has at least 2 segments (upstream-id/tail).
    // If it only has 1 segment, it's the tail (simplified path).
    if let Some((potential_upstream_id, tail)) = remainder.split_once('/') {
        // Full path format: /v1/{protocol}/{upstream-id}/{tail}
        Some(ParsedPath {
            protocol: protocol.to_string(),
            upstream_id: Some(potential_upstream_id.to_string()),
            tail: format!("/{tail}"),
        })
    } else {
        // Simplified path format: /v1/{protocol}/{tail}
        // e.g., /v1/anthropic/messages → protocol=anthropic, upstream_id=None, tail=/messages
        Some(ParsedPath {
            protocol: protocol.to_string(),
            upstream_id: None,
            tail: format!("/{remainder}"),
        })
    }
}

/// Normalize and join a base URL with a tail path.
///
/// Implements the design document's `normalize_join()`:
/// - Strip trailing `/` from base URL
/// - Ensure tail starts with single `/`
/// - Preserve query string from tail
pub fn normalize_join(base_url: &str, tail: &str) -> String {
    let normalized_base = base_url.trim_end_matches('/');

    // Split tail into path and query
    let (tail_path, query) = match tail.split_once('?') {
        Some((p, q)) => (p, Some(q)),
        None => (tail, None),
    };

    let normalized_tail = if tail_path.starts_with('/') {
        tail_path.to_string()
    } else {
        format!("/{tail_path}")
    };

    match query {
        Some(q) => format!("{}{}?{q}", normalized_base, normalized_tail),
        None => format!("{}{}", normalized_base, normalized_tail),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // -- parse_request_path tests --

    #[test]
    fn parse_full_path_with_upstream_id() {
        let parsed = parse_request_path("/v1/anthropic/zhipu-anthropic/messages").unwrap();
        assert_eq!(parsed.protocol, "anthropic");
        assert_eq!(parsed.upstream_id, Some("zhipu-anthropic".to_string()));
        assert_eq!(parsed.tail, "/messages");
    }

    #[test]
    fn parse_full_path_openai() {
        let parsed = parse_request_path("/v1/openai/openai-direct/responses").unwrap();
        assert_eq!(parsed.protocol, "openai");
        assert_eq!(parsed.upstream_id, Some("openai-direct".to_string()));
        assert_eq!(parsed.tail, "/responses");
    }

    #[test]
    fn parse_full_path_gemini() {
        let parsed = parse_request_path("/v1/gemini/google-direct/models/gemini-2.5-pro:generateContent").unwrap();
        assert_eq!(parsed.protocol, "gemini");
        assert_eq!(parsed.upstream_id, Some("google-direct".to_string()));
        assert_eq!(parsed.tail, "/models/gemini-2.5-pro:generateContent");
    }

    #[test]
    fn parse_simplified_path_no_upstream_id() {
        let parsed = parse_request_path("/v1/anthropic/messages").unwrap();
        assert_eq!(parsed.protocol, "anthropic");
        assert_eq!(parsed.upstream_id, None);
        assert_eq!(parsed.tail, "/messages");
    }

    #[test]
    fn parse_simplified_path_with_query() {
        let parsed = parse_request_path("/v1/openai/chat/completions?stream=true").unwrap();
        assert_eq!(parsed.protocol, "openai");
        // With 3+ segments after protocol, chat is treated as upstream-id
        assert_eq!(parsed.upstream_id, Some("chat".to_string()));
        assert_eq!(parsed.tail, "/completions");
    }

    #[test]
    fn parse_path_without_v1_prefix_returns_none() {
        assert!(parse_request_path("/anthropic/messages").is_none());
    }

    #[test]
    fn parse_path_unknown_protocol_returns_none() {
        assert!(parse_request_path("/v1/unknown/something").is_none());
    }

    #[test]
    fn parse_path_only_protocol_returns_none() {
        // /v1/anthropic is not a valid path (no tail)
        assert!(parse_request_path("/v1/anthropic").is_none());
    }

    // -- normalize_join tests --

    #[test]
    fn join_trailing_slash_on_base() {
        assert_eq!(
            normalize_join("https://api.xxx.com/v1/", "/responses"),
            "https://api.xxx.com/v1/responses"
        );
    }

    #[test]
    fn join_no_trailing_slash_on_base() {
        assert_eq!(
            normalize_join("https://api.xxx.com/v1", "/responses"),
            "https://api.xxx.com/v1/responses"
        );
    }

    #[test]
    fn join_anthropic_example() {
        assert_eq!(
            normalize_join("https://open.bigmodel.cn/api/anthropic", "/messages"),
            "https://open.bigmodel.cn/api/anthropic/messages"
        );
    }

    #[test]
    fn join_openai_example() {
        assert_eq!(
            normalize_join("https://api.openai.com/v1", "/responses"),
            "https://api.openai.com/v1/responses"
        );
    }

    #[test]
    fn join_gemini_example() {
        assert_eq!(
            normalize_join("https://generativelanguage.googleapis.com", "/models/gemini-2.5-pro:generateContent"),
            "https://generativelanguage.googleapis.com/models/gemini-2.5-pro:generateContent"
        );
    }

    #[test]
    fn join_preserves_query_string() {
        assert_eq!(
            normalize_join("https://api.xxx.com/v1", "/chat/completions?stream=true&model=gpt-4"),
            "https://api.xxx.com/v1/chat/completions?stream=true&model=gpt-4"
        );
    }

    #[test]
    fn join_trailing_slash_base_with_trailing_slash_tail() {
        assert_eq!(
            normalize_join("https://api.xxx.com/v1/", "/responses"),
            "https://api.xxx.com/v1/responses"
        );
    }

    #[test]
    fn join_empty_tail_becomes_slash() {
        // Edge case: tail is just the protocol prefix stripped
        assert_eq!(
            normalize_join("https://api.anthropic.com", "/"),
            "https://api.anthropic.com/"
        );
    }
}
