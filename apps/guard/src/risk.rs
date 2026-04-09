/// Risk level classification for Broker rule matches.
///
/// Defined in the design document:
/// - Critical: Direct block, no approval option
/// - High: Block + pending approval
/// - Medium: Block + pending approval
/// - Low: Default allow with logging

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RiskLevel {
    Critical,
    High,
    Medium,
    Low,
}

impl RiskLevel {
    /// Parse a risk level from a severity string.
    /// Returns Low for unrecognized values.
    pub fn from_severity(severity: &str) -> Self {
        match severity.to_ascii_lowercase().as_str() {
            "critical" => Self::Critical,
            "high" => Self::High,
            "medium" => Self::Medium,
            "low" => Self::Low,
            _ => Self::Low,
        }
    }

    /// Whether this risk level should be directly blocked without approval.
    pub fn should_block_directly(&self) -> bool {
        matches!(self, Self::Critical)
    }

    /// Whether this risk level requires interactive approval before proceeding.
    pub fn requires_approval(&self) -> bool {
        matches!(self, Self::High | Self::Medium)
    }

    /// Whether this risk level is allowed by default (with logging).
    pub fn is_allowed_by_default(&self) -> bool {
        matches!(self, Self::Low)
    }
}

impl std::fmt::Display for RiskLevel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Critical => write!(f, "critical"),
            Self::High => write!(f, "high"),
            Self::Medium => write!(f, "medium"),
            Self::Low => write!(f, "low"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn from_severity_mapping() {
        assert_eq!(RiskLevel::from_severity("critical"), RiskLevel::Critical);
        assert_eq!(RiskLevel::from_severity("high"), RiskLevel::High);
        assert_eq!(RiskLevel::from_severity("medium"), RiskLevel::Medium);
        assert_eq!(RiskLevel::from_severity("low"), RiskLevel::Low);
    }

    #[test]
    fn from_severity_case_insensitive() {
        assert_eq!(RiskLevel::from_severity("HIGH"), RiskLevel::High);
        assert_eq!(RiskLevel::from_severity("Medium"), RiskLevel::Medium);
    }

    #[test]
    fn from_severity_unknown_defaults_to_low() {
        assert_eq!(RiskLevel::from_severity("unknown"), RiskLevel::Low);
        assert_eq!(RiskLevel::from_severity(""), RiskLevel::Low);
    }

    #[test]
    fn critical_blocks_directly() {
        assert!(RiskLevel::Critical.should_block_directly());
        assert!(!RiskLevel::Critical.requires_approval());
        assert!(!RiskLevel::Critical.is_allowed_by_default());
    }

    #[test]
    fn high_requires_approval() {
        assert!(!RiskLevel::High.should_block_directly());
        assert!(RiskLevel::High.requires_approval());
        assert!(!RiskLevel::High.is_allowed_by_default());
    }

    #[test]
    fn medium_requires_approval() {
        assert!(!RiskLevel::Medium.should_block_directly());
        assert!(RiskLevel::Medium.requires_approval());
        assert!(!RiskLevel::Medium.is_allowed_by_default());
    }

    #[test]
    fn low_allows_by_default() {
        assert!(!RiskLevel::Low.should_block_directly());
        assert!(!RiskLevel::Low.requires_approval());
        assert!(RiskLevel::Low.is_allowed_by_default());
    }

    #[test]
    fn display() {
        assert_eq!(RiskLevel::Critical.to_string(), "critical");
        assert_eq!(RiskLevel::High.to_string(), "high");
        assert_eq!(RiskLevel::Medium.to_string(), "medium");
        assert_eq!(RiskLevel::Low.to_string(), "low");
    }
}
