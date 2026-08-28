//! Serde DTOs for every tool's inputs and outputs. These serialize
//! verbatim into MCP tool results, so field names are part of the
//! agent-facing contract; every entity mention carries both id and
//! name so agents never need a lookup call to write a readable message.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::ids::Level;

// ---------------------------------------------------------------------
// Inputs
// ---------------------------------------------------------------------

/// `plan_feature`: the whole tree, created atomically.
#[derive(Debug, Deserialize, Serialize)]
pub struct PlanFeature {
    pub name: String,
    #[serde(default)]
    pub description: String,
    pub stages: Vec<PlanStage>,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct PlanStage {
    pub name: String,
    pub modules: Vec<PlanModule>,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct PlanModule {
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub tasks: Vec<String>,
}

/// `revise_plan` batch operations, applied atomically or not at all.
#[derive(Debug, Deserialize, Serialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum PlanOp {
    /// Append a stage at the end of the pipeline (or after `after`, a
    /// stage id).
    AddStage {
        name: String,
        #[serde(default)]
        after: Option<String>,
    },
    AddModule {
        stage_id: String,
        name: String,
        #[serde(default)]
        description: String,
        #[serde(default)]
        tasks: Vec<String>,
    },
    AddTask {
        module_id: String,
        name: String,
        #[serde(default)]
        note: Option<String>,
    },
    /// Rename any tree node by id.
    Rename { id: String, name: String },
    /// Remove a stage/module/task. Refused when the subtree holds
    /// completed or in-progress work.
    Remove { id: String },
}

/// `complete_task` outcome.
#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TaskOutcome {
    Done,
    Skipped,
}

/// A memory scope: one node of the tree.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct MemoryScope {
    pub level: Level,
    pub id: String,
}

/// `search_memory` direction along the tree from the anchor scope.
#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SearchDirection {
    /// The anchor node only.
    Here,
    /// Anchor + its ancestors — a worker reading conventions.
    Up,
    /// Anchor + all descendants — a manager reading outcomes.
    Down,
    /// The whole project (the default when no scope is given).
    #[default]
    All,
}

// ---------------------------------------------------------------------
// Tree views
// ---------------------------------------------------------------------

#[derive(Debug, Serialize, Deserialize)]
pub struct ProjectInfo {
    pub name: String,
    pub repo_path: String,
    pub description: String,
}

/// One feature with rolled-up progress (`get_context`, `feature_status`).
#[derive(Debug, Serialize, Deserialize)]
pub struct FeatureRollup {
    pub id: String,
    pub name: String,
    pub status: String,
    pub stages_done: i64,
    pub stages_total: i64,
    pub modules_done: i64,
    pub modules_total: i64,
    pub tasks_done: i64,
    pub tasks_total: i64,
}

/// Full tree for one feature.
#[derive(Debug, Serialize, Deserialize)]
pub struct FeatureTree {
    pub id: String,
    pub name: String,
    pub description: String,
    pub status: String,
    pub summary: Option<String>,
    pub stages: Vec<StageView>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct StageView {
    pub id: String,
    pub name: String,
    pub position: i32,
    /// Derived: `locked` | `unlocked` | `done`.
    pub status: String,
    pub modules: Vec<ModuleView>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ModuleView {
    pub id: String,
    pub name: String,
    pub description: String,
    pub status: String,
    pub claimed_by: Option<String>,
    pub summary: Option<String>,
    /// Derived: todo + unclaimed + stage unlocked.
    pub dispatchable: bool,
    pub tasks: Vec<TaskView>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct TaskView {
    pub id: String,
    pub name: String,
    pub status: String,
    pub origin: String,
    pub note: Option<String>,
}

// ---------------------------------------------------------------------
// Tool results
// ---------------------------------------------------------------------

/// `get_context` result — built so a restarted agent can re-orient from
/// this single call.
#[derive(Debug, Serialize, Deserialize)]
pub struct Context {
    pub project: ProjectInfo,
    pub you: AgentIdentity,
    pub features: Vec<FeatureRollup>,
    /// Modules this agent currently holds a claim on.
    pub your_claims: Vec<ClaimRef>,
    /// Loose ideas waiting to be composed into features.
    pub open_wants: i64,
    pub suggested_next: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct AgentIdentity {
    pub name: String,
    pub role: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ClaimRef {
    pub module_id: String,
    pub module_name: String,
    pub feature_id: String,
    pub feature_name: String,
    pub open_tasks: i64,
}

/// `feature_status` result: the tree plus the events after the cursor.
#[derive(Debug, Serialize, Deserialize)]
pub struct FeatureStatus {
    pub feature: FeatureTree,
    pub events: Vec<Event>,
    /// Pass back as `events_since` on the next poll.
    pub events_cursor: i64,
}

/// One dispatchable module from `next_work`.
#[derive(Debug, Serialize, Deserialize)]
pub struct WorkItem {
    pub module_id: String,
    pub module_name: String,
    pub description: String,
    pub stage_id: String,
    pub stage_name: String,
    pub stage_position: i32,
    pub tasks: Vec<TaskView>,
    /// How many memories a worker would find searching `up` from here.
    pub relevant_memories: i64,
}

/// `next_work` result.
#[derive(Debug, Serialize, Deserialize)]
pub struct NextWork {
    pub feature_id: String,
    pub dispatchable: Vec<WorkItem>,
    /// When `dispatchable` is empty and the feature is unfinished:
    /// what's in flight or blocked, so the manager knows to wait.
    pub in_flight: Vec<ClaimRef>,
    pub blocked: Vec<String>,
    pub note: String,
}

/// `claim_module` result: the claim is also the worker's briefing.
#[derive(Debug, Serialize, Deserialize)]
pub struct Briefing {
    pub module: ModuleView,
    pub stage_id: String,
    pub stage_name: String,
    pub stage_position: i32,
    pub feature_id: String,
    pub feature_name: String,
    /// Memories at the module's ancestors (stage + feature scope) —
    /// conventions and interface decisions recorded upstream.
    pub ancestor_memories: Vec<Memory>,
    /// Completed-module summaries from earlier stages of this feature.
    pub upstream_summaries: Vec<UpstreamSummary>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct UpstreamSummary {
    pub module_id: String,
    pub module_name: String,
    pub stage_position: i32,
    pub summary: String,
}

/// One memory (commit + search result).
#[derive(Debug, Serialize, Deserialize)]
pub struct Memory {
    pub id: String,
    pub level: String,
    pub subject_id: String,
    /// Name of the node the memory is pinned to.
    pub subject_name: String,
    pub content: String,
    pub tags: Vec<String>,
    pub author: String,
    pub created_at: DateTime<Utc>,
}

/// One event ledger entry.
#[derive(Debug, Serialize, Deserialize)]
pub struct Event {
    pub seq: i64,
    pub ts: DateTime<Utc>,
    #[serde(rename = "type")]
    pub kind: String,
    pub feature_id: Option<String>,
    pub subject_id: Option<String>,
    pub agent: Option<String>,
    pub payload: serde_json::Value,
}

/// One registered agent with its live claim load (dashboard roster).
#[derive(Debug, Serialize, Deserialize)]
pub struct AgentOverview {
    pub name: String,
    pub role: String,
    pub active_claims: i64,
    /// Comma-joined names of the modules it currently holds.
    pub claim_names: String,
}

/// Generic acknowledgement carrying the follow-on facts an agent needs.
#[derive(Debug, Serialize, Deserialize)]
pub struct Ack {
    pub ok: bool,
    pub message: String,
    #[serde(skip_serializing_if = "serde_json::Value::is_null")]
    pub data: serde_json::Value,
}

impl Ack {
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            ok: true,
            message: message.into(),
            data: serde_json::Value::Null,
        }
    }

    pub fn with(message: impl Into<String>, data: serde_json::Value) -> Self {
        Self {
            ok: true,
            message: message.into(),
            data,
        }
    }
}

// ---------------------------------------------------------------------
// Wants — the idea pool
// ---------------------------------------------------------------------

/// One raw idea in the pool. A want is deliberately unstructured: it is
/// what someone wanted, not a plan for it. `status` is derived, never
/// held — `promoted` means at least one feature links to it.
#[derive(Debug, Serialize, Deserialize)]
pub struct Want {
    pub id: String,
    pub body: String,
    pub tags: Vec<String>,
    /// Derived: `open` | `promoted` | `declined`.
    pub status: String,
    pub decline_reason: Option<String>,
    pub author: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    /// Every feature that absorbed this want, oldest link first.
    pub features: Vec<WantLink>,
}

/// One want↔feature composition record.
#[derive(Debug, Serialize, Deserialize)]
pub struct WantLink {
    pub feature_id: String,
    pub feature_name: String,
    /// How this want was read into that feature.
    pub rationale: String,
}

/// `list_wants` filter over the derived status.
#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum WantFilter {
    /// Not yet in any feature, not declined — the composition input.
    #[default]
    Open,
    Promoted,
    Declined,
    All,
}

impl WantFilter {
    pub fn as_str(self) -> &'static str {
        match self {
            WantFilter::Open => "open",
            WantFilter::Promoted => "promoted",
            WantFilter::Declined => "declined",
            WantFilter::All => "all",
        }
    }
}

/// `list_wants` result: the matching wants plus pool-wide counts, so a
/// composing agent sees the shape of the backlog in one call.
#[derive(Debug, Serialize, Deserialize)]
pub struct WantPool {
    pub open: i64,
    pub promoted: i64,
    pub declined: i64,
    pub wants: Vec<Want>,
    pub note: String,
}

/// `update_want`: every field optional — send only what changes.
#[derive(Debug, Default, Deserialize, Serialize)]
pub struct WantEdit {
    #[serde(default)]
    pub body: Option<String>,
    #[serde(default)]
    pub tags: Option<Vec<String>>,
    /// `open` reopens a declined want; `declined` requires `reason`.
    #[serde(default)]
    pub state: Option<WantState>,
    #[serde(default)]
    pub reason: Option<String>,
}

/// The only two states a want *holds* (promoted is derived from links).
#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum WantState {
    Open,
    Declined,
}

/// One want named in a promotion, with how it was interpreted.
#[derive(Debug, Deserialize, Serialize)]
pub struct WantRef {
    pub id: String,
    #[serde(default)]
    pub rationale: String,
}

/// `promote_wants`: compose a set of wants into a feature — either one
/// planned right here, or one that already exists. Exactly one of
/// `plan` / `feature_id` is given.
#[derive(Debug, Deserialize, Serialize)]
pub struct PromoteWants {
    pub wants: Vec<WantRef>,
    #[serde(default)]
    pub feature_id: Option<String>,
    #[serde(default)]
    pub plan: Option<PlanFeature>,
}

/// `promote_wants` result.
#[derive(Debug, Serialize, Deserialize)]
pub struct Promotion {
    pub feature: FeatureTree,
    /// The promoted wants, re-read (their status is now `promoted`).
    pub linked: Vec<Want>,
    pub note: String,
}

/// One tag in the registry, with how many wants carry it.
#[derive(Debug, Serialize, Deserialize)]
pub struct TagInfo {
    /// Normalized slug — what wants actually carry.
    pub name: String,
    /// How it was first typed, for display.
    pub label: String,
    pub uses: i64,
    pub created_by: String,
    pub created_at: DateTime<Utc>,
}

/// One want in a bulk capture.
#[derive(Debug, Default, Deserialize, Serialize)]
pub struct WantDraft {
    pub body: String,
    #[serde(default)]
    pub tags: Vec<String>,
}
