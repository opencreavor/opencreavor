use crate::settings::RuntimeType;

#[derive(Debug)]
pub enum Command {
    Run { runtime: RuntimeType },
    Config { runtime: RuntimeType },
    Status,
    Doctor,
    Cleanup { runtime: Option<RuntimeType> },
    Broker { subcmd: BrokerSubcmd },
}

#[derive(Debug)]
pub enum BrokerSubcmd {
    Start { config: Option<String> },
    Stop,
    Status,
    Logs { last: Option<String>, blocked_only: bool },
    RulesList,
}

const USAGE: &str = "\
creavor — AI-native R&D toolkit

Usage:
  creavor run <runtime>       Launch a runtime through the creavor broker
  creavor config <runtime>    Permanently configure runtime to use broker
  creavor status              Check if the broker is running
  creavor doctor              Check config, ports, upstream connectivity
  creavor cleanup [runtime]   Restore runtime configs, clean residual state
  creavor broker start [--config <path>]  Start the broker server
  creavor broker stop         Stop the broker server
  creavor broker status       Show broker status (port, PID, rules)
  creavor broker logs [--last <dur>] [--blocked-only]  Query audit logs
  creavor broker rules list   List loaded rules

Runtimes:
  claude    Claude Code (Anthropic)
  opencode  OpenCode (OpenAI-compatible)
  openclaw  OpenClaw (OpenAI-compatible)
  codex     Codex (OpenAI-compatible)
  cline     Cline (OpenAI-compatible)
  gemini    Gemini CLI (Google)
  qwen      Qwen Code (OpenAI-compatible)

Examples:
  creavor run claude         Start Claude Code routed through broker
  creavor broker start       Start the broker server
  creavor broker logs --last 1h  Show logs from the last hour

Environment:
  CREAVOR_BROKER_URL         Broker address (default: http://127.0.0.1:8765)
";

fn parse_runtime(name: &str) -> anyhow::Result<RuntimeType> {
    match name {
        "claude" => Ok(RuntimeType::Claude),
        "opencode" => Ok(RuntimeType::OpenCode),
        "openclaw" => Ok(RuntimeType::OpenClaw),
        "codex" => Ok(RuntimeType::Codex),
        "cline" => Ok(RuntimeType::Cline),
        "gemini" => Ok(RuntimeType::Gemini),
        "qwen" => Ok(RuntimeType::Qwen),
        other => anyhow::bail!(
            "unknown runtime: '{other}'. Supported: claude, opencode, openclaw, codex, cline, gemini, qwen"
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
        "doctor" => Ok(Command::Doctor),
        "cleanup" => {
            let runtime = iter.next().map(|s| parse_runtime(&s)).transpose()?;
            Ok(Command::Cleanup { runtime })
        }
        "broker" => {
            let broker_subcmd = iter
                .next()
                .ok_or_else(|| anyhow::anyhow!("missing broker subcommand. Usage: creavor broker <start|stop|status|logs|rules>"))?;
            parse_broker_subcmd(broker_subcmd, iter)
        }
        "--help" | "-h" => {
            println!("{USAGE}");
            std::process::exit(0);
        }
        other => {
            anyhow::bail!("unknown command: '{other}'. Run 'creavor --help' for usage.");
        }
    }
}

fn parse_broker_subcmd(subcmd: String, mut iter: std::vec::IntoIter<String>) -> anyhow::Result<Command> {
    match subcmd.as_str() {
        "start" => {
            let mut config = None;
            while let Some(arg) = iter.next() {
                if arg == "--config" {
                    config = Some(iter.next().ok_or_else(|| {
                        anyhow::anyhow!("missing value for --config")
                    })?);
                } else {
                    anyhow::bail!("unknown argument: '{arg}'");
                }
            }
            Ok(Command::Broker { subcmd: BrokerSubcmd::Start { config } })
        }
        "stop" => Ok(Command::Broker { subcmd: BrokerSubcmd::Stop }),
        "status" => Ok(Command::Broker { subcmd: BrokerSubcmd::Status }),
        "logs" => {
            let mut last = None;
            let mut blocked_only = false;
            while let Some(arg) = iter.next() {
                if arg == "--last" {
                    last = Some(iter.next().ok_or_else(|| {
                        anyhow::anyhow!("missing value for --last")
                    })?);
                } else if arg == "--blocked-only" {
                    blocked_only = true;
                } else {
                    anyhow::bail!("unknown argument: '{arg}'");
                }
            }
            Ok(Command::Broker { subcmd: BrokerSubcmd::Logs { last, blocked_only } })
        }
        "rules" => {
            let sub = iter.next();
            match sub.as_deref() {
                Some("list") => Ok(Command::Broker { subcmd: BrokerSubcmd::RulesList }),
                Some(other) => anyhow::bail!("unknown rules subcommand: '{other}'"),
                None => anyhow::bail!("missing rules subcommand. Usage: creavor broker rules list"),
            }
        }
        other => anyhow::bail!("unknown broker subcommand: '{other}'"),
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
    fn parse_doctor() {
        let cmd = parse(vec!["doctor".into()]).unwrap();
        assert!(matches!(cmd, Command::Doctor));
    }

    #[test]
    fn parse_cleanup_all() {
        let cmd = parse(vec!["cleanup".into()]).unwrap();
        assert!(matches!(cmd, Command::Cleanup { runtime: None }));
    }

    #[test]
    fn parse_cleanup_specific() {
        let cmd = parse(vec!["cleanup".into(), "opencode".into()]).unwrap();
        assert!(matches!(cmd, Command::Cleanup { runtime: Some(RuntimeType::OpenCode) }));
    }

    #[test]
    fn parse_broker_start() {
        let cmd = parse(vec!["broker".into(), "start".into()]).unwrap();
        assert!(matches!(cmd, Command::Broker { subcmd: BrokerSubcmd::Start { config: None } }));
    }

    #[test]
    fn parse_broker_start_with_config() {
        let cmd = parse(vec!["broker".into(), "start".into(), "--config".into(), "my.toml".into()]).unwrap();
        match cmd {
            Command::Broker { subcmd: BrokerSubcmd::Start { config } } => {
                assert_eq!(config, Some("my.toml".to_string()));
            }
            _ => panic!("expected Broker Start"),
        }
    }

    #[test]
    fn parse_broker_stop() {
        let cmd = parse(vec!["broker".into(), "stop".into()]).unwrap();
        assert!(matches!(cmd, Command::Broker { subcmd: BrokerSubcmd::Stop }));
    }

    #[test]
    fn parse_broker_logs() {
        let cmd = parse(vec!["broker".into(), "logs".into(), "--last".into(), "24h".into(), "--blocked-only".into()]).unwrap();
        match cmd {
            Command::Broker { subcmd: BrokerSubcmd::Logs { last, blocked_only } } => {
                assert_eq!(last, Some("24h".to_string()));
                assert!(blocked_only);
            }
            _ => panic!("expected Broker Logs"),
        }
    }

    #[test]
    fn parse_broker_rules_list() {
        let cmd = parse(vec!["broker".into(), "rules".into(), "list".into()]).unwrap();
        assert!(matches!(cmd, Command::Broker { subcmd: BrokerSubcmd::RulesList }));
    }

    #[test]
    fn parse_unknown_runtime_still_errors() {
        let result = parse(vec!["run".into(), "nonexistent".into()]);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("unknown runtime"));
    }
}
