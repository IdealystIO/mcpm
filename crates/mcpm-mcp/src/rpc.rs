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
//!
//! A third source cuts across both. A `delegation_token` argument, when
//! it resolves, REPLACES the session's identity for that one call: the
//! write is attributed to the minted name, the role is forced to worker
//! whatever the key says, and the actor is confined to the module the
//! token names. That is how one process tree — which can only ever hold
//! one key — holds more than one agent.

use mcpm_core::{
    Actor, Delegation, EdgeKind, ErrorCode, KeyIdentity, KeyRole, McpmError, MemoryKind, MemoryQuery,
    MemoryScope, MintRequest, PlanFeature,
    PlanOp, PromoteWants, SearchDirection, Store, Supersede, TaskOutcome, WantDraft, WantEdit,
    WantFilter, WantState,
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
    report_blocker, or release_module. Workers are dispatched, not \
    self-directing: a worker's module ids arrive from its manager in its \
    launch prompt, in stage order, so a worker box started without them \
    has nothing to claim — the board and next_work are readable by any \
    agent, but nothing assigns a worker its own module. Stage order is \
    enforced by the server: a STAGE_LOCKED rejection means stop and \
    report to your \
    manager. Loose ideas live in the want pool (add_want / list_wants); \
    features are composed out of GROUPS of wants with promote_wants, \
    never one want to one feature. Ids are prefixed by kind: feat_ stg_ mod_ \
    tsk_ want_. Subagents that share one machine's key each get their own \
    identity from mint_worker: the manager mints one per module and puts \
    the token in the subagent's prompt, and the subagent passes \
    delegation_token on get_context and on every write. A worker that \
    runs on its OWN box is a different case: give it a key of its own \
    with issue_worker_key, because a delegation token is honoured \
    alongside exactly one key and boxes sharing one key are one identity \
    the ledger cannot split. Beyond tools \
    there are prompts (manager_briefing, \
    worker_briefing, compose_wants) that brief a fresh agent for a role, \
    and read-only project:// resources for the board, the want pool, and \
    the event ledger. The project also has a knowledge base, and you are \
    already using it: complete_module commits your summary to it, and \
    claim_module pushes your lineage's memories into your briefing \
    unasked. search_memory is for reaching OUTSIDE that lineage — why \
    something load-bearing is the way it is, an answer that predates your \
    feature, a constraint the code does not explain. It is a resource, \
    not a ritual: a search at the top of every module is noise.";

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
    // A delegation token, if one was presented, is resolved BEFORE the
    // gate — because resolving it is what decides which role the gate
    // applies. It is verified against the session's own key, so a token
    // from another machine resolves to nothing here.
    let delegation = resolve_delegation(store, session, args).await?;

    // The role gate. It binds where a role was PROVEN — by a key, or by
    // a delegation the server just minted and resolved itself. On an
    // unauthenticated stdio session with no token the role is
    // self-declared, so refusing a call on it would be theatre —
    // whoever typed "worker" can type "manager" — while breaking the
    // local workflow this transport exists for.
    //
    // A resolved delegation forces WORKER whatever key carried it. That
    // single line is what lets a manager's process tree contain workers:
    // the minting key stays a manager, every subagent holding one of its
    // tokens is not.
    let effective_role = match &delegation {
        Some(_) => Some(KeyRole::Worker),
        None => session.verified_role(),
    };
    if let Some(role) = effective_role {
        if !role.may_call(name) {
            return Err(McpmError::forbidden(name, role));
        }
    }

    if name == "get_context" {
        // A verified identity already said who this is, and its word
        // beats the arguments: taking `agent_name` from a keyed or
        // delegated caller would hand back the one thing the credential
        // exists to make unforgeable. A subagent needs this call to
        // re-orient — `your_claims` and the suggested next step are
        // wrong for it otherwise — so the token is honoured here too.
        let (agent, role) = match (&delegation, &session.key) {
            (Some(d), _) => (d.agent_name.clone(), KeyRole::Worker.as_str().to_string()),
            (None, Some(key)) => (key.agent_name.clone(), key.role.as_str().to_string()),
            (None, None) => (str_arg(args, "agent_name")?, str_arg(args, "role")?),
        };
        let ctx = store.get_context(&agent, &role).await?;
        if session.key.is_none() && delegation.is_none() {
            session.declared = Some((agent, role));
        }
        return to_value(ctx);
    }
    // Who this write is recorded as, and what it is allowed to touch.
    // The store enforces the scope; the dispatcher only carries it.
    let actor = match &delegation {
        Some(d) => Actor::delegated(&d.agent_name, &d.module_id),
        None => Actor::new(session.agent().ok_or_else(|| {
            McpmError::new(
                ErrorCode::NotRegistered,
                "This session has no identity yet.",
                Value::Null,
                "Call get_context(agent_name, role) first — every other tool attributes its \
                 writes to that identity.",
            )
        })?),
    };
    let agent = actor.name.clone();

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
        "mint_worker" => {
            let module_id = str_arg(args, "module_id")?;
            let agent_name = str_arg(args, "agent_name")?;
            let for_key_id = opt_str_arg(args, "for_key_id");
            let key_id = session.key.as_ref().map(|k| k.key_id.as_str());
            let req = MintRequest {
                module_id: &module_id,
                agent_name: &agent_name,
                ttl_minutes: args.get("ttl_minutes").and_then(Value::as_i64),
                for_key_id: for_key_id.as_deref(),
            };
            to_value(store.mint_worker(&actor, key_id, req).await?)
        }
        "issue_worker_key" => {
            let agent_name = str_arg(args, "agent_name")?;
            let label = opt_str_arg(args, "label");
            to_value(
                store
                    .issue_worker_key(&actor, &agent_name, label.as_deref())
                    .await?,
            )
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
            to_value(store.claim_module(&actor, &module_id).await?)
        }
        "complete_task" => {
            let task_id = str_arg(args, "task_id")?;
            let outcome: TaskOutcome = parse_field(args, "outcome")?;
            let note = opt_str_arg(args, "note");
            to_value(
                store
                    .complete_task(&actor, &task_id, outcome, note.as_deref())
                    .await?,
            )
        }
        "add_task" => {
            let module_id = str_arg(args, "module_id")?;
            let task_name = str_arg(args, "name")?;
            let note = opt_str_arg(args, "note");
            to_value(
                store
                    .add_task(&actor, &module_id, &task_name, note.as_deref())
                    .await?,
            )
        }
        "complete_module" => {
            let module_id = str_arg(args, "module_id")?;
            let summary = str_arg(args, "summary")?;
            let used: Vec<String> = json_field(args, "used_memories")?.unwrap_or_default();
            to_value(store.complete_module(&actor, &module_id, &summary, &used).await?)
        }
        "report_blocker" => {
            let module_id = str_arg(args, "module_id")?;
            let description = str_arg(args, "description")?;
            let misled: Vec<String> = json_field(args, "misled_by")?.unwrap_or_default();
            to_value(store.report_blocker(&actor, &module_id, &description, &misled).await?)
        }
        "release_module" => {
            let module_id = str_arg(args, "module_id")?;
            let reason = str_arg(args, "reason")?;
            to_value(store.release_module(&actor, &module_id, &reason).await?)
        }
        "commit_memory" => {
            let scope: MemoryScope = parse_field(args, "scope")?;
            let kind: MemoryKind = match args.get("kind") {
                None | Some(Value::Null) => MemoryKind::Note,
                Some(kind) => serde_json::from_value(kind.clone()).map_err(bad_args)?,
            };
            let content = str_arg(args, "content")?;
            let tags: Vec<String> = args
                .get("tags")
                .map(|t| serde_json::from_value(t.clone()))
                .transpose()
                .map_err(bad_args)?
                .unwrap_or_default();
            let supersedes: Vec<Supersede> = json_field(args, "supersedes")?.unwrap_or_default();
            to_value(
                store
                    .commit_memory(&agent, scope, kind, &content, &tags, &supersedes)
                    .await?,
            )
        }
        "search_memory" => {
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
            let query = MemoryQuery {
                text: opt_str_arg(args, "query").unwrap_or_default(),
                kinds: json_field(args, "kinds")?.unwrap_or_default(),
                tags: json_field(args, "tags")?.unwrap_or_default(),
                author: opt_str_arg(args, "author"),
                since: rfc3339(args, "since")?,
                until: rfc3339(args, "until")?,
                scope,
                direction,
                limit: args.get("limit").and_then(Value::as_i64).unwrap_or(20),
                offset: 0,
                include_superseded: args
                    .get("include_superseded")
                    .and_then(Value::as_bool)
                    .unwrap_or(false),
                include_refuted: false,
            };
            // Agents get the hits; `total` is the console's pager
            // business, and a count they cannot page through would only
            // invite them to ask for more than the limit they set.
            to_value(store.search_memory(&query).await?.hits)
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
        "touch_memory" => {
            let ids: Vec<String> = parse_field(args, "memory_ids")?;
            let note = opt_str_arg(args, "note").unwrap_or_default();
            to_value(store.touch_memories(&agent, &ids, &note).await?)
        }
        "confirm_memory" => {
            let id = str_arg(args, "memory_id")?;
            let note = opt_str_arg(args, "note").unwrap_or_default();
            to_value(store.confirm_memory(&agent, &id, &note).await?)
        }
        "dispute_memory" => {
            let id = str_arg(args, "memory_id")?;
            let reason = str_arg(args, "reason")?;
            to_value(store.dispute_memory(&agent, &id, &reason).await?)
        }
        "relate_memories" => {
            let from = str_arg(args, "from_memory_id")?;
            let to = str_arg(args, "to_memory_id")?;
            let kind: EdgeKind = parse_field(args, "kind")?;
            let rationale = opt_str_arg(args, "rationale").unwrap_or_default();
            to_value(store.relate_memories(&agent, &from, &to, kind, &rationale).await?)
        }
        "memory_history" => {
            let id = str_arg(args, "memory_id")?;
            let belief_only = args
                .get("belief_only")
                .and_then(Value::as_bool)
                .unwrap_or(true);
            to_value(store.memory_history(&id, belief_only).await?)
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

/// Pull `delegation_token` out of the arguments and resolve it, or
/// `None` when the caller did not present one.
///
/// An unresolvable token is an ERROR, never a silent fall-back to the
/// session's own identity: falling back would attribute a subagent's
/// work to the machine — quietly, and exactly in the case the token
/// exists to prevent.
async fn resolve_delegation(
    store: &Store,
    session: &Session,
    args: &Value,
) -> Result<Option<Delegation>, McpmError> {
    let Some(token) = args
        .get("delegation_token")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|t| !t.is_empty())
    else {
        return Ok(None);
    };
    let key_id = session.key.as_ref().map(|k| k.key_id.as_str());
    store.resolve_delegation(key_id, token).await.map(Some)
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
            "uri": "project://knowledge",
            "name": "Knowledge base",
            "description": "What this crew knows: conventions, decisions, gotchas and \
                outcomes, newest first. Search it with search_memory rather than reading \
                it whole.",
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
    } else if uri == "project://knowledge" {
        serde_json::to_value(
            store
                .search_memory(&mcpm_core::MemoryQuery { limit: 200, ..Default::default() })
                .await?,
        )
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

/// An optional JSON-typed argument: absent and `null` both mean "not
/// given", which is what an agent omitting a filter looks like on the
/// wire.
fn json_field<T: serde::de::DeserializeOwned>(
    args: &Value,
    key: &str,
) -> Result<Option<T>, McpmError> {
    match args.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(v) => serde_json::from_value(v.clone()).map(Some).map_err(bad_args),
    }
}

/// An optional RFC 3339 instant. Rejected loudly rather than silently
/// ignored: a date filter that quietly did nothing would hand back a
/// wider answer than the caller asked for, and they would believe it.
fn rfc3339(args: &Value, key: &str) -> Result<Option<chrono::DateTime<chrono::Utc>>, McpmError> {
    let Some(raw) = opt_str_arg(args, key).filter(|s| !s.trim().is_empty()) else {
        return Ok(None);
    };
    chrono::DateTime::parse_from_rfc3339(raw.trim())
        .map(|t| Some(t.with_timezone(&chrono::Utc)))
        .map_err(|e| {
            McpmError::new(
                ErrorCode::PlanInvalid,
                format!("'{key}' is not an RFC 3339 instant: {e}"),
                Value::Null,
                "Send something like 2026-09-01T00:00:00Z, or omit the argument to not filter \
                 by date.",
            )
        })
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
