use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Mutex;

/// Approval request status.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ApprovalStatus {
    Pending,
    Approved,
    Rejected,
    Expired,
}

impl std::fmt::Display for ApprovalStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Pending => write!(f, "pending"),
            Self::Approved => write!(f, "approved"),
            Self::Rejected => write!(f, "rejected"),
            Self::Expired => write!(f, "expired"),
        }
    }
}

/// User's approval decision.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ApprovalAction {
    AllowOnce,
    AllowSession,
    Block,
}

impl std::fmt::Display for ApprovalAction {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::AllowOnce => write!(f, "allow_once"),
            Self::AllowSession => write!(f, "allow_session"),
            Self::Block => write!(f, "block"),
        }
    }
}

/// An approval request created by Broker when a medium/high severity rule matches.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ApprovalRequest {
    pub id: String,
    pub request_id: String,
    pub session_id: Option<String>,
    pub runtime: String,
    pub upstream_id: Option<String>,
    pub risk_level: String,
    pub rule_id: String,
    pub sanitized_summary: String,
    pub status: ApprovalStatus,
    pub expires_at: Option<String>,
    pub created_at: String,
}

/// An approval action taken by the user.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ApprovalActionRecord {
    pub id: String,
    pub approval_request_id: String,
    pub action: ApprovalAction,
    pub actor: String,
    pub source: String,
    pub created_at: String,
}

/// In-memory approval state machine.
pub struct ApprovalStore {
    requests: Mutex<HashMap<String, ApprovalRequest>>,
    actions: Mutex<Vec<ApprovalActionRecord>>,
    /// Session-level approvals: session_id → upstream_id
    session_approvals: Mutex<HashMap<String, String>>,
}

impl ApprovalStore {
    pub fn new() -> Self {
        Self {
            requests: Mutex::new(HashMap::new()),
            actions: Mutex::new(Vec::new()),
            session_approvals: Mutex::new(HashMap::new()),
        }
    }

    /// Create a new pending approval request.
    pub fn create_request(&self, request: ApprovalRequest) {
        let mut requests = self.requests.lock().unwrap();
        requests.insert(request.id.clone(), request);
    }

    /// Get a pending approval request by ID.
    pub fn get_request(&self, id: &str) -> Option<ApprovalRequest> {
        let requests = self.requests.lock().unwrap();
        requests.get(id).cloned()
    }

    /// List all pending approval requests.
    pub fn list_pending(&self) -> Vec<ApprovalRequest> {
        let requests = self.requests.lock().unwrap();
        requests
            .values()
            .filter(|r| r.status == ApprovalStatus::Pending)
            .cloned()
            .collect()
    }

    /// Process an approval decision for a given request.
    /// Returns the updated request status.
    pub fn decide(&self, approval_request_id: &str, action: ApprovalAction) -> anyhow::Result<ApprovalStatus> {
        let mut requests = self.requests.lock().unwrap();

        let request = requests
            .get_mut(approval_request_id)
            .ok_or_else(|| anyhow::anyhow!("approval request not found: {}", approval_request_id))?;

        if request.status != ApprovalStatus::Pending {
            anyhow::bail!(
                "approval request {} is already {}",
                approval_request_id,
                request.status
            );
        }

        let new_status = match action {
            ApprovalAction::AllowOnce | ApprovalAction::AllowSession => ApprovalStatus::Approved,
            ApprovalAction::Block => ApprovalStatus::Rejected,
        };

        request.status = new_status;

        // Record the action
        let record = ApprovalActionRecord {
            id: uuid::Uuid::new_v4().to_string(),
            approval_request_id: approval_request_id.to_string(),
            action,
            actor: "local_user".to_string(),
            source: "guard".to_string(),
            created_at: now_iso8601(),
        };

        // If session-level approval, record it
        if action == ApprovalAction::AllowSession {
            if let Some(ref session_id) = request.session_id {
                let mut session_approvals = self.session_approvals.lock().unwrap();
                session_approvals.insert(session_id.clone(), request.rule_id.clone());
            }
        }

        drop(requests);

        let mut actions = self.actions.lock().unwrap();
        actions.push(record);

        Ok(new_status)
    }

    /// Check if a session has an active approval for a given rule.
    pub fn is_session_approved(&self, session_id: &str, rule_id: &str) -> bool {
        let session_approvals = self.session_approvals.lock().unwrap();
        session_approvals
            .get(session_id)
            .map(|approved_rule| approved_rule == rule_id)
            .unwrap_or(false)
    }

    /// Expire pending requests that have passed their expires_at time.
    /// Returns the number of expired requests.
    pub fn expire_timed_out(&self) -> usize {
        let mut requests = self.requests.lock().unwrap();
        let now = now_iso8601();
        let mut expired_count = 0;

        for request in requests.values_mut() {
            if request.status == ApprovalStatus::Pending {
                if let Some(ref expires_at) = request.expires_at {
                    if expires_at < &now {
                        request.status = ApprovalStatus::Expired;
                        expired_count += 1;
                    }
                }
            }
        }

        expired_count
    }
}

fn now_iso8601() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    // Simple ISO-8601-like timestamp from epoch
    // In production this should use a proper datetime library
    format!("{}", secs)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_request(id: &str, status: ApprovalStatus) -> ApprovalRequest {
        ApprovalRequest {
            id: id.to_string(),
            request_id: format!("req-{id}"),
            session_id: Some("session-1".to_string()),
            runtime: "claude-code".to_string(),
            upstream_id: Some("zhipu-anthropic".to_string()),
            risk_level: "high".to_string(),
            rule_id: "rule-secrets".to_string(),
            sanitized_summary: "API key detected".to_string(),
            status,
            expires_at: None,
            created_at: "2026-04-09T12:00:00Z".to_string(),
        }
    }

    #[test]
    fn create_and_get_request() {
        let store = ApprovalStore::new();
        let request = sample_request("appr-1", ApprovalStatus::Pending);
        store.create_request(request);

        let got = store.get_request("appr-1").unwrap();
        assert_eq!(got.status, ApprovalStatus::Pending);
        assert_eq!(got.risk_level, "high");
    }

    #[test]
    fn list_pending_returns_only_pending() {
        let store = ApprovalStore::new();
        store.create_request(sample_request("appr-1", ApprovalStatus::Pending));
        store.create_request(sample_request("appr-2", ApprovalStatus::Approved));

        let pending = store.list_pending();
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].id, "appr-1");
    }

    #[test]
    fn decide_allow_once_approves() {
        let store = ApprovalStore::new();
        store.create_request(sample_request("appr-1", ApprovalStatus::Pending));

        let status = store.decide("appr-1", ApprovalAction::AllowOnce).unwrap();
        assert_eq!(status, ApprovalStatus::Approved);

        let got = store.get_request("appr-1").unwrap();
        assert_eq!(got.status, ApprovalStatus::Approved);
    }

    #[test]
    fn decide_block_rejects() {
        let store = ApprovalStore::new();
        store.create_request(sample_request("appr-1", ApprovalStatus::Pending));

        let status = store.decide("appr-1", ApprovalAction::Block).unwrap();
        assert_eq!(status, ApprovalStatus::Rejected);
    }

    #[test]
    fn decide_on_already_decided_fails() {
        let store = ApprovalStore::new();
        store.create_request(sample_request("appr-1", ApprovalStatus::Pending));
        store.decide("appr-1", ApprovalAction::AllowOnce).unwrap();

        let result = store.decide("appr-1", ApprovalAction::Block);
        assert!(result.is_err());
    }

    #[test]
    fn decide_allow_session_registers_session_approval() {
        let store = ApprovalStore::new();
        store.create_request(sample_request("appr-1", ApprovalStatus::Pending));

        store.decide("appr-1", ApprovalAction::AllowSession).unwrap();

        assert!(store.is_session_approved("session-1", "rule-secrets"));
        assert!(!store.is_session_approved("session-1", "other-rule"));
        assert!(!store.is_session_approved("other-session", "rule-secrets"));
    }

    #[test]
    fn expire_timed_out_expires_old_requests() {
        let store = ApprovalStore::new();
        let mut request = sample_request("appr-1", ApprovalStatus::Pending);
        // Set expires_at in the past (epoch 0)
        request.expires_at = Some("0".to_string());
        store.create_request(request);

        let expired = store.expire_timed_out();
        assert_eq!(expired, 1);

        let got = store.get_request("appr-1").unwrap();
        assert_eq!(got.status, ApprovalStatus::Expired);
    }

    #[test]
    fn approval_status_display() {
        assert_eq!(ApprovalStatus::Pending.to_string(), "pending");
        assert_eq!(ApprovalStatus::Approved.to_string(), "approved");
        assert_eq!(ApprovalStatus::Rejected.to_string(), "rejected");
        assert_eq!(ApprovalStatus::Expired.to_string(), "expired");
    }

    #[test]
    fn approval_action_display() {
        assert_eq!(ApprovalAction::AllowOnce.to_string(), "allow_once");
        assert_eq!(ApprovalAction::AllowSession.to_string(), "allow_session");
        assert_eq!(ApprovalAction::Block.to_string(), "block");
    }
}
