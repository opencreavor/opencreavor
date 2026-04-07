mod claude;
mod openclaw;
mod opencode;

use crate::cli::Runtime;

pub fn run(runtime: Runtime) -> anyhow::Result<()> {
    match runtime {
        Runtime::Claude => claude::run(),
        Runtime::OpenCode => opencode::run(),
        Runtime::OpenClaw => openclaw::run(),
    }
}
