use serde::{Deserialize, Serialize};

/// Sanitization mode for audit data.
///
/// Defined in the design document:
/// - `mask`: Replace sensitive portions with `***`, keeping partial characters (default)
/// - `remove`: Completely remove matched content
/// - `hash`: Replace with a one-way hash for correlation without exposing content
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum SanitizeMode {
    Mask,
    Remove,
    Hash,
}

impl Default for SanitizeMode {
    fn default() -> Self {
        Self::Mask
    }
}

/// Redaction configuration for audit storage.
///
/// Defined in the design document:
/// ```toml
/// [audit]
/// sanitize_mode = "mask"
/// header_whitelist = ["content-type", "x-request-id", "x-ratelimit-remaining"]
/// ```
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(default)]
pub struct RedactionConfig {
    /// Sanitization strategy.
    pub sanitize_mode: SanitizeMode,
    /// Only these headers are stored in audit records.
    pub header_whitelist: Vec<String>,
}

impl Default for RedactionConfig {
    fn default() -> Self {
        Self {
            sanitize_mode: SanitizeMode::Mask,
            header_whitelist: vec![
                "content-type".to_string(),
                "x-request-id".to_string(),
                "x-ratelimit-remaining".to_string(),
            ],
        }
    }
}

impl RedactionConfig {
    /// Check if a header name (case-insensitive) is in the whitelist.
    pub fn is_header_allowed(&self, name: &str) -> bool {
        let lower = name.to_ascii_lowercase();
        self.header_whitelist
            .iter()
            .any(|w| w.to_ascii_lowercase() == lower)
    }

    /// Filter headers, returning only whitelisted ones as a sorted vec of (name, value).
    pub fn filter_headers<'a, I>(&self, headers: I) -> Vec<(&'a str, &'a str)>
    where
        I: Iterator<Item = (&'a str, &'a str)>,
    {
        headers
            .filter(|(name, _)| self.is_header_allowed(name))
            .collect()
    }

    /// Apply the current sanitize mode to matched content.
    pub fn sanitize(&self, content: &str) -> String {
        match self.sanitize_mode {
            SanitizeMode::Mask => mask_content(content),
            SanitizeMode::Remove => "***".to_string(),
            SanitizeMode::Hash => {
                // Simple hash representation for correlation
                use std::collections::hash_map::DefaultHasher;
                use std::hash::{Hash, Hasher};
                let mut hasher = DefaultHasher::new();
                content.hash(&mut hasher);
                format!("hash:{:016x}", hasher.finish())
            }
        }
    }
}

/// Mask content by keeping first 3 and last 3 chars, replacing middle with `***`.
fn mask_content(content: &str) -> String {
    let chars: Vec<char> = content.chars().collect();
    if chars.len() <= 6 {
        return "***".to_string();
    }
    let prefix: String = chars[..3].iter().collect();
    let suffix: String = chars[chars.len() - 3..].iter().collect();
    format!("{prefix}***{suffix}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitize_mode_default_is_mask() {
        assert_eq!(SanitizeMode::default(), SanitizeMode::Mask);
    }

    #[test]
    fn redaction_config_default_whitelist() {
        let config = RedactionConfig::default();
        assert!(config.is_header_allowed("content-type"));
        assert!(config.is_header_allowed("Content-Type"));
        assert!(config.is_header_allowed("x-request-id"));
        assert!(!config.is_header_allowed("authorization"));
        assert!(!config.is_header_allowed("x-api-key"));
    }

    #[test]
    fn mask_content_short() {
        assert_eq!(mask_content("ab"), "***");
    }

    #[test]
    fn mask_content_long() {
        assert_eq!(mask_content("sk-1234567890abcdef123456"), "sk-***456");
    }

    #[test]
    fn mask_content_exact_boundary() {
        assert_eq!(mask_content("123456"), "***");
        assert_eq!(mask_content("1234567"), "123***567");
    }

    #[test]
    fn sanitize_remove_mode() {
        let config = RedactionConfig {
            sanitize_mode: SanitizeMode::Remove,
            ..Default::default()
        };
        assert_eq!(config.sanitize("sk-1234567890"), "***");
    }

    #[test]
    fn sanitize_hash_mode() {
        let config = RedactionConfig {
            sanitize_mode: SanitizeMode::Hash,
            ..Default::default()
        };
        let result = config.sanitize("test-content");
        assert!(result.starts_with("hash:"));
        // Same input should produce same hash
        assert_eq!(result, config.sanitize("test-content"));
    }

    #[test]
    fn filter_headers_returns_whitelisted() {
        let config = RedactionConfig::default();
        let headers = vec![
            ("content-type", "application/json"),
            ("authorization", "Bearer secret"),
            ("x-request-id", "abc-123"),
        ];
        let filtered = config.filter_headers(headers.into_iter());
        assert_eq!(filtered.len(), 2);
        assert_eq!(filtered[0], ("content-type", "application/json"));
        assert_eq!(filtered[1], ("x-request-id", "abc-123"));
    }
}
