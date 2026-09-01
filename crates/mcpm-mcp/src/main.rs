// `tools::tool_defs` builds the whole tool list as one `json!` literal,
// and serde_json's macro recurses once per token. Twenty-six tools with
// full schemas exceeds the default 128 frames — the error names the
// macro rather than the file, so it is worth saying here that this is a
// macro-expansion budget and not a runtime stack.
#![recursion_limit = "1024"]

//! `mcpm-mcp` — the Control Center MCP server.
//!
//! Two transports over one dispatcher ([`rpc`]):
//!
//! - **stdio** (default): newline-delimited JSON-RPC 2.0 on
//!   stdin/stdout, logs on stderr. One process per connected agent,
//!   started by that agent's own runner — a local pipe with no network
//!   surface, so it carries no authentication.
//! - **HTTP** (`--http`): one shared listener for agents running on
//!   other machines. Every request presents an API key, and that key —
//!   not the agent — decides what name the writes are recorded under
//!   and which tools are reachable.
//!
//! Both share the Postgres store, whose transactions make claims and
//! gates race-safe across connections however the caller arrived.
//!
//! Domain rejections (STAGE_LOCKED, TASKS_OPEN, FORBIDDEN, …) are
//! returned as MCP *tool results* flagged `isError` — a gated claim is a
//! domain answer, not a transport fault — carrying the full error
//! envelope (code, message, data, hint) as JSON text.

mod cli;
mod http;
mod prompts;
mod rpc;
mod tools;

use std::io::{BufRead, Write};

use mcpm_core::{ApiKeyInfo, McpmError, Store};
use serde_json::Value;

use crate::cli::Mode;
use crate::rpc::Session;

const SERVER_VERSION: &str = env!("CARGO_PKG_VERSION");
/// What the ledger records as the issuer of a key minted from the
/// command line. The operator is at a shell, not holding a key.
const OPERATOR: &str = "operator";

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mode = match cli::parse(&args) {
        Ok(mode) => mode,
        Err(message) => {
            eprintln!("mcpm-mcp: {message}");
            std::process::exit(2);
        }
    };
    if let Mode::Help = mode {
        println!("{}", cli::HELP);
        return;
    }

    let rt = tokio::runtime::Runtime::new().expect("tokio runtime");
    let store = rt.block_on(connect());

    let outcome = match mode {
        Mode::Help => Ok(()),
        Mode::Stdio => {
            eprintln!("mcpm-mcp v{SERVER_VERSION} ready (stdio)");
            rt.block_on(run_stdio(&store));
            Ok(())
        }
        Mode::Http { bind } => rt.block_on(http::serve(store, &bind)),
        Mode::IssueKey { label, agent, role } => rt.block_on(async {
            let issued = store
                .issue_key(OPERATOR, label.as_deref().unwrap_or(""), &agent, role)
                .await
                .map_err(|e: McpmError| e.to_string())?;
            // The one moment the secret exists outside the bearer's
            // hands. On stdout so it can be piped; everything else this
            // binary says goes to stderr.
            println!("{}", issued.token);
            eprintln!(
                "mcpm-mcp: issued {} for agent '{}' as {} — this token is shown once and \
                 cannot be recovered.",
                issued.info.id,
                issued.info.agent_name,
                issued.info.role.as_str()
            );
            Ok(())
        }),
        Mode::ListKeys => rt.block_on(async {
            let keys = store.list_keys().await.map_err(|e: McpmError| e.to_string())?;
            print_keys(&keys);
            Ok(())
        }),
        Mode::RevokeKey { id } => rt.block_on(async {
            let info = store
                .revoke_key(OPERATOR, &id)
                .await
                .map_err(|e: McpmError| e.to_string())?;
            eprintln!(
                "mcpm-mcp: revoked {} ('{}', agent '{}'). Any agent holding it is now locked out.",
                info.id, info.label, info.agent_name
            );
            Ok(())
        }),
    };

    if let Err(message) = outcome {
        eprintln!("mcpm-mcp: {message}");
        std::process::exit(1);
    }
}

async fn connect() -> Store {
    let database_url = std::env::var("DATABASE_URL")
        .unwrap_or_else(|_| "postgres://app:app@localhost:55432/app".to_string());
    let project_name =
        std::env::var("MCPM_PROJECT_NAME").unwrap_or_else(|_| "control-center".to_string());

    let store = match Store::connect(&database_url).await {
        Ok(store) => store,
        Err(err) => {
            eprintln!("mcpm-mcp: cannot start: {err}");
            eprintln!("mcpm-mcp: is the devcontainer database up? (DATABASE_URL={database_url})");
            std::process::exit(1);
        }
    };
    if let Err(err) = store
        .ensure_project(
            &project_name,
            &std::env::current_dir()
                .map(|p| p.display().to_string())
                .unwrap_or_default(),
            "Managed by mcpm — MCP-first project management for agent crews.",
        )
        .await
    {
        eprintln!("mcpm-mcp: cannot start: {err}");
        std::process::exit(1);
    }
    eprintln!("mcpm-mcp: db {database_url}");
    store
}

/// The stdio transport. One session for the life of the pipe: the agent
/// names itself once through `get_context` and every later message
/// inherits that.
async fn run_stdio(store: &Store) {
    let mut session = Session::default();
    let stdin = std::io::stdin();
    let stdout = std::io::stdout();
    for line in stdin.lock().lines() {
        let Ok(line) = line else { break };
        if line.trim().is_empty() {
            continue;
        }
        let msg: Value = match serde_json::from_str(&line) {
            Ok(msg) => msg,
            Err(err) => {
                write_msg(
                    &stdout,
                    &serde_json::json!({
                        "jsonrpc": "2.0", "id": Value::Null,
                        "error": { "code": -32700, "message": format!("parse error: {err}") }
                    }),
                );
                continue;
            }
        };
        if let Some(reply) = rpc::handle_message(store, &mut session, &msg).await {
            write_msg(&stdout, &reply);
        }
    }
}

fn write_msg(stdout: &std::io::Stdout, msg: &Value) {
    let mut lock = stdout.lock();
    let _ = serde_json::to_writer(&mut lock, msg);
    let _ = lock.write_all(b"\n");
    let _ = lock.flush();
}

/// `--list-keys`, as a table an operator can read at a glance. Revoked
/// keys stay listed: "what did we withdraw, and when" is exactly the
/// question this output gets asked.
fn print_keys(keys: &[ApiKeyInfo]) {
    if keys.is_empty() {
        println!("No keys issued. Mint one: mcpm-mcp --issue-key --agent NAME --role manager");
        return;
    }
    println!(
        "{:<14} {:<10} {:<20} {:<20} {:<12} {}",
        "KEY", "ROLE", "AGENT", "LABEL", "LAST USED", "STATE"
    );
    for k in keys {
        println!(
            "{:<14} {:<10} {:<20} {:<20} {:<12} {}",
            k.id,
            k.role.as_str(),
            k.agent_name,
            k.label,
            k.last_used
                .map(|t| t.format("%Y-%m-%d").to_string())
                .unwrap_or_else(|| "never".into()),
            match k.revoked_at {
                Some(at) => format!("revoked {}", at.format("%Y-%m-%d")),
                None => "live".to_string(),
            }
        );
    }
}
