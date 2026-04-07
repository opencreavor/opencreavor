#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Runtime {
    Claude,
    OpenCode,
    OpenClaw,
}

#[derive(Debug)]
pub enum Command {
    Run { runtime: Runtime },
    Status,
}

const USAGE: &str = "\
creavor — AI-native R&D toolkit

Usage:
  creavor run <runtime>   Launch a runtime through the creavor broker
  creavor status           Check if the broker is running

Runtimes:
  claude    Claude Code (Anthropic)
  opencode  OpenCode (OpenAI-compatible)
  openclaw  OpenClaw (OpenAI-compatible)

Examples:
  creavor run claude       Start Claude Code routed through broker
  creavor status           Check broker health

Environment:
  CREAVOR_BROKER_URL       Broker address (default: http://127.0.0.1:8765)
";

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
                .ok_or_else(|| anyhow::anyhow!("missing runtime name. Usage: creavor run <claude|opencode|openclaw>"))?;
            let runtime = match runtime_name.as_str() {
                "claude" => Runtime::Claude,
                "opencode" => Runtime::OpenCode,
                "openclaw" => Runtime::OpenClaw,
                other => anyhow::bail!("unknown runtime: '{other}'. Supported: claude, opencode, openclaw"),
            };
            Ok(Command::Run { runtime })
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
