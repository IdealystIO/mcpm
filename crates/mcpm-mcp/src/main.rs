//! `mcpm-mcp` — the Control Center MCP server.
//!
//! MCP stdio transport: newline-delimited JSON-RPC 2.0 on stdin/stdout,
//! logs on stderr. One process per connected agent; all processes share
//! the Postgres store, whose transactions make claims and gates
//! race-safe across connections.
//!
//! Domain rejections (STAGE_LOCKED, TASKS_OPEN, …) are returned as MCP
//! *tool results* flagged `isError` — a gated claim is a domain answer,
//! not a transport fault — carrying the full error envelope (code,
//! message, data, hint) as JSON text.

mod prompts;
mod tools;

use std::io::{BufRead, Write};

use mcpm_core::{
    McpmError, ErrorCode, MemoryScope, PlanFeature, PlanOp, PromoteWants, SearchDirection,
    Store, TaskOutcome, WantDraft, WantEdit, WantFilter, WantState,
};
use serde_json::{json, Value};

const SERVER_VERSION: &str = env!("CARGO_PKG_VERSION");

fn main() {
    let rt = tokio::runtime::Runtime::new().expect("tokio runtime");
    let database_url = std::env::var("DATABASE_URL")
        .unwrap_or_else(|_| "postgres://app:app@localhost:55432/app".to_string());
    let project_name =
        std::env::var("MCPM_PROJECT_NAME").unwrap_or_else(|_| "control-center".to_string());

    let store = match rt.block_on(async {
        let store = Store::connect(&database_url).await?;
        store
            .ensure_project(
                &project_name,
                &std::env::current_dir()
                    .map(|p| p.display().to_string())
                    .unwrap_or_default(),
                "Managed by mcpm — MCP-first project management for agent crews.",
            )
            .await?;
        Ok::<_, McpmError>(store)
    }) {
        Ok(store) => store,
        Err(err) => {
            eprintln!("mcpm-mcp: cannot start: {err}");
            eprintln!("mcpm-mcp: is the devcontainer database up? (DATABASE_URL={database_url})");
            std::process::exit(1);
        }
    };

    eprintln!("mcpm-mcp v{SERVER_VERSION} ready (db: {database_url})");

    // Session identity, set by get_context.
    let mut session: Option<(String, String)> = None;

    let stdin = std::io::stdin();
    let stdout = std::io::stdout();
    for line in stdin.lock().lines() {
        let line = match line {
            Ok(line) => line,
            Err(_) => break,
        };
        if line.trim().is_empty() {
            continue;
        }
        let msg: Value = match serde_json::from_str(&line) {
            Ok(msg) => msg,
            Err(err) => {
                write_msg(
                    &stdout,
                    &json!({
                        "jsonrpc": "2.0", "id": Value::Null,
                        "error": { "code": -32700, "message": format!("parse error: {err}") }
                    }),
                );
                continue;
            }
        };
        let id = msg.get("id").cloned();
        let method = msg.get("method").and_then(Value::as_str).unwrap_or("");
        let params = msg.get("params").cloned().unwrap_or(Value::Null);

        // Notifications need no response.
        let Some(id) = id else { continue };

        let response = match method {
            "initialize" => {
                let requested = params
                    .get("protocolVersion")
                    .and_then(Value::as_str)
                    .unwrap_or("2025-06-18");
                ok(&id, json!({
                    "protocolVersion": requested,
                    "capabilities": { "tools": {}, "prompts": {}, "resources": {} },
                    "serverInfo": { "name": "mcpm", "version": SERVER_VERSION },
                    "instructions": "mcpm (Model Context Project Management). Call get_context first \
                        to register your identity — every other tool requires it. Managers \
                        plan features and dispatch what next_work returns; workers claim one \
                        module, work its checklist, and exit through complete_module, \
                        report_blocker, or release_module. Stage order is enforced by the \
                        server: a STAGE_LOCKED rejection means stop and report to your \
                        manager. Loose ideas live in the want pool (add_want / list_wants); \
                        features are composed out of GROUPS of wants with promote_wants, \
                        never one want to one feature. Ids are prefixed by kind: feat_ stg_ mod_ \
                        tsk_ want_. Beyond tools there are prompts (manager_briefing, \
                        worker_briefing, compose_wants) that brief a fresh agent for a role, \
                        and read-only project:// resources for the board, the want pool, and \
                        the event ledger.",
                }))
            }
            "ping" => ok(&id, json!({})),
            "tools/list" => ok(&id, json!({ "tools": tools::tool_defs() })),
            "prompts/list" => ok(&id, json!({ "prompts": prompts::prompt_defs() })),
            "prompts/get" => match rt.block_on(prompts::get_prompt(&store, &params)) {
                Ok(result) => ok(&id, result),
                Err(err) => rpc_err(&id, -32602, &err.to_string()),
            },
            "resources/list" => match rt.block_on(list_resources(&store)) {
                Ok(result) => ok(&id, result),
                Err(err) => rpc_err(&id, -32603, &err.to_string()),
            },
            "resources/read" => match rt.block_on(read_resource(&store, &params)) {
                Ok(result) => ok(&id, result),
                Err(err) => rpc_err(&id, -32002, &err.to_string()),
            },
            "tools/call" => {
                let name = params.get("name").and_then(Value::as_str).unwrap_or("");
                let args = params.get("arguments").cloned().unwrap_or(json!({}));
                let outcome = rt.block_on(call_tool(&store, &mut session, name, &args));
                match outcome {
                    Ok(value) => ok(&id, tool_text(value, false)),
                    Err(err) => {
                        let envelope = serde_json::to_value(&err)
                            .unwrap_or_else(|_| json!({ "code": "INTERNAL" }));
                        ok(&id, tool_text(envelope, true))
                    }
                }
            }
            _ => rpc_err(&id, -32601, &format!("method not found: {method}")),
        };
        write_msg(&stdout, &response);
    }
}

fn ok(id: &Value, result: Value) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "result": result })
}

fn rpc_err(id: &Value, code: i64, message: &str) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": message } })
}

fn tool_text(value: Value, is_error: bool) -> Value {
    let text = serde_json::to_string_pretty(&value).unwrap_or_else(|_| value.to_string());
    json!({ "content": [{ "type": "text", "text": text }], "isError": is_error })
}

fn write_msg(stdout: &std::io::Stdout, msg: &Value) {
    let mut lock = stdout.lock();
    let _ = serde_json::to_writer(&mut lock, msg);
    let _ = lock.write_all(b"\n");
    let _ = lock.flush();
}

// ---------------------------------------------------------------------
// Tool dispatch
// ---------------------------------------------------------------------

async fn call_tool(
    store: &Store,
    session: &mut Option<(String, String)>,
    name: &str,
    args: &Value,
) -> Result<Value, McpmError> {
    if name == "get_context" {
        let agent = str_arg(args, "agent_name")?;
        let role = str_arg(args, "role")?;
        let ctx = store.get_context(&agent, &role).await?;
        *session = Some((agent, role));
        return to_value(ctx);
    }
    let (agent, _role) = session.clone().ok_or_else(|| {
        McpmError::new(
            ErrorCode::NotRegistered,
            "This session has no identity yet.",
            Value::Null,
            "Call get_context(agent_name, role) first — every other tool attributes its \
             writes to that identity.",
        )
    })?;

    match name {
        "plan_feature" => {
            let plan: PlanFeature = parse_args(args)?;
            to_value(store.plan_feature(&agent, plan).await?)
        }
        "revise_plan" => {
            let feature_id = str_arg(args, "feature_id")?;
            let ops: Vec<PlanOp> = parse_field(args, "ops")?;
            to_value(store.revise_plan(&agent, &feature_id, ops).await?)
        }
        "complete_feature" => {
            let feature_id = str_arg(args, "feature_id")?;
            let summary = str_arg(args, "summary")?;
            to_value(store.complete_feature(&agent, &feature_id, &summary).await?)
        }
        "next_work" => {
            let feature_id = str_arg(args, "feature_id")?;
            to_value(store.next_work(&feature_id).await?)
        }
        "feature_status" => {
            let feature_id = str_arg(args, "feature_id")?;
            let since = args.get("events_since").and_then(Value::as_i64).unwrap_or(0);
            to_value(store.feature_status(&feature_id, since).await?)
        }
        "claim_module" => {
            let module_id = str_arg(args, "module_id")?;
            to_value(store.claim_module(&agent, &module_id).await?)
        }
        "complete_task" => {
            let task_id = str_arg(args, "task_id")?;
            let outcome: TaskOutcome = parse_field(args, "outcome")?;
            let note = opt_str_arg(args, "note");
            to_value(
                store
                    .complete_task(&agent, &task_id, outcome, note.as_deref())
                    .await?,
            )
        }
        "add_task" => {
            let module_id = str_arg(args, "module_id")?;
            let task_name = str_arg(args, "name")?;
            let note = opt_str_arg(args, "note");
            to_value(
                store
                    .add_task(&agent, &module_id, &task_name, note.as_deref())
                    .await?,
            )
        }
        "complete_module" => {
            let module_id = str_arg(args, "module_id")?;
            let summary = str_arg(args, "summary")?;
            to_value(store.complete_module(&agent, &module_id, &summary).await?)
        }
        "report_blocker" => {
            let module_id = str_arg(args, "module_id")?;
            let description = str_arg(args, "description")?;
            to_value(store.report_blocker(&agent, &module_id, &description).await?)
        }
        "release_module" => {
            let module_id = str_arg(args, "module_id")?;
            let reason = str_arg(args, "reason")?;
            to_value(store.release_module(&agent, &module_id, &reason).await?)
        }
        "commit_memory" => {
            let scope: MemoryScope = parse_field(args, "scope")?;
            let content = str_arg(args, "content")?;
            let tags: Vec<String> = args
                .get("tags")
                .map(|t| serde_json::from_value(t.clone()))
                .transpose()
                .map_err(bad_args)?
                .unwrap_or_default();
            to_value(store.commit_memory(&agent, scope, &content, &tags).await?)
        }
        "search_memory" => {
            let query = opt_str_arg(args, "query").unwrap_or_default();
            let scope: Option<MemoryScope> = match args.get("scope") {
                None | Some(Value::Null) => None,
                Some(scope) => Some(serde_json::from_value(scope.clone()).map_err(bad_args)?),
            };
            let direction: SearchDirection = match args.get("direction") {
                None | Some(Value::Null) => {
                    if scope.is_some() {
                        SearchDirection::Here
                    } else {
                        SearchDirection::All
                    }
                }
                Some(direction) => serde_json::from_value(direction.clone()).map_err(bad_args)?,
            };
            let tags: Vec<String> = args
                .get("tags")
                .map(|t| serde_json::from_value(t.clone()))
                .transpose()
                .map_err(bad_args)?
                .unwrap_or_default();
            let limit = args.get("limit").and_then(Value::as_i64).unwrap_or(20);
            to_value(
                store
                    .search_memory(&query, scope, direction, &tags, limit)
                    .await?,
            )
        }
        "add_want" => {
            let body = str_arg(args, "body")?;
            let tags: Vec<String> = args
                .get("tags")
                .map(|t| serde_json::from_value(t.clone()))
                .transpose()
                .map_err(bad_args)?
                .unwrap_or_default();
            to_value(store.add_want(&agent, &body, &tags).await?)
        }
        "add_wants" => {
            let drafts: Vec<WantDraft> = parse_field(args, "wants")?;
            to_value(store.add_wants(&agent, drafts).await?)
        }
        "list_tags" => to_value(store.list_tags().await?),
        "create_tag" => {
            let label = str_arg(args, "label")?;
            to_value(store.create_tag(&agent, &label).await?)
        }
        "list_wants" => {
            let query = opt_str_arg(args, "query").unwrap_or_default();
            let filter: WantFilter = match args.get("status") {
                None | Some(Value::Null) => WantFilter::Open,
                Some(status) => serde_json::from_value(status.clone()).map_err(bad_args)?,
            };
            let tags: Vec<String> = args
                .get("tags")
                .map(|t| serde_json::from_value(t.clone()))
                .transpose()
                .map_err(bad_args)?
                .unwrap_or_default();
            let limit = args.get("limit").and_then(Value::as_i64).unwrap_or(50);
            to_value(store.list_wants(&query, filter, &tags, limit).await?)
        }
        "update_want" => {
            let want_id = str_arg(args, "want_id")?;
            let state: Option<WantState> = match args.get("state") {
                None | Some(Value::Null) => None,
                Some(state) => Some(serde_json::from_value(state.clone()).map_err(bad_args)?),
            };
            let tags: Option<Vec<String>> = args
                .get("tags")
                .map(|t| serde_json::from_value(t.clone()))
                .transpose()
                .map_err(bad_args)?;
            let edit = WantEdit {
                body: opt_str_arg(args, "body"),
                tags,
                state,
                reason: opt_str_arg(args, "reason"),
            };
            to_value(store.update_want(&agent, &want_id, edit).await?)
        }
        "promote_wants" => {
            let req: PromoteWants = parse_args(args)?;
            to_value(store.promote_wants(&agent, req).await?)
        }
        "get_events" => {
            let feature_id = opt_str_arg(args, "feature_id");
            let since = args.get("since").and_then(Value::as_i64).unwrap_or(0);
            let limit = args.get("limit").and_then(Value::as_i64).unwrap_or(100);
            to_value(store.get_events(feature_id.as_deref(), since, limit).await?)
        }
        other => Err(McpmError::new(
            ErrorCode::NotFound,
            format!("Unknown tool '{other}'."),
            Value::Null,
            "Use one of the tools from tools/list.",
        )),
    }
}

// ---------------------------------------------------------------------
// Resources
// ---------------------------------------------------------------------

async fn list_resources(store: &Store) -> Result<Value, McpmError> {
    let mut resources = vec![
        json!({
            "uri": "project://status",
            "name": "Project status",
            "description": "The whole board, compact: project info + per-feature rollups.",
            "mimeType": "application/json"
        }),
        json!({
            "uri": "project://wants",
            "name": "Want pool",
            "description": "Every loose idea captured for this project, with the features \
                (if any) that composed it.",
            "mimeType": "application/json"
        }),
        json!({
            "uri": "project://events",
            "name": "Event ledger",
            "description": "The append-only audit trail (most recent 500).",
            "mimeType": "application/json"
        }),
    ];
    for feature in store.rollups().await? {
        resources.push(json!({
            "uri": format!("project://features/{}", feature.id),
            "name": format!("Feature: {}", feature.name),
            "description": "Full Stage → Module → Task tree with derived gate states.",
            "mimeType": "application/json"
        }));
    }
    Ok(json!({ "resources": resources }))
}

async fn read_resource(store: &Store, params: &Value) -> Result<Value, McpmError> {
    let uri = params
        .get("uri")
        .and_then(Value::as_str)
        .ok_or_else(|| bad_args("missing uri"))?;
    let body = if uri == "project://status" {
        store.snapshot().await?
    } else if uri == "project://wants" {
        serde_json::to_value(store.list_wants("", mcpm_core::WantFilter::All, &[], 500).await?)
            .map_err(McpmError::internal)?
    } else if uri == "project://events" {
        serde_json::to_value(store.get_events(None, 0, 500).await?).map_err(McpmError::internal)?
    } else if let Some(feature_id) = uri.strip_prefix("project://features/") {
        serde_json::to_value(store.feature_tree(feature_id).await?).map_err(McpmError::internal)?
    } else {
        return Err(McpmError::not_found("resource", uri));
    };
    Ok(json!({
        "contents": [{
            "uri": uri,
            "mimeType": "application/json",
            "text": serde_json::to_string_pretty(&body).map_err(McpmError::internal)?,
        }]
    }))
}

// ---------------------------------------------------------------------
// Argument helpers
// ---------------------------------------------------------------------

fn str_arg(args: &Value, key: &str) -> Result<String, McpmError> {
    args.get(key)
        .and_then(Value::as_str)
        .map(str::to_string)
        .filter(|s| !s.trim().is_empty())
        .ok_or_else(|| bad_args(format!("missing required string argument '{key}'")))
}

fn opt_str_arg(args: &Value, key: &str) -> Option<String> {
    args.get(key).and_then(Value::as_str).map(str::to_string)
}

fn parse_args<T: serde::de::DeserializeOwned>(args: &Value) -> Result<T, McpmError> {
    serde_json::from_value(args.clone()).map_err(bad_args)
}

fn parse_field<T: serde::de::DeserializeOwned>(args: &Value, key: &str) -> Result<T, McpmError> {
    let field = args
        .get(key)
        .cloned()
        .ok_or_else(|| bad_args(format!("missing required argument '{key}'")))?;
    serde_json::from_value(field).map_err(bad_args)
}

fn bad_args(err: impl std::fmt::Display) -> McpmError {
    McpmError::new(
        ErrorCode::PlanInvalid,
        format!("Invalid arguments: {err}"),
        Value::Null,
        "Check the tool's inputSchema and resend — nothing was written.",
    )
}

fn to_value<T: serde::Serialize>(value: T) -> Result<Value, McpmError> {
    serde_json::to_value(value).map_err(McpmError::internal)
}
