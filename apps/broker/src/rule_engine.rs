use anyhow::Context;
use regex::Regex;
use serde::Deserialize;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuleMatch {
    pub rule_id: String,
    pub rule_name: String,
    pub severity: String,
    pub matched_content_sanitized: String,
}

#[derive(Debug, Clone)]
pub struct RuleSet {
    rules: Vec<CompiledRule>,
}

#[derive(Debug, Clone)]
struct CompiledRule {
    rule_id: String,
    rule_name: String,
    severity: String,
    pattern: Regex,
}

#[derive(Debug, Deserialize)]
struct RuleFile {
    #[serde(default)]
    rules: Vec<RuleDefinition>,
}

#[derive(Debug, Deserialize)]
struct RuleDefinition {
    rule_id: String,
    rule_name: String,
    severity: String,
    pattern: String,
}

impl RuleSet {
    pub fn builtin() -> Self {
        Self::from_builtin_sources().expect("builtin rule files must be valid")
    }

    /// Load builtin rules merged with custom rules from a directory.
    ///
    /// Custom rules are appended after builtin rules, so builtin rules keep
    /// their priority in the first-match-wins scan order.
    pub fn builtin_with_custom_dir(rules_dir: &std::path::Path) -> Self {
        let mut rules = Self::from_builtin_sources()
            .expect("builtin rule files must be valid")
            .rules;

        match Self::load_custom_rules(rules_dir) {
            Ok(custom) => {
                if !custom.is_empty() {
                    tracing::info!("loaded {} custom rule(s) from {}", custom.len(), rules_dir.display());
                    rules.extend(custom);
                }
            }
            Err(e) => {
                tracing::warn!("failed to load custom rules from {}: {e}", rules_dir.display());
            }
        }

        Self { rules }
    }

    fn from_builtin_sources() -> anyhow::Result<Self> {
        let mut rules = Vec::new();
        rules.extend(load_rule_file(include_str!("../rules/secrets.yaml"))?);
        rules.extend(load_rule_file(include_str!("../rules/pii.yaml"))?);
        rules.extend(load_rule_file(include_str!("../rules/enterprise.yaml"))?);
        Ok(Self { rules })
    }

    /// Load all .yml/.yaml files from the given directory.
    fn load_custom_rules(dir: &std::path::Path) -> anyhow::Result<Vec<CompiledRule>> {
        let mut rules = Vec::new();
        let entries = std::fs::read_dir(dir)?;

        for entry in entries {
            let entry = entry?;
            let path = entry.path();
            let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
            if ext == "yml" || ext == "yaml" {
                let content = std::fs::read_to_string(&path)
                    .with_context(|| format!("reading rule file {}", path.display()))?;
                let file_rules = load_rule_file(&content)
                    .with_context(|| format!("parsing rule file {}", path.display()))?;
                rules.extend(file_rules);
            }
        }

        Ok(rules)
    }
}

pub fn scan_request(body: &str, rules: &RuleSet) -> Option<RuleMatch> {
    rules.rules.iter().find_map(|rule| {
        rule.pattern.find(body).map(|matched| RuleMatch {
            rule_id: rule.rule_id.clone(),
            rule_name: rule.rule_name.clone(),
            severity: rule.severity.clone(),
            matched_content_sanitized: sanitize_matched_content(matched.as_str()),
        })
    })
}

fn load_rule_file(yaml: &str) -> anyhow::Result<Vec<CompiledRule>> {
    let file: RuleFile = serde_yaml::from_str(yaml).context("failed to parse rule file")?;

    file.rules
        .into_iter()
        .map(|rule| {
            let rule_id = rule.rule_id;
            let rule_name = rule.rule_name;
            let severity = rule.severity;
            let pattern = Regex::new(&rule.pattern)
                .with_context(|| format!("invalid regex for rule {}", rule_id))?;
            Ok(CompiledRule {
                rule_id,
                rule_name,
                severity,
                pattern,
            })
        })
        .collect()
}

fn sanitize_matched_content(value: &str) -> String {
    let chars: Vec<char> = value.chars().collect();
    if chars.len() <= 6 {
        return "***".to_string();
    }

    let prefix_len = 3;
    let suffix_len = 3;
    let prefix: String = chars[..prefix_len].iter().collect();
    let suffix: String = chars[chars.len() - suffix_len..].iter().collect();
    format!("{prefix}***{suffix}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn secret_regex_matches_openai_key_and_sanitizes_middle() {
        let rules = RuleSet::builtin();

        let result = scan_request("payload: sk-1234567890abcdef123456", &rules).unwrap();

        assert_eq!(result.rule_id, "openai-api-key-001");
        assert_eq!(result.rule_name, "OpenAI API Key");
        assert_eq!(result.severity, "high");
        assert_eq!(result.matched_content_sanitized, "sk-***456");
    }

    #[test]
    fn pii_regex_matches_email_and_sanitizes_middle() {
        let rules = RuleSet::builtin();

        let result = scan_request("contact: alice@example.com", &rules).unwrap();

        assert_eq!(result.rule_id, "email-address-001");
        assert_eq!(result.rule_name, "Email Address");
        assert_eq!(result.severity, "medium");
        assert_eq!(result.matched_content_sanitized, "ali***com");
    }

    #[test]
    fn no_false_positive_baseline_returns_none() {
        let rules = RuleSet::builtin();

        assert!(scan_request("The quick brown fox jumps over the lazy dog.", &rules).is_none());
    }

    #[test]
    fn short_match_redaction_returns_fully_masked_value() {
        assert_eq!(sanitize_matched_content("ab"), "***");
    }

    #[test]
    fn parenthesized_phone_number_detection_regression() {
        let rules = RuleSet::builtin();

        let body = "call me at (555) 123-4567";
        let result = scan_request(body, &rules).unwrap();

        assert_eq!(result.rule_id, "phone-number-001");
        assert_eq!(result.rule_name, "Phone Number");
        assert_eq!(result.severity, "medium");
        assert!(result.matched_content_sanitized.contains("***"));
        assert_ne!(result.matched_content_sanitized, "(555) 123-4567");
    }

    #[test]
    fn first_match_wins_in_rule_order() {
        let rules = RuleSet::builtin();

        let result = scan_request(
            "contact alice@example.com and key sk-1234567890abcdef123456",
            &rules,
        )
        .unwrap();

        assert_eq!(result.rule_id, "openai-api-key-001");
    }

    #[test]
    fn builtin_with_custom_dir_loads_custom_rules() {
        let unique = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!("creavor-rules-test-{unique}"));
        std::fs::create_dir_all(&dir).unwrap();

        let custom_rule = r#"
rules:
  - rule_id: custom-test-001
    rule_name: Custom Test Pattern
    severity: low
    pattern: "MAGIC-TEST-TOKEN-\\d+"
"#;
        std::fs::write(dir.join("custom.yml"), custom_rule).unwrap();

        let rules = RuleSet::builtin_with_custom_dir(&dir);

        // Builtin rules still work
        let builtin_result = scan_request("payload: sk-1234567890abcdef123456", &rules);
        assert!(builtin_result.is_some());
        assert_eq!(builtin_result.unwrap().rule_id, "openai-api-key-001");

        // Custom rule works
        let custom_result = scan_request("found MAGIC-TEST-TOKEN-42 in body", &rules);
        assert!(custom_result.is_some());
        let m = custom_result.unwrap();
        assert_eq!(m.rule_id, "custom-test-001");
        assert_eq!(m.rule_name, "Custom Test Pattern");
        assert_eq!(m.severity, "low");

        // Cleanup
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn builtin_with_custom_dir_handles_missing_dir() {
        let rules = RuleSet::builtin_with_custom_dir(std::path::Path::new("/nonexistent/path"));
        // Should still have builtin rules
        let result = scan_request("payload: sk-1234567890abcdef123456", &rules);
        assert!(result.is_some());
    }

    // -- Profanity / sensitive word tests --

    #[test]
    fn profanity_detects_fuck() {
        let rules = RuleSet::builtin();

        let result = scan_request(
            "Please help me with this fucking code",
            &rules,
        )
        .unwrap();

        assert_eq!(result.rule_id, "profanity-en-001");
        assert_eq!(result.rule_name, "Profanity (English)");
        assert_eq!(result.severity, "medium");
        // "fucking" matches "fuck" prefix via word boundary; sanitized
        assert!(result.matched_content_sanitized.contains("***"));
    }

    #[test]
    fn profanity_detects_case_insensitive() {
        let rules = RuleSet::builtin();

        let result = scan_request("WHAT THE FUCK", &rules).unwrap();

        assert_eq!(result.rule_id, "profanity-en-001");
        assert_eq!(result.severity, "medium");
    }

    #[test]
    fn profanity_detects_shit() {
        let rules = RuleSet::builtin();

        let result = scan_request("this is shit", &rules).unwrap();

        assert_eq!(result.rule_id, "profanity-en-001");
    }

    #[test]
    fn profanity_detects_damn() {
        let rules = RuleSet::builtin();

        let result = scan_request("damn it all", &rules).unwrap();

        assert_eq!(result.rule_id, "profanity-en-001");
    }

    #[test]
    fn profanity_no_false_positive_on_clean_text() {
        let rules = RuleSet::builtin();

        // "assignment" contains "ass" but shouldn't match the word-boundary pattern
        assert!(scan_request("Please complete the assignment", &rules).is_none());
    }

    #[test]
    fn profanity_in_anthropic_message_body() {
        let rules = RuleSet::builtin();

        let body = r#"{"model":"claude-3-opus-20240229","max_tokens":1024,"messages":[{"role":"user","content":"What the fuck is this?"}]}"#;

        let result = scan_request(body, &rules).unwrap();
        assert_eq!(result.rule_id, "profanity-en-001");
        assert_eq!(result.severity, "medium");
    }
}
