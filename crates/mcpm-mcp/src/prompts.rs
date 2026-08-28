//! MCP prompts: the operating protocol served over the same wire as the
//! data (architecture doc §4). An orchestrator spawning a manager
//! fetches `manager_briefing`; a manager spawning a worker fetches
//! `worker_briefing`. Update the server and every future agent is
//! briefed on the new rules.

use mcpm_core::{McpmError, Store};
use serde_json::{json, Value};

pub fn prompt_defs() -> Value {
    json!([
        {
            "name": "manager_briefing",
            "description": "System briefing for a manager agent that owns one feature end to \
                end: plan, dispatch stage by stage, watch the ledger, close.",
            "arguments": [
                { "name": "feature_id", "description": "Feature to resume (omit when planning a new one).", "required": false }
            ]
        },
        {
            "name": "compose_wants",
            "description": "Briefing for an agent that reads the raw want pool and composes \
                coherent groups of ideas into feature plans. Carries the live open wants.",
            "arguments": [
                { "name": "theme", "description": "Optional focus, e.g. 'billing' — only wants matching it are shown.", "required": false }
            ]
        },
        {
            "name": "worker_briefing",
            "description": "System briefing for a worker agent that owns one module: claim, \
                work the checklist, record what you learned, exit through one door.",
            "arguments": [
                { "name": "module_id", "description": "The module this worker is dispatched to.", "required": true },
                { "name": "agent_name", "description": "The agent name to register as (e.g. agent.mod.schema).", "required": false }
            ]
        }
    ])
}

pub async fn get_prompt(store: &Store, params: &Value) -> Result<Value, McpmError> {
    let name = params.get("name").and_then(Value::as_str).unwrap_or("");
    let args = params.get("arguments").cloned().unwrap_or(json!({}));
    match name {
        "manager_briefing" => manager_briefing(store, &args).await,
        "compose_wants" => compose_wants(store, &args).await,
        "worker_briefing" => worker_briefing(&args),
        other => Err(McpmError::not_found("prompt", other)),
    }
}

async fn manager_briefing(store: &Store, args: &Value) -> Result<Value, McpmError> {
    let feature_id = args.get("feature_id").and_then(Value::as_str);
    let rollups = store.rollups().await?;
    let mut board = String::new();
    for f in &rollups {
        board.push_str(&format!(
            "- {} ({}): {} — stages {}/{}, modules {}/{}, tasks {}/{}\n",
            f.name,
            f.id,
            f.status,
            f.stages_done,
            f.stages_total,
            f.modules_done,
            f.modules_total,
            f.tasks_done,
            f.tasks_total
        ));
    }
    if board.is_empty() {
        board.push_str("(no features planned yet)\n");
    }
    let resume = match feature_id {
        Some(id) => format!("You are resuming feature {id}.\n"),
        None => String::new(),
    };
    let text = format!(
        "You are a MANAGER agent on the mcpm project-management MCP server. You own ONE \
feature end to end. The server is the gatekeeper — it enforces stage order, exclusive \
claims, and checklist-proven completion — and you bring the loop.\n\n\
{resume}Current board:\n{board}\n\
Your loop:\n\
1. get_context(agent_name, role='manager') — register; resume any feature already in \
flight rather than replanning it.\n\
2. plan_feature — one atomic call creates the whole Stage → Module → Task tree. Stages \
run strictly in order; modules within a stage run concurrently, one worker subagent \
each; tasks are each module's checklist.\n\
3. Dispatch loop, until done: call next_work(feature_id) and spawn ONE worker subagent \
per dispatchable module (give each the worker_briefing prompt with its module_id). \
Never compute stage gating yourself — next_work already did. When workers return, poll \
feature_status(feature_id, events_since=<cursor>) and read the new events: completions, \
blockers, premature claims, discovered tasks, stage_unlocked. Dispatch the next wave.\n\
4. On blocker_reported or premature_claim: fix the plan (revise_plan), re-dispatch, or \
escalate to the human. A premature_claim event means YOUR dispatch was early.\n\
5. When every stage is done: complete_feature(feature_id, summary). The summary becomes \
a feature-scope memory.\n\n\
Record conventions and interface decisions with commit_memory at feature scope — \
workers read them via search_memory(direction='up') before writing code."
    );
    Ok(prompt_result("Manager operating protocol + live board.", &text))
}

/// The composition briefing: the live want pool plus the rules for
/// turning loose ideas into a feature plan. This is the step between
/// "someone wanted a thing" and "a crew is building it".
async fn compose_wants(store: &Store, args: &Value) -> Result<Value, McpmError> {
    let theme = args.get("theme").and_then(Value::as_str).unwrap_or("");
    let pool = store
        .list_wants(theme, mcpm_core::WantFilter::Open, &[], 100)
        .await?;
    let mut listing = String::new();
    for w in &pool.wants {
        let tags = if w.tags.is_empty() {
            String::new()
        } else {
            format!(" [{}]", w.tags.join(", "))
        };
        listing.push_str(&format!("- {} — {}{}\n", w.id, w.body, tags));
    }
    if listing.is_empty() {
        listing.push_str("(the pool holds no open wants right now)\n");
    }
    let focus = if theme.is_empty() {
        String::new()
    } else {
        format!("Filtered to the theme '{theme}'.\n")
    };
    let text = format!(
        "You are composing FEATURES out of the want pool on the mcpm MCP server.\n\n\
A want is a raw idea, captured however loosely someone said it. It is NOT a small \
feature, and your job is NOT to turn each one into a feature. Your job is to find the \
themes that run through several wants and propose the feature that satisfies them \
together.\n\n\
{focus}Open wants ({} of {} in the pool):\n{listing}\n\
How to work:\n\
1. get_context(agent_name, role='manager'). Read the wants above — and search the \
existing ones you may have missed with list_wants(query=...).\n\
2. Group them. A good group shares a user-visible outcome, not just a keyword. Two to \
six wants per feature is typical. A want that belongs in two features is fine — link it \
to both.\n\
3. Check what already exists: if a group extends a feature that is already planned, \
fold the wants into it with promote_wants(wants, feature_id=...) and revise_plan, rather \
than planning a duplicate.\n\
4. Propose the grouping to the human BEFORE writing it, in one short paragraph per \
feature: which wants, what the feature is, and what you had to assume. Loose ideas are \
ambiguous by nature — the assumptions are the part worth checking.\n\
5. On approval, call promote_wants once per feature, with `plan` (the full Stage → \
Module → Task tree) and a `rationale` on every want saying how you read it into that \
plan. The links, the plan, and a feature-scope origin memory commit together.\n\
6. A want that should not happen: update_want(state='declined', reason=...). Never \
silently drop one — an idea with no verdict gets re-proposed forever.\n\n\
Then hand the feature to a manager agent (manager_briefing) to dispatch.",
        pool.wants.len(),
        pool.open
    );
    Ok(prompt_result("Want-pool composition protocol + the live pool.", &text))
}

fn worker_briefing(args: &Value) -> Result<Value, McpmError> {
    let module_id = args
        .get("module_id")
        .and_then(Value::as_str)
        .unwrap_or("<module_id>");
    let agent_name = args
        .get("agent_name")
        .and_then(Value::as_str)
        .unwrap_or("agent.mod.<short-name>");
    let text = format!(
        "You are a WORKER agent on the mcpm project-management MCP server. You own exactly \
ONE module: {module_id}.\n\n\
Your loop:\n\
1. get_context(agent_name='{agent_name}', role='worker') then claim_module('{module_id}'). \
The claim IS your briefing: it returns your checklist, the memories recorded above your \
module, and the completed-module summaries from earlier stages.\n\
   - If claim_module returns STAGE_LOCKED: STOP. Do no work. Report the error to your \
manager and end your turn — the server has already recorded the premature_claim event.\n\
2. Before writing code, search_memory(scope={{level:'module', id:'{module_id}'}}, \
direction='up') for conventions and interfaces decided upstream.\n\
3. Work the checklist: complete_task each item as it lands (not in a batch at the end). \
When reality reveals work the plan missed, add_task it — it is recorded as discovered.\n\
4. commit_memory (module scope) anything the next agent will need: decisions, gotchas, \
interfaces exposed.\n\
5. Exit through exactly ONE door:\n\
   - complete_module(module_id, summary) — all tasks resolved; your summary is what \
downstream workers read.\n\
   - report_blocker(module_id, description) — you cannot proceed; keep the claim, stop.\n\
   - release_module(module_id, reason) — you must abandon; task states survive you.\n\
Never exit silently."
    );
    Ok(prompt_result("Worker operating protocol.", &text))
}

fn prompt_result(description: &str, text: &str) -> Value {
    json!({
        "description": description,
        "messages": [{
            "role": "user",
            "content": { "type": "text", "text": text }
        }]
    })
}
