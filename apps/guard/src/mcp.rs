use crate::approval::{ApprovalAction, ApprovalStore};
use serde_json::{json, Value};
use std::io::{self, BufRead, Write};
use std::sync::Arc;

/// Lightweight MCP server implementation for Guard.
///
/// Communicates over stdin/stdout using JSON-RPC 2.0.
/// Exposes tools for Claude Code to review and decide on pending approvals.
pub struct McpServer {
    store: Arc<ApprovalStore>,
}

impl McpServer {
    pub fn new(store: Arc<ApprovalStore>) -> Self {
        Self { store }
    }

    /// Run the MCP server, reading JSON-RPC from stdin and writing to stdout.
    pub fn run(&self) -> anyhow::Result<()> {
        let stdin = io::stdin();
        let stdout = io::stdout();
        let mut stdout = stdout.lock();

        for line in stdin.lock().lines() {
            let line = line?;
            if line.trim().is_empty() {
                continue;
            }

            let request: Value = match serde_json::from_str(&line) {
                Ok(v) => v,
                Err(e) => {
                    let error_response = json!({
                        "jsonrpc": "2.0",
                        "error": {"code": -32700, "message": format!("Parse error: {e}")},
                        "id": null
                    });
                    writeln!(stdout, "{}", error_response)?;
                    stdout.flush()?;
                    continue;
                }
            };

            let id = request.get("id").cloned();
            let response = self.handle_request(request);

            let response_with_id = match id {
                Some(id) => {
                    let mut r = response;
                    r.as_object_mut().map(|o| o.insert("id".to_string(), id));
                    r
                }
                None => response,
            };

            writeln!(stdout, "{}", response_with_id)?;
            stdout.flush()?;
        }

        Ok(())
    }

    fn handle_request(&self, request: Value) -> Value {
        let method = request.get("method").and_then(Value::as_str);
        let params = request.get("params").cloned().unwrap_or(json!({}));

        match method {
            Some("initialize") => self.handle_initialize(),
            Some("notifications/initialized") => json!({}),
            Some("tools/list") => self.handle_tools_list(),
            Some("tools/call") => self.handle_tools_call(params),
            _ => json!({
                "jsonrpc": "2.0",
                "error": {"code": -32601, "message": "Method not found"}
            }),
        }
    }

    fn handle_initialize(&self) -> Value {
        json!({
            "jsonrpc": "2.0",
            "result": {
                "protocolVersion": "2024-11-05",
                "capabilities": {
                    "tools": {}
                },
                "serverInfo": {
                    "name": "creavor-guard",
                    "version": "0.1.0"
                }
            }
        })
    }

    fn handle_tools_list(&self) -> Value {
        json!({
            "jsonrpc": "2.0",
            "result": {
                "tools": [
                    {
                        "name": "review_pending",
                        "description": "List all pending approval requests that need user decision",
                        "inputSchema": {
                            "type": "object",
                            "properties": {},
                            "required": []
                        }
                    },
                    {
                        "name": "decide_approval",
                        "description": "Make an approval decision on a pending request. Options: allow_once, allow_session, block",
                        "inputSchema": {
                            "type": "object",
                            "properties": {
                                "approval_id": {
                                    "type": "string",
                                    "description": "The ID of the approval request"
                                },
                                "action": {
                                    "type": "string",
                                    "enum": ["allow_once", "allow_session", "block"],
                                    "description": "The decision to make"
                                }
                            },
                            "required": ["approval_id", "action"]
                        }
                    },
                    {
                        "name": "show_summary",
                        "description": "Show the sanitized summary of a specific approval request",
                        "inputSchema": {
                            "type": "object",
                            "properties": {
                                "approval_id": {
                                    "type": "string",
                                    "description": "The ID of the approval request"
                                }
                            },
                            "required": ["approval_id"]
                        }
                    }
                ]
            }
        })
    }

    fn handle_tools_call(&self, params: Value) -> Value {
        let tool_name = params.get("name").and_then(Value::as_str);
        let arguments = params.get("arguments").cloned().unwrap_or(json!({}));

        let result = match tool_name {
            Some("review_pending") => self.tool_review_pending(),
            Some("decide_approval") => self.tool_decide_approval(arguments),
            Some("show_summary") => self.tool_show_summary(arguments),
            _ => Err(format!("Unknown tool: {:?}", tool_name)),
        };

        match result {
            Ok(content) => json!({
                "jsonrpc": "2.0",
                "result": {
                    "content": [{"type": "text", "text": content}]
                }
            }),
            Err(error) => json!({
                "jsonrpc": "2.0",
                "result": {
                    "content": [{"type": "text", "text": format!("Error: {error}")}],
                    "isError": true
                }
            }),
        }
    }

    fn tool_review_pending(&self) -> Result<String, String> {
        let pending = self.store.list_pending();
        if pending.is_empty() {
            return Ok("No pending approval requests.".to_string());
        }

        let mut output = format!("{} pending approval request(s):\n\n", pending.len());
        for req in &pending {
            output.push_str(&format!(
                "ID: {}\nRuntime: {}\nRisk: {}\nRule: {}\nSummary: {}\nSession: {}\n\n",
                req.id,
                req.runtime,
                req.risk_level,
                req.rule_id,
                req.sanitized_summary,
                req.session_id.as_deref().unwrap_or("none"),
            ));
        }
        output.push_str("Use decide_approval to allow_once, allow_session, or block.");

        Ok(output)
    }

    fn tool_decide_approval(&self, args: Value) -> Result<String, String> {
        let approval_id = args
            .get("approval_id")
            .and_then(Value::as_str)
            .ok_or("missing approval_id")?;

        let action_str = args
            .get("action")
            .and_then(Value::as_str)
            .ok_or("missing action")?;

        let action = match action_str {
            "allow_once" => ApprovalAction::AllowOnce,
            "allow_session" => ApprovalAction::AllowSession,
            "block" => ApprovalAction::Block,
            _ => return Err(format!("invalid action: {action_str}")),
        };

        let status = self
            .store
            .decide(approval_id, action)
            .map_err(|e| e.to_string())?;

        Ok(format!(
            "Approval {} has been {}. Action: {}",
            approval_id, status, action
        ))
    }

    fn tool_show_summary(&self, args: Value) -> Result<String, String> {
        let approval_id = args
            .get("approval_id")
            .and_then(Value::as_str)
            .ok_or("missing approval_id")?;

        let request = self
            .store
            .get_request(approval_id)
            .ok_or(format!("approval request not found: {approval_id}"))?;

        Ok(format!(
            "Approval Request Details:\n\
             ID: {}\n\
             Request ID: {}\n\
             Runtime: {}\n\
             Upstream: {}\n\
             Risk Level: {}\n\
             Rule: {}\n\
             Status: {}\n\
             Summary: {}\n\
             Session: {}\n\
             Created: {}\n\
             Expires: {}",
            request.id,
            request.request_id,
            request.runtime,
            request.upstream_id.as_deref().unwrap_or("none"),
            request.risk_level,
            request.rule_id,
            request.status,
            request.sanitized_summary,
            request.session_id.as_deref().unwrap_or("none"),
            request.created_at,
            request.expires_at.as_deref().unwrap_or("none"),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::approval::{ApprovalRequest, ApprovalStatus};

    #[test]
    fn handle_initialize_returns_capabilities() {
        let store = Arc::new(ApprovalStore::new());
        let server = McpServer::new(store);

        let request = json!({"jsonrpc": "2.0", "method": "initialize", "id": 1});
        let response = server.handle_request(request);

        assert_eq!(response["result"]["serverInfo"]["name"], "creavor-guard");
        assert_eq!(response["result"]["protocolVersion"], "2024-11-05");
    }

    #[test]
    fn handle_tools_list_returns_three_tools() {
        let store = Arc::new(ApprovalStore::new());
        let server = McpServer::new(store);

        let request = json!({"jsonrpc": "2.0", "method": "tools/list", "id": 2});
        let response = server.handle_request(request);

        let tools = response["result"]["tools"].as_array().unwrap();
        assert_eq!(tools.len(), 3);
        assert_eq!(tools[0]["name"], "review_pending");
        assert_eq!(tools[1]["name"], "decide_approval");
        assert_eq!(tools[2]["name"], "show_summary");
    }

    #[test]
    fn tool_review_pending_empty() {
        let store = Arc::new(ApprovalStore::new());
        let server = McpServer::new(store);

        let result = server.tool_review_pending().unwrap();
        assert!(result.contains("No pending"));
    }

    #[test]
    fn tool_review_pending_with_requests() {
        let store = Arc::new(ApprovalStore::new());
        store.create_request(ApprovalRequest {
            id: "appr-1".to_string(),
            request_id: "req-1".to_string(),
            session_id: Some("sess-1".to_string()),
            runtime: "claude-code".to_string(),
            upstream_id: None,
            risk_level: "high".to_string(),
            rule_id: "rule-secrets".to_string(),
            sanitized_summary: "API key detected".to_string(),
            status: ApprovalStatus::Pending,
            expires_at: None,
            created_at: "2026-04-09T12:00:00Z".to_string(),
        });

        let server = McpServer::new(store);
        let result = server.tool_review_pending().unwrap();
        assert!(result.contains("1 pending"));
        assert!(result.contains("appr-1"));
    }

    #[test]
    fn tool_decide_approval() {
        let store = Arc::new(ApprovalStore::new());
        store.create_request(ApprovalRequest {
            id: "appr-1".to_string(),
            request_id: "req-1".to_string(),
            session_id: None,
            runtime: "claude-code".to_string(),
            upstream_id: None,
            risk_level: "high".to_string(),
            rule_id: "rule-secrets".to_string(),
            sanitized_summary: "test".to_string(),
            status: ApprovalStatus::Pending,
            expires_at: None,
            created_at: "2026-04-09T12:00:00Z".to_string(),
        });

        let server = McpServer::new(store);
        let result = server
            .tool_decide_approval(json!({"approval_id": "appr-1", "action": "allow_once"}))
            .unwrap();
        assert!(result.contains("approved"));
    }

    #[test]
    fn tool_decide_approval_invalid_action() {
        let store = Arc::new(ApprovalStore::new());
        let server = McpServer::new(store);

        let result = server.tool_decide_approval(json!({"approval_id": "appr-1", "action": "invalid"}));
        assert!(result.is_err());
    }

    #[test]
    fn tool_show_summary() {
        let store = Arc::new(ApprovalStore::new());
        store.create_request(ApprovalRequest {
            id: "appr-1".to_string(),
            request_id: "req-1".to_string(),
            session_id: Some("sess-1".to_string()),
            runtime: "claude-code".to_string(),
            upstream_id: Some("zhipu-anthropic".to_string()),
            risk_level: "high".to_string(),
            rule_id: "rule-secrets".to_string(),
            sanitized_summary: "API key in body".to_string(),
            status: ApprovalStatus::Pending,
            expires_at: None,
            created_at: "2026-04-09T12:00:00Z".to_string(),
        });

        let server = McpServer::new(store);
        let result = server.tool_show_summary(json!({"approval_id": "appr-1"})).unwrap();
        assert!(result.contains("zhipu-anthropic"));
        assert!(result.contains("API key in body"));
    }
}
