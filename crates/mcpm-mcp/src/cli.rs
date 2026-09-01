//! Command-line surface: how the server is started, and how an operator
//! mints the keys the agents will present.
//!
//! Deliberately hand-parsed. The whole grammar is a mode plus a handful
//! of string options, and a dependency that turns argv into a struct
//! would be a larger surface than the thing it parses.

use mcpm_core::KeyRole;

/// What this invocation is for.
pub enum Mode {
    /// The default: MCP over stdio, one process per agent, no auth.
    Stdio,
    /// The shared listener a cluster of agents connect to.
    Http { bind: String },
    /// Mint a key and print it — the only moment its secret exists.
    IssueKey { label: Option<String>, agent: String, role: KeyRole },
    /// Every key ever issued, live and revoked.
    ListKeys,
    /// Withdraw one key by its public id.
    RevokeKey { id: String },
    /// `--help`.
    Help,
}

/// Where the HTTP listener binds when `--bind` is not given.
///
/// Loopback, like every other default in this repo. A deployment that
/// wants remote agents says so explicitly (`--bind 0.0.0.0:3211`), so
/// nothing becomes network-reachable because a flag was forgotten.
pub const DEFAULT_BIND: &str = "127.0.0.1:3211";

pub const HELP: &str = "\
mcpm-mcp — the Model Context Project Management server.

TRANSPORTS
  mcpm-mcp                        MCP over stdio (default). One process per
                                  agent, no authentication: the operator
                                  already chose to start it.
  mcpm-mcp --http [--bind ADDR]   Shared HTTP listener for remote agents.
                                  Every request must carry
                                  `Authorization: Bearer <token>`.
                                  ADDR defaults to 127.0.0.1:3211 — pass
                                  0.0.0.0:3211 to accept remote agents.

KEYS
  mcpm-mcp --issue-key --agent NAME --role manager|worker|console [--label TEXT]
                                  Mint a key and print it ONCE.
  mcpm-mcp --list-keys            Every key issued, live and revoked.
  mcpm-mcp --revoke-key ID        Withdraw a key by its public id.

A key is an identity, not just a gate: the agent name and role it was
issued for are what the server records and enforces, so an agent on the
HTTP transport cannot name itself something else. A worker key is
refused by plan_feature, revise_plan, complete_feature and
promote_wants; a console key is refused by the MCP surface entirely.

ENVIRONMENT
  DATABASE_URL        postgres://app:app@localhost:55432/app
  MCPM_PROJECT_NAME   control-center
";

/// Parse argv (excluding the program name). `Err` carries the message
/// to print before exiting non-zero.
pub fn parse(args: &[String]) -> Result<Mode, String> {
    let mut http = false;
    let mut issue = false;
    let mut list = false;
    let mut bind: Option<String> = None;
    let mut revoke: Option<String> = None;
    let mut label: Option<String> = None;
    let mut agent: Option<String> = None;
    let mut role: Option<String> = None;

    let mut it = args.iter();
    while let Some(arg) = it.next() {
        // Every option that takes a value reads it the same way, so a
        // missing value is one message rather than five spellings of it.
        let mut value = |name: &str| -> Result<String, String> {
            it.next()
                .cloned()
                .ok_or_else(|| format!("{name} needs a value. See --help."))
        };
        match arg.as_str() {
            "--help" | "-h" => return Ok(Mode::Help),
            "--http" => http = true,
            "--issue-key" => issue = true,
            "--list-keys" => list = true,
            "--bind" => bind = Some(value("--bind")?),
            "--revoke-key" => revoke = Some(value("--revoke-key")?),
            "--label" => label = Some(value("--label")?),
            "--agent" => agent = Some(value("--agent")?),
            "--role" => role = Some(value("--role")?),
            other => return Err(format!("Unknown argument '{other}'. See --help.")),
        }
    }

    // Modes are mutually exclusive: silently preferring one over another
    // would run a server for somebody who asked for a key.
    let chosen = [http, issue, list, revoke.is_some()]
        .iter()
        .filter(|on| **on)
        .count();
    if chosen > 1 {
        return Err(
            "Pick one of --http, --issue-key, --list-keys, --revoke-key. See --help.".into(),
        );
    }

    if issue {
        let agent = agent.ok_or(
            "--issue-key needs --agent NAME: the name every write by this key is \
             recorded under.",
        )?;
        let role = role.ok_or("--issue-key needs --role manager|worker|console.")?;
        let role = KeyRole::parse(&role)
            .ok_or_else(|| format!("'{role}' is not a role. Use manager, worker, or console."))?;
        return Ok(Mode::IssueKey { label, agent, role });
    }
    if list {
        return Ok(Mode::ListKeys);
    }
    if let Some(id) = revoke {
        return Ok(Mode::RevokeKey { id });
    }
    if http {
        return Ok(Mode::Http { bind: bind.unwrap_or_else(|| DEFAULT_BIND.to_string()) });
    }
    if bind.is_some() {
        return Err("--bind only means something with --http. See --help.".into());
    }
    Ok(Mode::Stdio)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse_argv(argv: &[&str]) -> Result<Mode, String> {
        parse(&argv.iter().map(|s| s.to_string()).collect::<Vec<_>>())
    }

    #[test]
    fn no_arguments_is_the_stdio_server() {
        assert!(matches!(parse_argv(&[]), Ok(Mode::Stdio)));
    }

    #[test]
    fn http_binds_loopback_unless_told_otherwise() {
        let Ok(Mode::Http { bind }) = parse_argv(&["--http"]) else {
            panic!("--http must select the HTTP transport");
        };
        assert_eq!(bind, DEFAULT_BIND, "the default must not be reachable off-host");

        let Ok(Mode::Http { bind }) = parse_argv(&["--http", "--bind", "0.0.0.0:9000"]) else {
            panic!("--bind must be accepted alongside --http");
        };
        assert_eq!(bind, "0.0.0.0:9000");
    }

    #[test]
    fn issuing_a_key_needs_an_agent_and_a_real_role() {
        assert!(parse_argv(&["--issue-key"]).is_err());
        assert!(parse_argv(&["--issue-key", "--agent", "w1"]).is_err());
        assert!(parse_argv(&["--issue-key", "--agent", "w1", "--role", "wizard"]).is_err());

        let Ok(Mode::IssueKey { agent, role, label }) =
            parse_argv(&["--issue-key", "--agent", "w1", "--role", "worker"])
        else {
            panic!("a well-formed issue must parse");
        };
        assert_eq!(agent, "w1");
        assert_eq!(role, KeyRole::Worker);
        assert!(label.is_none());
    }

    #[test]
    fn two_modes_at_once_is_refused_rather_than_ranked() {
        assert!(parse_argv(&["--http", "--list-keys"]).is_err());
        assert!(parse_argv(&["--issue-key", "--list-keys"]).is_err());
    }

    #[test]
    fn an_option_missing_its_value_says_so() {
        assert!(parse_argv(&["--bind"]).is_err());
        assert!(parse_argv(&["--revoke-key"]).is_err());
        assert!(parse_argv(&["--nonsense"]).is_err());
    }

    /// `--bind` without `--http` would otherwise start a stdio server
    /// that silently ignored the address the operator gave it.
    #[test]
    fn bind_without_http_is_a_mistake_not_a_default() {
        assert!(parse_argv(&["--bind", "0.0.0.0:1"]).is_err());
    }
}
