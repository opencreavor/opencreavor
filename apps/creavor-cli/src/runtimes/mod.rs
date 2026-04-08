mod claude;
mod openclaw;
mod opencode;

use crate::settings::RuntimeType;

pub fn run(runtime: RuntimeType) -> anyhow::Result<()> {
    match runtime {
        RuntimeType::Claude => claude::run(),
        RuntimeType::OpenCode => opencode::run(),
        RuntimeType::OpenClaw => openclaw::run(),
        other => anyhow::bail!("runtime '{}' is not yet implemented", other.name()),
    }
}

pub fn config(runtime: RuntimeType) -> anyhow::Result<()> {
    match runtime {
        RuntimeType::Claude => claude::config(),
        RuntimeType::OpenCode => opencode::config(),
        RuntimeType::OpenClaw => openclaw::config(),
        other => anyhow::bail!("runtime '{}' config is not yet implemented", other.name()),
    }
}
