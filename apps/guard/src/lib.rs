pub mod approval;
pub mod mcp;
pub mod risk;

use approval::ApprovalStore;
use mcp::McpServer;
use std::sync::Arc;

pub fn run() -> anyhow::Result<()> {
    let store = Arc::new(ApprovalStore::new());
    let server = McpServer::new(store);

    tracing::info!("creavor-guard MCP server starting on stdin/stdout");

    server.run()?;

    Ok(())
}
