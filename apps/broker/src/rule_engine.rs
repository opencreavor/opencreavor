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

    fn from_builtin_sources() -> anyhow::Result<Self> {
        Self::from_yaml_sources(
            include_str!("../rules/secrets.yaml"),
            include_str!("../rules/pii.yaml"),
            include_str!("../rules/enterprise.yaml"),
        )
    }

    fn from_yaml_sources(
        secrets_yaml: &str,
        pii_yaml: &str,
        enterprise_yaml: &str,
    ) -> anyhow::Result<Self> {
        let mut rules = Vec::new();
        rules.extend(load_rule_file(secrets_yaml)?);
        rules.extend(load_rule_file(pii_yaml)?);
        rules.extend(load_rule_file(enterprise_yaml)?);
        Ok(Self { rules })
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
}
