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

/// `plan_feature`: the whole graph, created atomically.
///
/// A feature is a set of modules and the edges between them. `modules`
/// is the shape; `stages` is the old ladder, still accepted and lowered
/// to edges (each module depends on every module of the preceding
/// stage) so prompts written against it keep working. Giving both is
/// refused.
#[derive(Debug, Default, Deserialize, Serialize)]
pub struct PlanFeature {
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub modules: Vec<PlanModule>,
    /// DEPRECATED: the stage ladder, lowered to edges on the way in.
    #[serde(default)]
    pub stages: Vec<PlanStage>,
    /// The plan as prose — what the manager would say to a new hire.
    /// Stored as the feature's whitepaper document, revision 1.
    #[serde(default)]
    pub whitepaper: Option<String>,
}

/// One rung of the deprecated stage ladder.
#[derive(Debug, Deserialize, Serialize)]
pub struct PlanStage {
    pub name: String,
    pub modules: Vec<PlanModule>,
}

#[derive(Debug, Default, Deserialize, Serialize)]
pub struct PlanModule {
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub tasks: Vec<String>,
    /// Prerequisites, by module NAME within this plan (ids do not exist
    /// yet). Every one must be done before this module can be claimed.
    #[serde(default)]
    pub depends_on: Vec<String>,
    /// Path prefixes this module will write to. Optional; two modules
    /// that overlap must be ordered by an edge or the plan is refused.
    #[serde(default)]
    pub owns: Vec<String>,
}

/// `revise_plan` batch operations, applied atomically or not at all.
#[derive(Debug, Deserialize, Serialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum PlanOp {
    AddModule {
        name: String,
        #[serde(default)]
        description: String,
        #[serde(default)]
        tasks: Vec<String>,
        /// Prerequisites by module ID (the feature already exists).
        #[serde(default)]
        depends_on: Vec<String>,
        #[serde(default)]
        owns: Vec<String>,
    },
    AddTask {
        module_id: String,
        name: String,
        #[serde(default)]
        note: Option<String>,
    },
    /// `module_id` waits on `depends_on`. Refused when the dependent has
    /// already started (that rewrites the decision that let it start),
    /// when it would close a cycle, or when it crosses features.
    AddDependency { module_id: String, depends_on: String },
    /// Drop an edge. Only ever loosens; the ownership overlap check runs
    /// again because the edge may have been what ordered two writers.
    RemoveDependency { module_id: String, depends_on: String },
    /// Change a module's description and/or owned paths. Names go
    /// through `rename`.
    UpdateModule {
        id: String,
        #[serde(default)]
        description: Option<String>,
        #[serde(default)]
        owns: Option<Vec<String>>,
    },
    /// Rename any tree node by id.
    Rename { id: String, name: String },
    /// Change the feature's own description. Its name goes through
    /// `rename` with the feature id.
    UpdateFeature { description: String },
    /// Remove a module/task. Refused when it holds completed or
    /// in-progress work. Removing a module drops its edges; a dependent
    /// simply loses that prerequisite.
    Remove { id: String },
}

/// Which long-form document is meant.
#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum DocumentKind {
    /// The feature's plan as prose. Feature scope, written by the
    /// planner, revised as the plan moves.
    Whitepaper,
    /// How to use what a module built — the component, the function,
    /// its parameters, where it lives. Module scope, written by the
    /// claim holder for whoever continues the work.
    Handoff,
}

impl DocumentKind {
    pub fn as_str(self) -> &'static str {
        match self {
            DocumentKind::Whitepaper => "whitepaper",
            DocumentKind::Handoff => "handoff",
        }
    }

    pub fn parse(s: &str) -> Option<DocumentKind> {
        match s {
            "whitepaper" => Some(DocumentKind::Whitepaper),
            "handoff" => Some(DocumentKind::Handoff),
            _ => None,
        }
    }

    /// The scope level a kind lives at.
    pub fn level(self) -> Level {
        match self {
            DocumentKind::Whitepaper => Level::Feature,
            DocumentKind::Handoff => Level::Module,
        }
    }
}

/// One revision of a document. `revision` counts from 1 per subject
/// and kind; the newest is the current one.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DocumentView {
    pub id: String,
    pub level: String,
    pub subject_id: String,
    pub subject_name: String,
    pub kind: DocumentKind,
    pub revision: i32,
    pub title: String,
    /// Markdown.
    pub body: String,
    pub author: String,
    pub created_at: DateTime<Utc>,
}

/// One file attached to a feature or a want, as every reader sees it.
///
/// The bytes are elsewhere (see `FileProvider`); this is the record,
/// and the `description` is the part written for an agent: what the
/// file is and why it is here, so a worker can decide whether to fetch
/// it at all.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct AttachmentView {
    pub id: String,
    /// `feature` | `want`
    pub level: String,
    pub subject_id: String,
    /// The feature's name, or the want's body.
    pub subject_name: String,
    /// The file name as attached.
    pub name: String,
    pub description: String,
    pub content_type: String,
    pub size_bytes: i64,
    pub sha256: String,
    pub added_by: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    /// Set on a FEATURE's listing for a file that reached it through
    /// one of the wants it was composed from. `None` for the feature's
    /// own files, and on a want's own listing.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub via_want: Option<WantOrigin>,
    /// Set on a FEATURE's listing for a file attached to one of its
    /// modules.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub via_module: Option<ModuleOrigin>,
    /// The comment this file arrived with, when it did.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub comment: Option<CommentOrigin>,
}

/// A module named from an attachment listing.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct ModuleOrigin {
    pub id: String,
    pub name: String,
}

/// The comment a file came in with: who said what, briefly.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct CommentOrigin {
    pub id: String,
    pub author: String,
    /// The comment's first line or so.
    pub excerpt: String,
}

/// A file handed over inline — by an agent's tool call, or by the
/// console's multipart post — on its way to `attach_file` or a comment.
#[derive(Clone, Debug)]
pub struct InlineFile {
    pub name: String,
    pub description: String,
    pub content_type: String,
    pub bytes: Vec<u8>,
}

/// What a comment is.
#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum CommentKind {
    /// A remark on the record.
    Note,
    /// Names who owes an answer, and blocks its subject until one lands.
    Question,
    /// Resolves the question it points at.
    Answer,
}

impl CommentKind {
    pub fn as_str(self) -> &'static str {
        match self {
            CommentKind::Note => "note",
            CommentKind::Question => "question",
            CommentKind::Answer => "answer",
        }
    }

    pub fn parse(s: &str) -> Option<CommentKind> {
        match s {
            "note" => Some(CommentKind::Note),
            "question" => Some(CommentKind::Question),
            "answer" => Some(CommentKind::Answer),
            _ => None,
        }
    }
}

/// One comment in a discussion, with the files it carried.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct CommentView {
    pub id: String,
    /// `feature` | `want` | `module`
    pub level: String,
    pub subject_id: String,
    pub subject_name: String,
    pub kind: CommentKind,
    /// Markdown.
    pub body: String,
    pub author: String,
    /// A question: who owes the answer. `None` means anyone may.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub assigned_to: Option<String>,
    /// An answer: the question it resolves.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub answers: Option<String>,
    /// A question: whether an answer has landed.
    pub resolved: bool,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub resolved_by: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub resolved_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub edited_at: Option<DateTime<Utc>>,
    #[serde(default)]
    pub attachments: Vec<AttachmentView>,
}

/// An open question, named from the thing it blocks or the agent it
/// awaits.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct QuestionRef {
    pub id: String,
    /// `feature` | `want` | `module`
    pub level: String,
    pub subject_id: String,
    pub subject_name: String,
    /// The feature a module's question sits in; the feature itself
    /// for a feature's; `None` for a want's.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub feature_id: Option<String>,
    pub body: String,
    pub author: String,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub assigned_to: Option<String>,
    pub created_at: DateTime<Utc>,
}

/// A want named from an attachment listing: which idea a file came in
/// through.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct WantOrigin {
    pub id: String,
    pub body: String,
}

/// `complete_task` outcome.
#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TaskOutcome {
    Done,
    Skipped,
}

/// A memory scope: one node of the tree, or the project above it.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct MemoryScope {
    pub level: Level,
    /// The node's id. Optional in the wire form and ignored entirely at
    /// project level, which is a singleton — an agent writes
    /// `{"level": "project"}` and the store fills in the rest, because
    /// making it quote an id it can neither discover nor vary would be
    /// ceremony with no content.
    #[serde(default)]
    pub id: String,
}

impl MemoryScope {
    /// The whole project's shelf.
    pub fn project() -> MemoryScope {
        MemoryScope {
            level: Level::Project,
            id: crate::ids::PROJECT_SUBJECT.to_string(),
        }
    }

    /// Fill in what the caller may leave out. Called on every scope the
    /// store accepts, so the singleton's id is settled in one place.
    pub fn normalized(mut self) -> MemoryScope {
        if self.level == Level::Project {
            self.id = crate::ids::PROJECT_SUBJECT.to_string();
        }
        self
    }
}

/// What a memory IS, as distinct from what it is about.
///
/// A closed set on purpose. It is enumerated in the MCP tool schema, so
/// an agent sees the whole vocabulary and picks from it rather than
/// inventing a label — which is what keeps the facet queryable. The
/// same discipline the tag registry applies to tags.
#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum MemoryKind {
    /// A standing rule: how this project does something.
    Convention,
    /// A choice made and the reasoning that settled it.
    Decision,
    /// A trap: something that looks fine and is not.
    Gotcha,
    /// What actually happened when work was done.
    Outcome,
    /// A pointer outward — a doc, a ticket, a dashboard.
    Reference,
    /// Unclassified. What every memory written before kinds existed
    /// backfilled to, and what an agent that did not say gets: honestly
    /// different from claiming one of the others.
    #[default]
    Note,
}

impl MemoryKind {
    pub fn as_str(self) -> &'static str {
        match self {
            MemoryKind::Convention => "convention",
            MemoryKind::Decision => "decision",
            MemoryKind::Gotcha => "gotcha",
            MemoryKind::Outcome => "outcome",
            MemoryKind::Reference => "reference",
            MemoryKind::Note => "note",
        }
    }

    pub fn parse(s: &str) -> Option<MemoryKind> {
        match s {
            "convention" => Some(MemoryKind::Convention),
            "decision" => Some(MemoryKind::Decision),
            "gotcha" => Some(MemoryKind::Gotcha),
            "outcome" => Some(MemoryKind::Outcome),
            "reference" => Some(MemoryKind::Reference),
            "note" => Some(MemoryKind::Note),
            _ => None,
        }
    }

    /// Every kind, for the tool schema and the console's filter rail.
    pub const ALL: [MemoryKind; 6] = [
        MemoryKind::Convention,
        MemoryKind::Decision,
        MemoryKind::Gotcha,
        MemoryKind::Outcome,
        MemoryKind::Reference,
        MemoryKind::Note,
    ];
}

/// A claim an agent makes about a memory.
///
/// Never written by a search: use is attested, not observed. See
/// KNOWLEDGE.md, "Use is attested, not observed".
#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SignalKind {
    /// "I used this." Cheap to give, weak evidence of truth.
    Touch,
    /// "I checked this and it holds."
    Confirm,
    /// "This is wrong."
    Dispute,
}

impl SignalKind {
    pub fn as_str(self) -> &'static str {
        match self {
            SignalKind::Touch => "touch",
            SignalKind::Confirm => "confirm",
            SignalKind::Dispute => "dispute",
        }
    }
}

/// How one memory supersedes another.
///
/// Split into "the belief changed" (`Replaces`, `Refutes`) and "the
/// wording changed" (`Revises`, `Consolidates`), because those answer
/// different questions about the history.
#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum EdgeKind {
    /// The fact changed — the world moved.
    #[default]
    Replaces,
    /// It was wrong when written — we were wrong.
    Refutes,
    /// Same fact, better words or corrected tags. Clerical.
    Revises,
    /// Several entries folded into one.
    Consolidates,

    // --- Standing relations. These describe; they retire nothing. ---
    /// A narrower case of a broader rule.
    Refines,
    /// True only because the target is. Read the other way, this is the
    /// blast radius of changing the target.
    DependsOn,
    /// Disagrees with the target, and neither has won yet. A flag for a
    /// human — never averaged away.
    Contradicts,
    /// Plain association. The weakest claim, and what a confirmed
    /// suggestion becomes when nobody says anything stronger.
    RelatesTo,
}

impl EdgeKind {
    pub fn as_str(self) -> &'static str {
        match self {
            EdgeKind::Replaces => "replaces",
            EdgeKind::Refutes => "refutes",
            EdgeKind::Revises => "revises",
            EdgeKind::Consolidates => "consolidates",
            EdgeKind::Refines => "refines",
            EdgeKind::DependsOn => "depends_on",
            EdgeKind::Contradicts => "contradicts",
            EdgeKind::RelatesTo => "relates_to",
        }
    }

    /// Whether this edge RETIRES its target. Exactly the four
    /// supersession kinds; everything else describes without withdrawing.
    pub fn supersedes(self) -> bool {
        matches!(
            self,
            EdgeKind::Replaces
                | EdgeKind::Refutes
                | EdgeKind::Revises
                | EdgeKind::Consolidates
        )
    }

    pub fn parse(s: &str) -> Option<EdgeKind> {
        match s {
            "replaces" => Some(EdgeKind::Replaces),
            "refutes" => Some(EdgeKind::Refutes),
            "revises" => Some(EdgeKind::Revises),
            "consolidates" => Some(EdgeKind::Consolidates),
            "refines" => Some(EdgeKind::Refines),
            "depends_on" => Some(EdgeKind::DependsOn),
            "contradicts" => Some(EdgeKind::Contradicts),
            "relates_to" => Some(EdgeKind::RelatesTo),
            _ => None,
        }
    }

    /// Whether this edge marks a change of BELIEF rather than of
    /// wording. `history` collapses the clerical ones by default: asking
    /// "what did we used to think" and getting three rephrasings of one
    /// idea buries the one place the idea actually changed.
    pub fn changes_belief(self) -> bool {
        matches!(self, EdgeKind::Replaces | EdgeKind::Refutes)
    }
}

/// One supersession declared at commit time.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Supersede {
    /// The memory being superseded.
    pub memory_id: String,
    #[serde(default)]
    pub kind: EdgeKind,
    /// Why. Carried on the edge, not on either memory.
    #[serde(default)]
    pub rationale: String,
}

/// A memory's state, DERIVED from its edges and signals — never stored,
/// so it cannot drift from the evidence.
#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum MemoryState {
    /// Nothing supersedes it and nobody is disputing it.
    #[default]
    Current,
    /// Something supersedes it. Still searchable; out of default results.
    Superseded,
    /// More agents say it is wrong than say it holds, and nothing has
    /// replaced it yet. A manager's problem.
    Disputed,
}

impl MemoryState {
    pub fn as_str(self) -> &'static str {
        match self {
            MemoryState::Current => "current",
            MemoryState::Superseded => "superseded",
            MemoryState::Disputed => "disputed",
        }
    }

    /// Whether a default search returns it.
    pub fn in_default_results(self) -> bool {
        matches!(self, MemoryState::Current)
    }
}

/// The evidence behind one memory's rank, decomposed.
///
/// Returned rather than reduced to a number on purpose: "stale, and two
/// agents dispute it" is actionable where `0.31` is not, and every part
/// of it is reconstructible from rows anyone can read.
#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, PartialEq)]
pub struct Standing {
    /// Distinct agents that used it.
    pub touches: i64,
    /// Distinct agents that verified it.
    pub confirms: i64,
    /// Distinct agents that say it is wrong.
    pub disputes: i64,
    /// Fraction of same-kind memories written after this one, 0..1.
    /// The decay clock: how much the project has learned since.
    pub newer_fraction: f32,
    /// `tanh(k * weighted signal sum)`, in -1..1.
    pub evidence: f32,
    /// The rank multiplier this resolves to, never below the floor.
    pub multiplier: f32,
}

/// One query against the knowledge base.
///
/// Every field narrows; none widens. An empty query with no filters is
/// "everything, newest first", and each facet added removes rows rather
/// than adding them — so a caller can reason about a query by reading
/// it top to bottom, and a filter can never surprise them by pulling in
/// something the previous line excluded.
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct MemoryQuery {
    /// Free text. Matched against content by three routes at once —
    /// exact phrase, expanded lexemes, and trigram similarity — and
    /// used to rank. Empty means "do not filter by text".
    #[serde(default)]
    pub text: String,
    /// Kinds to include. Empty means every kind.
    #[serde(default)]
    pub kinds: Vec<MemoryKind>,
    /// Tags the memory must carry. Every one of them (AND, not OR —
    /// filters narrow).
    #[serde(default)]
    pub tags: Vec<String>,
    /// Only memories written by this agent.
    #[serde(default)]
    pub author: Option<String>,
    /// Only memories written at or after this instant.
    #[serde(default)]
    pub since: Option<DateTime<Utc>>,
    /// Only memories written before this instant.
    #[serde(default)]
    pub until: Option<DateTime<Utc>>,
    /// The anchor node. `None` searches the whole project.
    #[serde(default)]
    pub scope: Option<MemoryScope>,
    /// Which way to walk the tree from the anchor.
    #[serde(default)]
    pub direction: SearchDirection,
    #[serde(default)]
    pub limit: i64,
    /// Rows to skip — the console's pager. Agents leave it at 0.
    #[serde(default)]
    pub offset: i64,
    /// Include superseded entries whose successor REFUTED them.
    ///
    /// Narrower than `include_superseded` and used by the claim
    /// briefing: "we tried this and it was wrong" is what stops a fresh
    /// agent proposing it again, while a plain replacement has a
    /// successor that says everything needed. A dead end is knowledge.
    #[serde(default)]
    pub include_refuted: bool,
    /// Include superseded and disputed entries. Off by default: the
    /// current answer is what a caller almost always wants. On, this is
    /// how you read what the project used to believe — nothing is ever
    /// deleted, so the history is always there to ask for.
    #[serde(default)]
    pub include_superseded: bool,
}

/// A search result: the memory plus why it surfaced.
#[derive(Debug, Serialize, Deserialize)]
pub struct MemoryHit {
    #[serde(flatten)]
    pub memory: Memory,
    /// Combined lexical + fuzzy score. Comparable within one result
    /// set, meaningless across two — it is a sort key, not a
    /// percentage, and presenting it as one would invite a reader to
    /// draw conclusions it cannot support.
    pub relevance: f32,
}

/// One step of a memory's lineage.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct HistoryStep {
    pub memory: Memory,
    /// How this step relates to the one before it. `None` on the entry
    /// the walk started from.
    pub via: Option<EdgeKind>,
    /// Why that edge exists.
    pub rationale: String,
}

/// One standing relation, seen from a memory.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Relation {
    pub kind: EdgeKind,
    /// Whether this memory is the SOURCE of the edge. `refines` read
    /// outward is "this refines that"; read inward it is "that refines
    /// this", and the two mean opposite things.
    pub outgoing: bool,
    pub rationale: String,
    pub author: String,
    pub other: Memory,
}

/// A relation the graph has not been told about, inferred from agents
/// having leaned on both memories in the same breath.
///
/// A SUGGESTION, never an assertion: co-use is the weakest evidence a
/// relation exists, and a graph that asserted them would fill with
/// correlations wearing the same clothes as declared knowledge.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Suggestion {
    pub other: Memory,
    /// Distinct agents that used both in one `touch_memory` call.
    pub co_touches: i64,
}

/// A memory's lineage, both directions from the anchor.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct MemoryHistory {
    /// The entry asked about.
    pub anchor: Memory,
    /// What it superseded, and what those superseded, oldest last.
    pub supersedes: Vec<HistoryStep>,
    /// What superseded it, newest last. Empty when it is current.
    pub superseded_by: Vec<HistoryStep>,
    /// Declared standing relations, both directions.
    #[serde(default)]
    pub relations: Vec<Relation>,
    /// Undeclared relations the co-touch record hints at.
    #[serde(default)]
    pub suggestions: Vec<Suggestion>,
}

/// A page of results plus the total the filters matched, so a pager can
/// say "1-20 of 142" without a second round trip.
#[derive(Debug, Serialize, Deserialize)]
pub struct MemoryPage {
    pub hits: Vec<MemoryHit>,
    pub total: i64,
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

impl SearchDirection {
    pub fn as_str(self) -> &'static str {
        match self {
            SearchDirection::Here => "here",
            SearchDirection::Up => "up",
            SearchDirection::Down => "down",
            SearchDirection::All => "all",
        }
    }
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
    pub modules_done: i64,
    pub modules_total: i64,
    /// Modules whose prerequisites are all done and nobody holds.
    pub modules_ready: i64,
    /// Modules an agent holds right now (`in_progress`).
    pub modules_running: i64,
    pub tasks_done: i64,
    pub tasks_total: i64,
    /// Tasks the crew added while working, beyond the plan.
    pub tasks_added: i64,
    /// The planning manager, as recorded at plan time.
    pub created_by: Option<String>,
    /// When the ledger first and last mention this feature. `None` on a
    /// feature nothing has happened to.
    pub started: Option<DateTime<Utc>>,
    pub last_activity: Option<DateTime<Utc>>,
    /// The newest ledger write by any agent currently HOLDING a module
    /// here, on any subject. This — not `last_activity` — is whether
    /// the feature is moving: a manager's plan revision is activity
    /// on the feature, but only a holder's write says a box is alive.
    pub last_heard: Option<DateTime<Utc>>,
    /// The newest announcement anywhere in the feature — on it or on
    /// one of its modules — so a board reads what the crew is doing
    /// without opening the ledger.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_word: Option<Announcement>,
}

/// One thing across the project that has stopped and is waiting on a
/// person: a claim the gate refused, or a module its worker escalated.
///
/// Every row is a CURRENT condition, derived from the module's own
/// state — it disappears by itself the moment the work moves.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AttentionItem {
    pub feature_id: String,
    pub feature_name: String,
    pub module_id: String,
    pub module_name: String,
    /// `rejected` — unclaimed, not done, and a claim on it once bounced
    /// off the gate; or `blocked` — its worker reported a blocker.
    pub kind: String,
    /// Names of the prerequisites still open. Empty means the gate is
    /// open now.
    pub waiting_on: Vec<String>,
}

/// The ledger entries that fix a module's displayed state: when it was
/// first claimed, and the latest gate rejection and blocker on it.
#[derive(Debug, Serialize, Deserialize)]
pub struct ModuleMilestones {
    pub module_id: String,
    pub first_claim: Option<DateTime<Utc>>,
    pub last_rejection: Option<Event>,
    pub last_blocker: Option<Event>,
    /// The last time the agent holding this module wrote anything to
    /// the ledger, on any subject. `None` when nobody holds it, or the
    /// holder has never written. This is the liveness signal: a claim
    /// says a module is held, this says whether it is moving.
    pub last_activity: Option<DateTime<Utc>>,
}

/// The pool's shape: how many ideas sit in each state.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize)]
pub struct WantCounts {
    pub open: i64,
    pub promoted: i64,
    pub declined: i64,
}

/// One page of the pool, with what the filters matched in total.
#[derive(Debug, Serialize, Deserialize)]
pub struct WantSearch {
    pub wants: Vec<Want>,
    pub total: i64,
}

/// One committed event, as announced over LISTEN/NOTIFY.
///
/// Carries enough to decide what to refetch — which feature, what
/// kind of write — and nothing a listener would render: the console
/// re-reads the store rather than trusting a payload.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct EventNotice {
    pub seq: i64,
    #[serde(rename = "type", default)]
    pub kind: String,
    #[serde(default)]
    pub feature_id: Option<String>,
    #[serde(default)]
    pub subject_id: Option<String>,
}

impl EventNotice {
    /// Decode a notification payload: the JSON object the trigger sends
    /// since migration 0013, or the bare `seq` it sent before — a
    /// database migrated while a listener was up must not stall it.
    pub fn parse(payload: &str) -> Option<EventNotice> {
        let payload = payload.trim();
        if let Ok(seq) = payload.parse::<i64>() {
            return Some(EventNotice { seq, ..Default::default() });
        }
        serde_json::from_str(payload).ok()
    }
}

/// Full graph for one feature.
#[derive(Debug, Serialize, Deserialize)]
pub struct FeatureTree {
    pub id: String,
    pub name: String,
    pub description: String,
    pub status: String,
    pub summary: Option<String>,
    /// Topological order: every module after all of its prerequisites,
    /// ties broken by depth then name.
    pub modules: Vec<ModuleView>,
    /// The current whitepaper, when one was written.
    pub whitepaper: Option<DocumentView>,
    /// Files attached to the feature, and to the wants it was composed
    /// from, oldest first. Descriptions only — the bytes are fetched
    /// per file, by id.
    #[serde(default)]
    pub attachments: Vec<AttachmentView>,
    /// Open questions on the feature itself. While any is open nothing
    /// in the feature is dispatchable.
    #[serde(default)]
    pub open_questions: Vec<QuestionRef>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ModuleView {
    pub id: String,
    pub name: String,
    pub description: String,
    pub status: String,
    pub claimed_by: Option<String>,
    pub summary: Option<String>,
    /// Prerequisite module ids.
    pub depends_on: Vec<String>,
    /// Prerequisites not yet done — empty means the gate is open.
    pub waiting_on: Vec<String>,
    /// Path prefixes this module writes to; empty = undeclared.
    pub owns: Vec<String>,
    /// Longest path from a root, 1-based. Derived; a display hint.
    pub depth: i32,
    /// Derived: todo + unclaimed + every prerequisite done + no open
    /// question on it or on its feature.
    pub dispatchable: bool,
    /// Open questions on this module (the feature's are on the tree).
    #[serde(default)]
    pub open_questions: Vec<QuestionRef>,
    /// The latest thing an agent announced on this module, if anything.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_word: Option<Announcement>,
    pub tasks: Vec<TaskView>,
}

/// What an agent last said it was doing, in its own words. Derived from
/// the ledger (`type = 'announcement'`) at read time — the roster, a
/// module and a feature each carry the newest one that names them.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Announcement {
    pub text: String,
    /// Who said it.
    pub by: String,
    pub at: DateTime<Utc>,
    /// What it was said on: a module or a feature id.
    pub subject_id: String,
    /// That subject's name, for display.
    pub subject: String,
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
    /// Questions assigned to this agent that nobody has answered.
    /// Each blocks its subject until this agent (or a human) answers.
    #[serde(default)]
    pub awaiting_you: Vec<QuestionRef>,
    pub suggested_next: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct AgentIdentity {
    pub name: String,
    pub role: String,
    /// The URL this server probes to see whether your machine is up,
    /// if one is registered. Set it with `health_url` on `get_context`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub health_url: Option<String>,
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
    /// Prerequisite module ids — all done, or this would not be here.
    pub depends_on: Vec<String>,
    pub owns: Vec<String>,
    pub depth: i32,
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
    /// Open questions on the feature or its modules — each names who
    /// owes the answer. While any is open the frontier stays closed.
    #[serde(default)]
    pub pending: Vec<QuestionRef>,
    pub note: String,
}

/// `claim_module` result: the claim is also the worker's briefing.
#[derive(Debug, Serialize, Deserialize)]
pub struct Briefing {
    pub module: ModuleView,
    pub feature_id: String,
    pub feature_name: String,
    /// The feature's plan as prose, when the planner wrote one.
    pub whitepaper: Option<DocumentView>,
    /// Files attached to the feature (and to its source wants), each
    /// with the description its author wrote for a reader like this
    /// one. `read_attachment` fetches one by id.
    #[serde(default)]
    pub attachments: Vec<AttachmentView>,
    /// The newest comments on this module and its feature, oldest
    /// first — what people and agents have said about this work,
    /// questions and answers included.
    #[serde(default)]
    pub discussion: Vec<CommentView>,
    /// Memories at the module's ancestors (feature + project scope) —
    /// conventions and interface decisions recorded upstream.
    pub ancestor_memories: Vec<Memory>,
    /// Completed modules of this feature: transitive prerequisites
    /// first (`prerequisite: true`, in topological order), then the
    /// rest. Each carries its summary and its handoff document.
    pub upstream_summaries: Vec<UpstreamSummary>,
    /// How to work the checklist, said at the one moment the worker is
    /// setting its habits for this module. `complete_module` refuses
    /// with TASKS_OPEN, so ticking reads as an exit requirement and the
    /// gate sits at the end — advice framed as tidiness loses to that.
    /// Framed as durability it does not: the checklist is the only part
    /// of a worker that survives the worker.
    pub guidance: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct UpstreamSummary {
    pub module_id: String,
    pub module_name: String,
    /// Whether this module is on the claimed module's prerequisite
    /// chain. False for a completed sibling the plan did not order.
    pub prerequisite: bool,
    pub summary: String,
    /// The module's current handoff document, when its worker wrote one.
    pub handoff: Option<String>,
}

/// One memory (commit + search result).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Memory {
    pub id: String,
    pub level: String,
    pub subject_id: String,
    /// Name of the node the memory is pinned to.
    pub subject_name: String,
    /// What this memory is — convention, decision, gotcha, …
    pub kind: MemoryKind,
    pub content: String,
    pub tags: Vec<String>,
    pub author: String,
    pub created_at: DateTime<Utc>,
    /// Derived from edges + signals, never stored.
    pub state: MemoryState,
    /// The evidence behind its rank, decomposed.
    pub standing: Standing,
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

/// One agent on the roster with its live claim load.
///
/// The roster is the agents that are still part of the picture — a
/// live claim, a registered box that has not answered 404, or a voice
/// heard in the last day — not every name the ledger ever recorded;
/// `Store::agents_overview` holds the rule.
#[derive(Debug, Serialize, Deserialize)]
pub struct AgentOverview {
    pub name: String,
    pub role: String,
    pub active_claims: i64,
    /// Comma-joined names of the modules it currently holds.
    pub claim_names: String,
    /// The last time it registered or announced anything.
    pub last_seen: DateTime<Utc>,
    /// Its newest ledger write of any kind; `None` if it never wrote.
    pub last_activity: Option<DateTime<Utc>>,
    /// The health check registered for it, if any, with its verdict.
    pub health: Option<AgentHealth>,
    /// The last thing it announced, if anything.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_word: Option<Announcement>,
}

/// A registered health check and what the last probe made of it.
/// `state` is `None` between registration and the first sweep.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentHealth {
    pub url: String,
    pub state: Option<crate::HealthState>,
    /// The probe's one-line reason: `HTTP 503`, `timed out after 8s`.
    pub detail: String,
    pub checked_at: Option<DateTime<Utc>>,
    /// When the current state was first observed.
    pub since: Option<DateTime<Utc>>,
}

/// One agent the prober has to visit.
#[derive(Debug, Clone)]
pub struct HealthTarget {
    pub name: String,
    pub url: String,
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
    /// The status set this filter names; empty for `All`.
    pub fn statuses(self) -> Vec<String> {
        match self {
            WantFilter::All => Vec::new(),
            other => vec![other.as_str().to_string()],
        }
    }

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
