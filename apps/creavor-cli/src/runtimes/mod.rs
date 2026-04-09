mod claude;
mod cline;
mod codex;
mod gemini;
mod openclaw;
mod opencode;
mod qwen;

use crate::settings::RuntimeType;

pub fn run(runtime: RuntimeType) -> anyhow::Result<()> {
    match runtime {
        RuntimeType::Claude => claude::run(),
        RuntimeType::OpenCode => opencode::run(),
        RuntimeType::OpenClaw => openclaw::run(),
        RuntimeType::Codex => codex::run(),
        RuntimeType::Cline => cline::run(),
        RuntimeType::Gemini => gemini::run(),
        RuntimeType::Qwen => qwen::run(),
    }
}

pub fn config(runtime: RuntimeType) -> anyhow::Result<()> {
    match runtime {
        RuntimeType::Claude => claude::config(),
        RuntimeType::OpenCode => opencode::config(),
        RuntimeType::OpenClaw => openclaw::config(),
        RuntimeType::Codex => codex::config(),
        RuntimeType::Cline => cline::config(),
        RuntimeType::Gemini => gemini::config(),
        RuntimeType::Qwen => qwen::config(),
    }
}
