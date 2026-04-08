mod claude;
mod openclaw;
mod opencode;

use crate::settings::RuntimeType;

pub fn run(runtime: RuntimeType) -> anyhow::Result<()> {
    match runtime {
        RuntimeType::Claude => claude::run(),
        RuntimeType::OpenCode => opencode::run(),
        RuntimeType::OpenClaw => openclaw::run(),
        RuntimeType::Codex | RuntimeType::Cline | RuntimeType::Gemini => {
            // TODO: implement these runtime launchers
            anyhow::bail!("runtime '{}' is not yet implemented", runtime.name())
        }
    }
}

pub fn config(runtime: RuntimeType) -> anyhow::Result<()> {
    match runtime {
        RuntimeType::Claude => claude::config(),
        RuntimeType::OpenCode => opencode::config(),
        RuntimeType::OpenClaw => openclaw::config(),
        RuntimeType::Codex | RuntimeType::Cline | RuntimeType::Gemini => {
            // TODO: implement these runtime configs
            anyhow::bail!("config for '{}' is not yet implemented", runtime.name())
        }
    }
}
