use crate::settings::RuntimeType;

#[derive(Debug)]
pub enum Command {
    Run { runtime: RuntimeType },
    Config { runtime: RuntimeType },
    Status,
}

const USAGE: &str = "\
creavor — AI-native R&D toolkit

Usage:
  creavor run <runtime>     Launch a runtime through the creavor broker
  creavor config <runtime>  Permanently configure runtime to use broker
  creavor status            Check if the broker is running

Runtimes:
  claude    Claude Code (Anthropic)
  opencode  OpenCode (OpenAI-compatible)
  openclaw  OpenClaw (OpenAI-compatible)
  codex     Codex (OpenAI-compatible)
  cline     Cline (OpenAI-compatible)
  gemini    Gemini CLI (Google)

Examples:
  creavor run claude       Start Claude Code routed through broker
  creavor config claude    Permanently configure Claude Code to use broker
  creavor status           Check broker health

Environment:
  CREAVOR_BROKER_URL       Broker address (default: http://127.0.0.1:8765)
";

fn parse_runtime(name: &str) -> anyhow::Result<RuntimeType> {
    match name {
        "claude" => Ok(RuntimeType::Claude),
        "opencode" => Ok(RuntimeType::OpenCode),
        "openclaw" => Ok(RuntimeType::OpenClaw),
        "codex" => Ok(RuntimeType::Codex),
        "cline" => Ok(RuntimeType::Cline),
        "gemini" => Ok(RuntimeType::Gemini),
        other => anyhow::bail!(
            "unknown runtime: '{other}'. Supported: claude, opencode, openclaw, codex, cline, gemini"
        ),
    }
}

pub fn parse(args: Vec<String>) -> anyhow::Result<Command> {
    let mut iter = args.into_iter();

    let subcommand = match iter.next() {
        Some(s) => s,
        None => {
            println!("{USAGE}");
            std::process::exit(0);
        }
    };

    match subcommand.as_str() {
        "run" => {
            let runtime_name = iter
                .next()
                .ok_or_else(|| anyhow::anyhow!("missing runtime name. Usage: creavor run <runtime>"))?;
            let runtime = parse_runtime(&runtime_name)?;
            Ok(Command::Run { runtime })
        }
        "config" => {
            let runtime_name = iter
                .next()
                .ok_or_else(|| anyhow::anyhow!("missing runtime name. Usage: creavor config <runtime>"))?;
            let runtime = parse_runtime(&runtime_name)?;
            Ok(Command::Config { runtime })
        }
        "status" => Ok(Command::Status),
        "--help" | "-h" => {
            println!("{USAGE}");
            std::process::exit(0);
        }
        other => {
            anyhow::bail!("unknown command: '{other}'. Run 'creavor --help' for usage.");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_run_claude() {
        let cmd = parse(vec!["run".into(), "claude".into()]).unwrap();
        assert!(matches!(cmd, Command::Run { runtime: RuntimeType::Claude }));
    }

    #[test]
    fn parse_status() {
        let cmd = parse(vec!["status".into()]).unwrap();
        assert!(matches!(cmd, Command::Status));
    }

    #[test]
    fn parse_config_claude() {
        let cmd = parse(vec!["config".into(), "claude".into()]).unwrap();
        assert!(matches!(cmd, Command::Config { runtime: RuntimeType::Claude }));
    }

    #[test]
    fn parse_run_gemini() {
        let cmd = parse(vec!["run".into(), "gemini".into()]).unwrap();
        assert!(matches!(cmd, Command::Run { runtime: RuntimeType::Gemini }));
    }

    #[test]
    fn parse_unknown_runtime_still_errors() {
        let result = parse(vec!["run".into(), "nonexistent".into()]);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("unknown runtime"));
    }
}
