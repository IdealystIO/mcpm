//! Transport-agnostic JSON-RPC dispatch.
//!
//! Both transports — the stdio server one agent runs locally, and the
//! HTTP listener a cluster of agents share — hand their decoded
//! messages to [`handle_message`] and write back whatever it returns.
//! Keeping the dispatch in one place is what makes the two transports
//! honestly equivalent: a tool cannot exist on one and not the other,
//! and the role gate cannot be enforced on one and forgotten on the
//! other.
//!
//! Identity is the only thing the transports differ on, and [`Session`]
//! is where that difference is named: over stdio an agent declares
//! itself through `get_context`; over HTTP the verified key already
//! said who it is, and `get_context` takes no arguments at all.

use mcpm_core::{
    ErrorCode, KeyIdentity, KeyRole, McpmError, MemoryScope, PlanFeature, PlanOp, PromoteWants,
    SearchDirection, Store, TaskOutcome, WantDraft, WantEdit, WantFilter, WantState,
};
use serde_json::{json, Value};

use crate::{prompts, tools};

const SERVER_VERSION: &str = env!("CARGO_PKG_VERSION");

/// The server instructions returned by `initialize`. Written once here
/// because both transports send it.
pub const INSTRUCTIONS: &str = "mcpm (Model Context Project Management). Call get_context first \
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
    the event ledger.";

/// Who is calling, and how much of that the server had to take on
/// trust.
///
/// The two arms are not interchangeable. `key` is *verified* — it came
/// out of `Store::verify_key`, so the agent name it carries is what the
/// event ledger records and the role it carries is enforceable.
/// `declared` is whatever the caller typed into `get_context` on an
/// unauthenticated stdio session, which is a local process the operator
/// already started: there is no boundary there to enforce, so the role
/// stays the hint it always was.
#[derive(Default, Clone)]
pub struct Session {
    pub key: Option<KeyIdentity>,
    pub declared: Option<(String, String)>,
}

impl Session {
    /// A session whose identity is settled before the first message.
    pub fn keyed(key: KeyIdentity) -> Session {
        Session { key: Some(key), declared: None }
    }

    /// The name every write in this session is attributed to.
    pub fn agent(&self) -> Option<String> {
        match &self.key {
            Some(k) => Some(k.agent_name.clone()),
            None => self.declared.as_ref().map(|(a, _)| a.clone()),
        }
    }

    /// The enforceable role, or `None` when nothing was verified.
    pub fn verified_role(&self) -> Option<KeyRole> {
        self.key.as_ref().map(|k| k.role)
    }
}

/// Handle one decoded JSON-RPC message. Returns the reply, or `None`
/// for a notification (which by spec gets no response).
pub async fn handle_message(store: &Store, session: &mut Session, msg: &Value) -> Option<Value> {
    let id = msg.get("id").cloned();
    let method = msg.get("method").and_then(Value::as_str).unwrap_or("");
    let params = msg.get("params").cloned().unwrap_or(Value::Null);

    // Notifications need no response.
    let id = id?;

    Some(match method {
        "initialize" => {
            let requested = params
                .get("protocolVersion")
                .and_then(Value::as_str)
                .unwrap_or("2025-06-18");
            ok(&id, json!({
                "protocolVersion": requested,
                "capabilities": { "tools": {}, "prompts": {}, "resources": {} },
                "serverInfo": { "name": "mcpm", "version": SERVER_VERSION },
                "instructions": INSTRUCTIONS,
            }))
        }
        "ping" => ok(&id, json!({})),
        "tools/list" => ok(&id, json!({ "tools": tools::tool_defs() })),
        "prompts/list" => ok(&id, json!({ "prompts": prompts::prompt_defs() })),
        "prompts/get" => match prompts::get_prompt(store, &params).await {
            Ok(result) => ok(&id, result),
            Err(err) => rpc_err(&id, -32602, &err.to_string()),
        },
        "resources/list" => match list_resources(store).await {
            Ok(result) => ok(&id, result),
            Err(err) => rpc_err(&id, -32603, &err.to_string()),
        },
        "resources/read" => match read_resource(store, &params).await {
            Ok(result) => ok(&id, result),
            Err(err) => rpc_err(&id, -32002, &err.to_string()),
        },
        "tools/call" => {
            let name = params.get("name").and_then(Value::as_str).unwrap_or("");
            let args = params.get("arguments").cloned().unwrap_or(json!({}));
            match call_tool(store, session, name, &args).await {
                Ok(value) => ok(&id, tool_text(value, false)),
                Err(err) => {
                    let envelope =
                        serde_json::to_value(&err).unwrap_or_else(|_| json!({ "code": "INTERNAL" }));
                    ok(&id, tool_text(envelope, true))
                }
            }
        }
        _ => rpc_err(&id, -32601, &format!("method not found: {method}")),
    })
}

pub fn ok(id: &Value, result: Value) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "result": result })
}

pub fn rpc_err(id: &Value, code: i64, message: &str) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": message } })
}

pub fn tool_text(value: Value, is_error: bool) -> Value {
    let text = serde_json::to_string_pretty(&value).unwrap_or_else(|_| value.to_string());
    json!({ "content": [{ "type": "text", "text": text }], "isError": is_error })
}

// ---------------------------------------------------------------------
// Tool dispatch
// ---------------------------------------------------------------------

async fn call_tool(
    store: &Store,
    session: &mut Session,
    name: &str,
    args: &Value,
) -> Result<Value, McpmError> {
    // The role gate, before anything else. It binds only where a role
    // was actually PROVEN: on an unauthenticated stdio session the role
    // is self-declared, so refusing a call on it would be theatre —
    // whoever typed "worker" can type "manager" — while breaking the
    // local workflow this transport exists for.
    if let Some(role) = session.verified_role() {
        if !role.may_call(name) {
            return Err(McpmError::forbidden(name, role));
        }
    }

    if name == "get_context" {
        // A verified key already said who this is, and its word beats
        // the arguments: taking `agent_name` from a keyed caller would
        // hand back the one thing the key exists to make unforgeable.
        let (agent, role) = match &session.key {
            Some(key) => (key.agent_name.clone(), key.role.as_str().to_string()),
            None => (str_arg(args, "agent_name")?, str_arg(args, "role")?),
        };
        let ctx = store.get_context(&agent, &role).await?;
        if session.key.is_none() {
            session.declared = Some((agent, role));
        }
        return to_value(ctx);
    }
    let agent = session.agent().ok_or_else(|| {
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

pub async fn list_resources(store: &Store) -> Result<Value, McpmError> {
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

pub async fn read_resource(store: &Store, params: &Value) -> Result<Value, McpmError> {
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
