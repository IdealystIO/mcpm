//! Wire types + server functions for the Control Center console.
//!
//! The console (wasm) reads in tiers and renders whatever the store
//! holds — nothing is hardcoded client-side:
//!
//! - [`load_board`] is the one global read: a rollup row per feature,
//!   the attention list, the newest few events, the roster, the tag
//!   registry and the pool's counts. Scalars per feature, so it stays
//!   small however old the project gets. Refetched on every tick.
//! - [`load_feature`] is one feature's tree and whitepaper — fetched
//!   when it is selected, refetched when a tick names that feature.
//! - [`load_module`] is one module's handoff and history, for its
//!   drawer; [`load_events`] is one page of a ledger, newest first;
//!   [`search_wants`] is one page of the pool; [`load_want`] one idea.
//!
//! The earlier design shipped the whole project — every feature's
//! tree, every document body, five hundred events per feature — on
//! every event, and it grew without bound. What a view needs is now
//! what that view's read returns, and a [`Tick`] says which feature an
//! event touched so the client refetches the tree only when it has to.
//!
//! The server body (feature `server`) reads through `mcpm_core::Store`,
//! the same gatekeeper the MCP tools use, and pre-formats event
//! titles/bodies so the UI stays a dumb renderer.

pub mod capture;
#[cfg(feature = "server")]
pub mod files;

pub use capture::{parse_buffer, parse_line, scan_tags, tag_fragment_at, utf16_to_byte, TagSpan,
    WantDraftDto};

use serde::{Deserialize, Serialize};
use server::{server, subscription, ServerError};

// ---------------------------------------------------------------------
// The file routes
// ---------------------------------------------------------------------
//
// Attachment bytes do not ride a `#[server]` fn: those carry a JSON
// array of arguments, and a file is neither small nor JSON. They have
// two plain HTTP routes on the same host instead (see `files`), and the
// paths are spelled here — once, on both halves of the build — so the
// console and the host cannot disagree about where they are.

/// Where the console POSTs a file for `subject_id` (a feature or a
/// want), as `multipart/form-data` with a `file` part and an optional
/// `description` part. Answers a JSON [`WriteResult`] on success and a
/// plain-text refusal otherwise.
pub fn upload_path(subject_id: &str) -> String {
    format!("/_files/{subject_id}")
}

/// Where the console POSTs a comment for `subject_id`, as
/// `multipart/form-data`: a `kind` part (`note` | `question` |
/// `answer`), a `body`, for a question an `assigned_to`, for an
/// answer the `answers` question id, and zero or more `file` parts.
/// Answers a JSON [`WriteResult`] on success, a plain-text refusal
/// otherwise.
pub fn comment_path(subject_id: &str) -> String {
    format!("/_comments/{subject_id}")
}

/// Where a browser fetches one attachment's bytes. A navigation cannot
/// carry a header, so on a gated host the key rides the query string —
/// the same trade the event socket makes, for the same reason, and
/// the link then 302s to a short-lived presigned URL (or serves the
/// bytes itself when the object store mints none).
pub fn download_path(attachment_id: &str, key: &str) -> String {
    if key.is_empty() {
        format!("/_files/{attachment_id}")
    } else {
        format!("/_files/{attachment_id}?key={}", hex_of(key))
    }
}

/// Lowercase hex, as the event socket encodes its key: a key is
/// opaque bytes and a query string is not the place to find out which
/// characters it contains.
fn hex_of(s: &str) -> String {
    let mut out = String::with_capacity(s.len() * 2);
    for b in s.bytes() {
        out.push_str(&format!("{b:02x}"));
    }
    out
}

/// The inverse of [`hex_of`]; `None` for anything that is not hex.
pub fn unhex(s: &str) -> Option<String> {
    if s.len() % 2 != 0 {
        return None;
    }
    let bytes: Option<Vec<u8>> = (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&s[i..i + 2], 16).ok())
        .collect();
    String::from_utf8(bytes?).ok()
}

// ---------------------------------------------------------------------
// Wire types
// ---------------------------------------------------------------------

/// The console's global read: everything the home screen, the rail and
/// the all-features list show, and nothing any single feature's board
/// needs. One row of counts per feature — never a tree, never a
/// document body, never a ledger — so its size is the feature count
/// and not the project's age.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct Board {
    pub project: ProjectDto,
    /// Every feature, oldest first.
    pub features: Vec<FeatureRollupDto>,
    /// What is stuck across the project, worst first.
    pub attention: Vec<AttentionDto>,
    /// The newest events project-wide, newest first.
    pub recent: Vec<EventDto>,
    pub agents: Vec<AgentDto>,
    /// The tag registry, most-used first.
    pub tags: Vec<TagDto>,
    /// The pool's shape. The pool itself is paged — see [`search_wants`].
    pub wants: WantCountsDto,
    /// Every open question in the project, oldest first — what is
    /// waiting on somebody, and on whom.
    #[serde(default)]
    pub questions: Vec<QuestionDto>,
    /// The name this console's writes are recorded under: the key's
    /// agent name, or `console` on an open loopback host. What the
    /// console needs to know which comments are its own and which
    /// questions are owed to the person reading.
    #[serde(default)]
    pub you: String,
    /// The server's clock when this board was read, as Unix seconds.
    /// The console keeps the offset to its own clock and measures
    /// every `last_heard` against it, so liveness never depends on
    /// the reader's machine agreeing with the host about the time —
    /// and the board itself carries no clock-derived field that
    /// would make every poll look like a change.
    #[serde(default)]
    pub now: i64,
}

/// An open question, wherever it sits.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct QuestionDto {
    pub id: String,
    /// feature | want | module
    pub level: String,
    pub subject_id: String,
    pub subject_name: String,
    /// The feature a module's or a feature's question sits in; empty
    /// for a want's.
    pub feature_id: String,
    pub body: String,
    pub author: String,
    /// Who owes the answer; empty for anyone.
    pub assigned_to: String,
    /// "MMM D HH:MM"
    pub asked: String,
}

/// One comment in a discussion.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct CommentDto {
    pub id: String,
    /// note | question | answer
    pub kind: String,
    /// Markdown.
    pub body: String,
    pub author: String,
    /// A question: who owes the answer, or empty for anyone.
    pub assigned_to: String,
    /// An answer: the question it resolves.
    pub answers: String,
    /// A question: whether an answer has landed.
    pub resolved: bool,
    pub resolved_by: String,
    /// "MMM D HH:MM"
    pub posted: String,
    pub edited: bool,
    pub attachments: Vec<AttachmentDto>,
}

/// The discussion on one subject, oldest first.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct CommentPage {
    pub subject_id: String,
    pub comments: Vec<CommentDto>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct ProjectDto {
    pub name: String,
    pub description: String,
}

/// One feature as the board sees it: identity, state, and counts.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct FeatureRollupDto {
    pub id: String,
    pub name: String,
    /// planning | in_progress | done | shelved
    pub status: String,
    pub created_by: Option<String>,
    pub modules_done: i64,
    pub modules_total: i64,
    /// todo + unclaimed + every prerequisite done.
    pub modules_ready: i64,
    /// Modules an agent holds right now.
    #[serde(default)]
    pub modules_running: i64,
    pub tasks_done: i64,
    pub tasks_total: i64,
    /// Tasks the crew added beyond the plan.
    pub tasks_added: i64,
    /// "MMM D HH:MM" of the first and last ledger entry, or empty.
    pub started: String,
    pub last_activity: String,
    /// When an agent HOLDING a module here last wrote to the ledger,
    /// as Unix seconds; `None` when nobody holds one or no holder has
    /// written. A stamp and not a duration: the console measures it
    /// against [`Board::now`], so an idle poll returns an identical
    /// board and nothing on screen is rebuilt for it.
    #[serde(default)]
    pub last_heard: Option<i64>,
    /// The newest announcement anywhere in the feature.
    #[serde(default)]
    pub last_word: Option<AnnouncementDto>,
}

/// A stamp as Unix seconds, for the `last_heard` fields.
#[cfg(feature = "server")]
fn epoch(t: Option<chrono::DateTime<chrono::Utc>>) -> Option<i64> {
    t.map(|t| t.timestamp())
}

/// The latest thing an agent said it was doing, on a module, a
/// feature, or from the roster's side. Empty text means nothing was
/// ever announced there.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct AnnouncementDto {
    pub text: String,
    /// Who said it.
    pub by: String,
    /// "MMM D HH:MM" of when.
    pub at: String,
    /// mod_… or feat_… it was said on, and that subject's name.
    pub subject_id: String,
    pub subject: String,
}

/// One stuck thing on the home screen's attention list.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct AttentionDto {
    pub feature_id: String,
    pub feature_name: String,
    pub module_id: String,
    pub module_name: String,
    /// rejected | blocked
    pub kind: String,
    /// Names of the prerequisites still open.
    pub waiting_on: Vec<String>,
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq)]
pub struct WantCountsDto {
    pub open: i64,
    pub promoted: i64,
    pub declined: i64,
}

/// One feature's board: the graph and the plan. Fetched when the
/// feature is selected; refetched when a [`Tick`] names it.
///
/// No ledger and no handoffs: the feed is paged by [`load_events`] and
/// a module's handoff and history arrive with its drawer
/// ([`load_module`]). Both are the unbounded parts of a feature.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct FeatureDetail {
    pub id: String,
    pub name: String,
    pub description: String,
    /// planning | in_progress | done | shelved
    pub status: String,
    /// Topological order: every module after all of its prerequisites,
    /// ties by depth then name.
    pub modules: Vec<ModuleDto>,
    /// The plan as prose, current revision, when one was written.
    pub whitepaper: Option<DocumentDto>,
    /// The wants this feature was composed from, oldest link first.
    pub sources: Vec<WantSourceDto>,
    /// The feature's files, then its source wants' files.
    #[serde(default)]
    pub attachments: Vec<AttachmentDto>,
    /// Open questions on the feature itself; while any is open nothing
    /// in it is dispatchable.
    #[serde(default)]
    pub open_questions: Vec<QuestionDto>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct ModuleDto {
    pub id: String,
    pub name: String,
    pub description: String,
    /// todo | in_progress | blocked | done
    pub status: String,
    pub claimed_by: Option<String>,
    pub summary: Option<String>,
    /// Prerequisite module ids.
    pub depends_on: Vec<String>,
    /// Prerequisites not yet done. Empty means the gate is open.
    pub waiting_on: Vec<String>,
    /// Path prefixes this module writes to; empty = undeclared.
    pub owns: Vec<String>,
    /// Longest path from a root, 1-based. The graph's column.
    pub depth: i32,
    /// todo + unclaimed + every prerequisite done.
    pub dispatchable: bool,
    /// Unclaimed, not done, and a claim on it once bounced off the
    /// gate. Derived from the ledger server-side so the card needs no
    /// events to draw itself.
    pub rejected: bool,
    /// The latest rejection or blocker on it, pre-formatted, or empty.
    pub block_title: String,
    pub block_body: String,
    /// "MMM D HH:MM" of its first claim, or empty.
    pub spawned: String,
    /// When the agent holding this module last wrote to the ledger
    /// (on anything), as Unix seconds; `None` when nobody holds it or
    /// the holder has never written. A running module heard from a
    /// moment ago is MOVING; one silent for an hour is held by a box
    /// that has gone quiet, which is the failure this exists to make
    /// visible.
    #[serde(default)]
    pub last_heard: Option<i64>,
    /// Open questions on this module.
    #[serde(default)]
    pub open_questions: Vec<QuestionDto>,
    /// The newest announcement on this module.
    #[serde(default)]
    pub last_word: Option<AnnouncementDto>,
    pub tasks: Vec<TaskDto>,
}

/// What a module's drawer adds to its card: the handoff and the
/// history. Its own read because both grow with the module's life and
/// neither is needed until the drawer opens.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct ModuleDetail {
    pub id: String,
    pub feature_id: String,
    /// How to use what this module built, current revision.
    pub handoff: Option<DocumentDto>,
    /// Every ledger entry whose subject is this module, oldest first.
    pub history: Vec<EventDto>,
}

/// One page of a ledger, newest first.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct EventPage {
    /// The feature the page is of; empty for the project-wide ledger.
    pub feature_id: String,
    pub events: Vec<EventDto>,
    /// Whether an older page exists past the last event here.
    pub more: bool,
}

/// One page of the pool, plus what the filters matched in total so the
/// pager can say "1–20 of 142" without a second round trip.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct WantPage {
    pub wants: Vec<WantDto>,
    pub total: i64,
}

/// A plan as the console's editor composes it. Modules name their
/// prerequisites by NAME within the draft — ids do not exist yet.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct PlanDraft {
    pub name: String,
    pub description: String,
    /// The plan as prose, or empty for none.
    pub whitepaper: String,
    pub modules: Vec<ModuleDraft>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct ModuleDraft {
    pub name: String,
    pub description: String,
    pub tasks: Vec<String>,
    /// Prerequisites, by module name within the draft.
    pub depends_on: Vec<String>,
    pub owns: Vec<String>,
}

/// One plan-surgery operation, mirroring `mcpm_core::PlanOp` on the
/// wire so the console can send exactly what an agent's `revise_plan`
/// can. Applied in a batch, atomically or not at all.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum PlanOpDto {
    AddModule {
        name: String,
        description: String,
        tasks: Vec<String>,
        /// Prerequisites by module id.
        depends_on: Vec<String>,
        owns: Vec<String>,
    },
    AddTask { module_id: String, name: String },
    AddDependency { module_id: String, depends_on: String },
    RemoveDependency { module_id: String, depends_on: String },
    UpdateModule { id: String, description: String, owns: Vec<String> },
    UpdateFeature { description: String },
    Rename { id: String, name: String },
    Remove { id: String },
}

/// What one console write did, in a line the screen can show.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct WriteResult {
    pub message: String,
    /// The id the write produced or touched, when there is one — a
    /// new plan's feature id, say, so the console can open it.
    pub id: String,
}

/// One file attached to a feature or a want. The bytes are behind
/// [`download_path`]; this is the record and the description written
/// for whoever reads it next.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct AttachmentDto {
    pub id: String,
    /// feature | want
    pub level: String,
    pub subject_id: String,
    pub name: String,
    pub description: String,
    pub content_type: String,
    pub size_bytes: i64,
    /// "12.4 KB", "3.1 MB" — formatted server-side.
    pub size: String,
    pub added_by: String,
    /// "MMM D HH:MM"
    pub added: String,
    /// On a feature's list: the want a file came in through, as the
    /// idea's own words. Empty for the feature's own files.
    pub via_want_id: String,
    pub via_want: String,
    /// On a feature's list: the module a file is attached to.
    #[serde(default)]
    pub via_module_id: String,
    #[serde(default)]
    pub via_module: String,
    /// The comment the file arrived with, when it did.
    #[serde(default)]
    pub comment_id: String,
    #[serde(default)]
    pub comment_author: String,
    #[serde(default)]
    pub comment_excerpt: String,
}

/// One current document revision: a feature's whitepaper or a
/// module's handoff. Markdown in `body`.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct DocumentDto {
    pub id: String,
    /// whitepaper | handoff
    pub kind: String,
    pub revision: i32,
    pub title: String,
    pub body: String,
    pub author: String,
    /// "MMM D HH:MM"
    pub written: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct TaskDto {
    pub id: String,
    pub name: String,
    /// open | done | skipped
    pub status: String,
    /// planned | discovered
    pub origin: String,
    pub note: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct EventDto {
    pub seq: i64,
    /// "HH:MM"
    pub time: String,
    /// The raw event type (module_claimed, premature_claim, …).
    pub kind: String,
    pub agent: Option<String>,
    pub feature_id: Option<String>,
    pub subject_id: Option<String>,
    pub title: String,
    pub body: String,
}

/// One idea in the pool.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct WantDto {
    pub id: String,
    pub body: String,
    pub tags: Vec<String>,
    /// Derived server-side: open | promoted | declined
    pub status: String,
    /// The decline reason, or empty.
    pub note: String,
    pub author: String,
    /// "MMM D HH:MM"
    pub captured: String,
    /// Every feature that absorbed this want.
    pub features: Vec<WantLinkDto>,
    /// The want's files. Filled by [`load_want`] (the drawer's read);
    /// a page of the pool leaves it empty, because a row is a handle
    /// on the idea and its files are properties of it (UX rule 20).
    #[serde(default)]
    pub attachments: Vec<AttachmentDto>,
}

/// One entry in the knowledge base.
///
/// `Default` because it rides a component's props struct, and the
/// struct-literal dispatch needs one — the derived value is never
/// rendered; every call site passes a real entry.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct MemoryDto {
    pub id: String,
    /// project | feature | module | task
    pub level: String,
    pub subject_id: String,
    /// The node's name — or the project's, at project level.
    pub subject_name: String,
    /// convention | decision | gotcha | outcome | reference | note
    pub kind: String,
    pub content: String,
    pub tags: Vec<String>,
    pub author: String,
    /// "MMM D HH:MM"
    pub written: String,
    /// current | superseded | disputed. Derived, never stored.
    pub state: String,
    /// Distinct agents that used it.
    pub touches: i64,
    /// Distinct agents that verified it.
    pub confirms: i64,
    /// Distinct agents that say it is wrong.
    pub disputes: i64,
    /// Where it sits in its kind, 0 (newest) to 1 (oldest) — the decay
    /// clock, shown so a low rank is explicable rather than mysterious.
    pub newer_fraction: f32,
}

/// One page of knowledge, plus what the filters matched in total so the
/// pager can say "1–20 of 142" without a second round trip.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct KnowledgePage {
    pub entries: Vec<MemoryDto>,
    pub total: i64,
    /// Counts per kind across the WHOLE base, not this page — the
    /// header's stats must not lurch as the reader pages.
    pub by_kind: Vec<KindCount>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct KindCount {
    pub kind: String,
    pub count: i64,
}

/// One step of a memory's lineage, or one standing relation.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct LinkDto {
    /// replaces | refutes | revises | consolidates | refines |
    /// depends_on | contradicts | relates_to
    pub kind: String,
    /// Why the link exists. Often the only place the reason survives.
    pub rationale: String,
    /// For a standing relation: whether this memory is the source. A
    /// `refines` read outward and inward mean opposite things.
    pub outgoing: bool,
    pub other: MemoryDto,
}

/// Everything the graph knows about one entry.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct MemoryDetail {
    pub entry: MemoryDto,
    /// What it replaced, nearest first.
    pub supersedes: Vec<LinkDto>,
    /// What replaced it.
    pub superseded_by: Vec<LinkDto>,
    /// Declared standing relations, both directions.
    pub relations: Vec<LinkDto>,
    /// Undeclared relations the co-touch record hints at. Suggestions,
    /// never assertions.
    pub suggestions: Vec<MemoryDto>,
}

/// One tag in the registry.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct TagDto {
    /// Normalized slug — what you type after `#`.
    pub name: String,
    /// How it was first typed.
    pub label: String,
    pub uses: i64,
}

/// A want→feature link, seen from the want.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct WantLinkDto {
    pub feature_id: String,
    pub feature_name: String,
    pub rationale: String,
}

/// A want→feature link, seen from the feature.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct WantSourceDto {
    pub want_id: String,
    pub body: String,
    pub rationale: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct AgentDto {
    pub name: String,
    pub role: String,
    pub active_claims: i64,
    /// Comma-joined module names it currently holds.
    pub claim_names: String,
    /// The health check registered for it, if any. `None` is "no check
    /// configured", which the roster shows differently from "checked
    /// and down".
    #[serde(default)]
    pub health: Option<AgentHealthDto>,
    /// "MMM D HH:MM" it last registered or announced.
    #[serde(default)]
    pub last_seen: String,
    /// Its newest ledger write, as Unix seconds; `None` if it never
    /// wrote.
    #[serde(default)]
    pub last_heard: Option<i64>,
    /// The newest thing it announced.
    #[serde(default)]
    pub last_word: Option<AnnouncementDto>,
}

/// What the server's last probe of an agent's machine concluded.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct AgentHealthDto {
    pub url: String,
    /// `up` / `down` / `gone` / `unreachable`, or empty before the first
    /// probe.
    pub state: String,
    /// The probe's one-line reason: `HTTP 503`, `timed out after 8s`.
    pub detail: String,
    /// When the current state was first observed, as a stamp; empty
    /// before the first probe.
    pub since: String,
    /// The last probe's time, as a stamp.
    pub checked: String,
}

/// One committed event, announced. The console renders nothing out of
/// it — it refetches the reads it already knows how to apply — but it
/// reads `feature_id` and `kind` to decide WHICH: an event on another
/// feature refreshes the board's counts and leaves the open tree alone.
///
/// Carrying `seq` rather than an empty ping is what makes the client
/// idempotent: two notifications for the same seq (a reconnect
/// replaying, say) collapse into one refetch.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct Tick {
    /// The `events.seq` of the row that fired the trigger.
    pub seq: i64,
    /// The raw event type. Empty from a database whose trigger predates
    /// migration 0013 — the client then treats every tick as touching
    /// everything, which is what it did before.
    pub kind: String,
    pub feature_id: Option<String>,
    pub subject_id: Option<String>,
}

// ---------------------------------------------------------------------
// Who is calling
// ---------------------------------------------------------------------

/// The resolved identity behind one console request, put on the request
/// context by `mcpm-web`'s dispatch gate and read back here as
/// `Extension<Caller>`.
///
/// It always resolves, in both postures: with the gate on it is the
/// agent name off a verified key, and with the gate off (loopback dev)
/// it is the anonymous console author. A handler therefore never has to
/// ask whether authentication was configured — it just records who the
/// write belongs to.
#[cfg(feature = "server")]
#[derive(Clone, Debug)]
pub struct Caller {
    /// The name every write in this request is attributed to.
    pub name: String,
    /// The key's role, or `None` on an unauthenticated loopback host.
    pub role: Option<mcpm_core::KeyRole>,
}

#[cfg(feature = "server")]
impl Caller {
    /// The posture an open loopback host runs in: a person at the
    /// console, not an agent. Recorded as `console` exactly as before
    /// the gate existed, so the pool's provenance does not shift under
    /// a deployment that never turned auth on.
    pub fn local() -> Caller {
        Caller { name: CONSOLE_AUTHOR.to_string(), role: None }
    }

    /// This caller as the store sees it. A person — the open loopback
    /// host, or a console key — is a human actor, which is what lets
    /// them answer any question; an agent's key stays an agent.
    pub fn actor(&self) -> mcpm_core::Actor {
        match self.role {
            None | Some(mcpm_core::KeyRole::Console) => mcpm_core::Actor::human(&self.name),
            Some(_) => mcpm_core::Actor::new(&self.name),
        }
    }

    /// Resolve a presented key — or none — to a caller under this
    /// host's posture. `Err(())` is a refusal: a bad key in either
    /// posture, or no key on a host that demands one.
    ///
    /// One function for the server-fn gate AND the file routes, so
    /// the two surfaces cannot drift on who gets in: a route that
    /// authenticated a little differently from `/_srv/*` would be the
    /// hole nobody looks at.
    pub async fn resolve(
        store: &mcpm_core::Store,
        presented: Option<&str>,
        require_auth: bool,
    ) -> Result<Caller, ()> {
        match presented {
            Some(token) => match store.verify_key(token).await {
                Ok(identity) => Ok(Caller { name: identity.agent_name, role: Some(identity.role) }),
                Err(_) => Err(()),
            },
            None if !require_auth => Ok(Caller::local()),
            None => Err(()),
        }
    }
}

// ---------------------------------------------------------------------
// Server functions
// ---------------------------------------------------------------------

/// The console's liveness feed: one [`Tick`] per committed event, over
/// a WebSocket.
///
/// The stream is a Postgres LISTEN on the channel the `events_notify`
/// trigger announces on, so it carries writes from EVERY process
/// against this database — the MCP server's agent writes as much as the
/// console's own captures. See `Store::watch_events`.
///
/// # Why the key is an argument and not a header
///
/// This one endpoint authenticates in its own body rather than at the
/// dispatch gate, because a browser cannot set `Authorization` on a
/// WebSocket handshake — the request that opens this socket carries no
/// headers we control. The SDK's one channel for caller-supplied data
/// at open time is the subscription's own arguments, so the key rides
/// there, hex-encoded into the connect URL.
///
/// That is a real trade: a URL is likelier to be logged by a proxy than
/// a header, so a console key can end up in somebody's access log. It is
/// the least-privileged key this system issues (read plus capture, never
/// the agent surface) and both ends sit inside the deployment, which is
/// what makes the trade acceptable rather than fine. The upgrade, if
/// this ever fronts something untrusted, is a short-lived ticket minted
/// over the authenticated POST channel and spent here.
// NOT `#[cfg(feature = "server")]`: the macro cfgs the real body itself
// and emits the client stub under `not(server)`. Gating the whole item
// would delete the stub the console calls. The `State<_>` param is an
// extractor, so it never appears in that stub — the client build never
// names `mcpm_core`; `key` is an open arg and DOES.
#[subscription]
pub async fn watch_events(
    key: String,
    store: server::State<mcpm_core::Store>,
) -> impl futures_core::Stream<Item = Tick> {
    use futures_util::StreamExt as _;
    // An unauthenticated socket yields nothing rather than erroring:
    // the console degrades to its fallback poll, which is the same
    // place a dropped socket lands it, and the poll's own 401 is what
    // tells the reader their key is wrong. Two reports of one fact
    // would just be noise.
    if !socket_authorized(&store, &key).await {
        return futures_util::stream::empty().left_stream();
    }
    // A listener that cannot be opened yields an empty stream: the
    // console keeps its slow fallback poll, which is exactly the
    // degraded mode this is a fast path over.
    let notices = store.watch_events().await;
    async_stream_compat(notices)
        .map(|n| Tick {
            seq: n.seq,
            kind: n.kind,
            feature_id: n.feature_id,
            subject_id: n.subject_id,
        })
        .right_stream()
}

/// Whether this socket may open. Mirrors `mcpm-web`'s gate: with auth
/// off (loopback dev) every socket is admitted; with it on the key must
/// verify. Reading the same env var as the host binary keeps the two
/// postures from drifting apart.
#[cfg(feature = "server")]
async fn socket_authorized(store: &mcpm_core::Store, key: &str) -> bool {
    if !auth_required() {
        return true;
    }
    match store.verify_key(key).await {
        Ok(_) => true,
        Err(_) => {
            eprintln!("mcpm-web: event socket refused — no valid API key");
            false
        }
    }
}

/// Whether this host demands a key. Defined here rather than only in
/// the binary because the subscription gate above has to agree with it
/// and cannot see the binary's locals.
///
/// Two ways in, and the second is the point: a host that binds anything
/// but loopback is reachable from the network, and an unauthenticated
/// console on the network is precisely the accident this refuses to let
/// you have. Turning auth OFF there is not expressible.
#[cfg(feature = "server")]
pub fn auth_required() -> bool {
    let explicit = std::env::var("MCPM_REQUIRE_AUTH")
        .map(|v| matches!(v.trim(), "1" | "true" | "yes" | "on"))
        .unwrap_or(false);
    explicit || !binds_loopback()
}

/// Whether `HOST` keeps this process on the loopback interface. An
/// unparseable or unset `HOST` counts as loopback, matching the
/// binary's own default.
#[cfg(feature = "server")]
pub fn binds_loopback() -> bool {
    match std::env::var("HOST") {
        Ok(host) => host
            .parse::<std::net::IpAddr>()
            .map(|ip| ip.is_loopback())
            .unwrap_or(true),
        Err(_) => true,
    }
}

/// `Store::watch_events` returns a `Result`; flatten it to a stream so
/// the subscription body stays one expression. An error becomes an
/// empty stream — the client falls back to polling and the reason is
/// logged here rather than crashing the socket handler.
#[cfg(feature = "server")]
fn async_stream_compat(
    notices: std::result::Result<
        impl futures_core::Stream<Item = mcpm_core::EventNotice>,
        mcpm_core::McpmError,
    >,
) -> impl futures_core::Stream<Item = mcpm_core::EventNotice> {
    use futures_util::StreamExt as _;
    match notices {
        Ok(stream) => stream.left_stream(),
        Err(err) => {
            eprintln!("mcpm-web: event listener unavailable, console will poll: {err}");
            futures_util::stream::empty().right_stream()
        }
    }
}

/// The console's global read. See [`Board`].
#[server]
pub async fn load_board(caller: server::Extension<Caller>) -> Result<Board, ServerError> {
    let store = server::use_state::<mcpm_core::Store>()
        .ok_or_else(|| ServerError::failed("Store not installed"))?;
    let raw = store.snapshot().await.map_err(fail)?;
    let project = ProjectDto {
        name: raw["project"]["name"].as_str().unwrap_or("control-center").to_string(),
        description: raw["project"]["description"].as_str().unwrap_or("").to_string(),
    };
    let stamp = |t: Option<chrono::DateTime<chrono::Utc>>| {
        t.map(|t| t.format("%b %-d %H:%M").to_string()).unwrap_or_default()
    };
    let features = store
        .rollups()
        .await
        .map_err(fail)?
        .into_iter()
        .map(|r| FeatureRollupDto {
            id: r.id,
            name: r.name,
            status: r.status,
            created_by: r.created_by,
            modules_done: r.modules_done,
            modules_total: r.modules_total,
            modules_ready: r.modules_ready,
            modules_running: r.modules_running,
            tasks_done: r.tasks_done,
            tasks_total: r.tasks_total,
            tasks_added: r.tasks_added,
            started: stamp(r.started),
            last_activity: stamp(r.last_activity),
            last_heard: epoch(r.last_heard),
            last_word: r.last_word.as_ref().map(announcement_dto),
        })
        .collect();
    let attention = store
        .attention()
        .await
        .map_err(fail)?
        .into_iter()
        .map(|a| AttentionDto {
            feature_id: a.feature_id,
            feature_name: a.feature_name,
            module_id: a.module_id,
            module_name: a.module_name,
            kind: a.kind,
            waiting_on: a.waiting_on,
        })
        .collect();
    // A dozen: the home screen shows six and a tick usually lands one.
    let (recent, _) = store.events_page(None, None, 12).await.map_err(fail)?;
    let agents = store
        .agents_overview()
        .await
        .map_err(fail)?
        .into_iter()
        .map(|a| AgentDto {
            name: a.name,
            role: a.role,
            active_claims: a.active_claims,
            claim_names: a.claim_names,
            health: a.health.map(|h| AgentHealthDto {
                url: h.url,
                state: h.state.map(|s| s.as_str().to_string()).unwrap_or_default(),
                detail: h.detail,
                since: stamp(h.since),
                checked: stamp(h.checked_at),
            }),
            last_seen: stamp(Some(a.last_seen)),
            last_heard: epoch(a.last_activity),
            last_word: a.last_word.as_ref().map(announcement_dto),
        })
        .collect();
    let tags = store
        .list_tags()
        .await
        .map_err(fail)?
        .into_iter()
        .map(|t| TagDto {
            name: t.name,
            label: t.label,
            uses: t.uses,
        })
        .collect();
    let counts = store.want_counts().await.map_err(fail)?;
    let questions = store
        .open_questions()
        .await
        .map_err(fail)?
        .iter()
        .map(question_dto)
        .collect();

    Ok(Board {
        project,
        features,
        attention,
        recent: recent.iter().map(format_event).collect(),
        agents,
        tags,
        wants: WantCountsDto {
            open: counts.open,
            promoted: counts.promoted,
            declined: counts.declined,
        },
        questions,
        you: caller.name.clone(),
        now: chrono::Utc::now().timestamp(),
    })
}

/// One feature's board. See [`FeatureDetail`].
#[server]
pub async fn load_feature(feature_id: String) -> Result<FeatureDetail, ServerError> {
    let store = server::use_state::<mcpm_core::Store>()
        .ok_or_else(|| ServerError::failed("Store not installed"))?;
    let tree = store.feature_tree(&feature_id).await.map_err(fail)?;
    let milestones = store.module_milestones(&feature_id).await.map_err(fail)?;
    let sources = store
        .wants_of_feature(&feature_id)
        .await
        .map_err(fail)?
        .into_iter()
        .map(|w| WantSourceDto {
            rationale: w
                .features
                .iter()
                .find(|l| l.feature_id == feature_id)
                .map(|l| l.rationale.clone())
                .unwrap_or_default(),
            want_id: w.id,
            body: w.body,
        })
        .collect();

    let modules = tree
        .modules
        .into_iter()
        .map(|m| {
            let marks = milestones.iter().find(|x| x.module_id == m.id);
            let rejected = m.status != "done"
                && m.claimed_by.is_none()
                && marks.is_some_and(|x| x.last_rejection.is_some());
            let (block_title, block_body) = if let Some(q) = m.open_questions.first() {
                (
                    format!(
                        "Pending resolution \u{2014} owed by {}",
                        if q.assigned_to.as_deref().unwrap_or("").is_empty() {
                            "anyone"
                        } else {
                            q.assigned_to.as_deref().unwrap_or("")
                        }
                    ),
                    q.body.clone(),
                )
            } else if rejected {
                (
                    "Start rejected \u{2014} prerequisites open".to_string(),
                    marks
                        .and_then(|x| x.last_rejection.as_ref())
                        .map(|e| format_event(e).body)
                        .unwrap_or_default(),
                )
            } else if m.status == "blocked" {
                (
                    "Blocked \u{2014} waiting on the manager".to_string(),
                    marks
                        .and_then(|x| x.last_blocker.as_ref())
                        .map(|e| format_event(e).body)
                        .unwrap_or_default(),
                )
            } else {
                (String::new(), String::new())
            };
            ModuleDto {
                spawned: marks
                    .and_then(|x| x.first_claim)
                    .map(|t| t.format("%b %-d %H:%M").to_string())
                    .unwrap_or_default(),
                last_heard: epoch(marks.and_then(|x| x.last_activity)),
                rejected,
                block_title,
                block_body,
                id: m.id,
                name: m.name,
                description: m.description,
                status: m.status,
                claimed_by: m.claimed_by,
                summary: m.summary,
                depends_on: m.depends_on,
                waiting_on: m.waiting_on,
                owns: m.owns,
                depth: m.depth,
                dispatchable: m.dispatchable,
                open_questions: m.open_questions.iter().map(question_dto).collect(),
                last_word: m.last_word.as_ref().map(announcement_dto),
                tasks: m
                    .tasks
                    .into_iter()
                    .map(|t| TaskDto {
                        id: t.id,
                        name: t.name,
                        status: t.status,
                        origin: t.origin,
                        note: t.note,
                    })
                    .collect(),
            }
        })
        .collect();

    Ok(FeatureDetail {
        id: tree.id,
        name: tree.name,
        description: tree.description,
        status: tree.status,
        modules,
        whitepaper: tree.whitepaper.as_ref().map(document_dto),
        sources,
        attachments: tree.attachments.iter().map(attachment_dto).collect(),
        open_questions: tree.open_questions.iter().map(question_dto).collect(),
    })
}

/// The discussion on one subject — a feature, a want or a module —
/// oldest first, the newest 200. Its own read, fetched when the
/// discussion is on screen, because a thread grows without bound and
/// the feature's board must not.
#[server]
pub async fn load_comments(subject_id: String) -> Result<CommentPage, ServerError> {
    let store = server::use_state::<mcpm_core::Store>()
        .ok_or_else(|| ServerError::failed("Store not installed"))?;
    let comments = store.comments_of(&subject_id, None, 200).await.map_err(fail)?;
    Ok(CommentPage { subject_id, comments: comments.iter().map(comment_dto).collect() })
}

/// One module's drawer. See [`ModuleDetail`].
#[server]
pub async fn load_module(module_id: String) -> Result<ModuleDetail, ServerError> {
    let store = server::use_state::<mcpm_core::Store>()
        .ok_or_else(|| ServerError::failed("Store not installed"))?;
    let handoff = store
        .read_document(mcpm_core::DocumentKind::Handoff, &module_id)
        .await
        .map_err(fail)?;
    let history = store.events_of_subject(&module_id, 200).await.map_err(fail)?;
    let feature_id = history
        .iter()
        .find_map(|e| e.feature_id.clone())
        .unwrap_or_default();
    Ok(ModuleDetail {
        id: module_id,
        feature_id,
        handoff: handoff.as_ref().map(document_dto),
        history: history.iter().map(format_event).collect(),
    })
}

/// One page of a ledger, newest first: one feature's when `feature_id`
/// is set, the project's when it is empty. `before` is the oldest
/// `seq` the reader already holds (0 for the newest page).
#[server]
pub async fn load_events(
    feature_id: String,
    before: i64,
    limit: i64,
) -> Result<EventPage, ServerError> {
    let store = server::use_state::<mcpm_core::Store>()
        .ok_or_else(|| ServerError::failed("Store not installed"))?;
    let scope = (!feature_id.is_empty()).then_some(feature_id.as_str());
    let cursor = (before > 0).then_some(before);
    let (events, more) = store.events_page(scope, cursor, limit).await.map_err(fail)?;
    Ok(EventPage {
        feature_id,
        events: events.iter().map(format_event).collect(),
        more,
    })
}

/// One page of the pool. `statuses` is any set of open / promoted /
/// declined (empty means every state); every tag in `tags` must be
/// present; `query` is a full-text search, ranked when non-empty.
///
/// Paged server-side for the same reason the knowledge base is: the
/// pool grows without bound, and the filter that makes it useful runs
/// against the text index — shipping it whole to filter in the browser
/// was the second-largest thing the console sent.
#[server]
pub async fn search_wants(
    query: String,
    statuses: Vec<String>,
    tags: Vec<String>,
    page: i64,
) -> Result<WantPage, ServerError> {
    let store = server::use_state::<mcpm_core::Store>()
        .ok_or_else(|| ServerError::failed("Store not installed"))?;
    const PER_PAGE: i64 = 20;
    let statuses: Vec<String> = statuses
        .into_iter()
        .filter(|s| matches!(s.as_str(), "open" | "promoted" | "declined"))
        .collect();
    let found = store
        .search_wants(&query, &statuses, &tags, page.max(0) * PER_PAGE, PER_PAGE)
        .await
        .map_err(fail)?;
    Ok(WantPage {
        wants: found.wants.iter().map(want_dto).collect(),
        total: found.total,
    })
}

/// One idea, for its drawer — which can open from a feature's origin
/// list onto a want that is on no loaded page of the pool.
#[server]
pub async fn load_want(want_id: String) -> Result<WantDto, ServerError> {
    let store = server::use_state::<mcpm_core::Store>()
        .ok_or_else(|| ServerError::failed("Store not installed"))?;
    let mut want = want_dto(&store.get_want(&want_id).await.map_err(fail)?);
    want.attachments = store
        .attachments_of(&want_id)
        .await
        .map_err(fail)?
        .iter()
        .map(attachment_dto)
        .collect();
    Ok(want)
}

// ---------------------------------------------------------------------
// Writes from the console
// ---------------------------------------------------------------------
//
// A person at the console plans, edits and deletes by hand. Every write
// below goes through the same store methods the MCP tools use, so the
// invariants (a plan's cycle and ownership checks, "history is not
// deleted", a promoted want is frozen) hold for a click exactly as for
// an agent — nothing is re-derived here.

/// Who may plan from the console: a person (an open loopback host, or a
/// console key) or a manager key. A worker key is refused — workers are
/// dispatched, they do not plan — and that is the one rule this surface
/// adds to the store's own.
#[cfg(feature = "server")]
pub(crate) fn require_planner(caller: &Caller) -> Result<(), ServerError> {
    match caller.role {
        Some(mcpm_core::KeyRole::Worker) => Err(ServerError::failed(
            "A worker key cannot plan or edit from the console.",
        )),
        _ => Ok(()),
    }
}

/// Plan a feature from the console's editor. Returns the new feature's
/// id so the console can open it.
#[server]
pub async fn create_plan(
    draft: PlanDraft,
    caller: server::Extension<Caller>,
) -> Result<WriteResult, ServerError> {
    require_planner(&caller)?;
    let store = server::use_state::<mcpm_core::Store>()
        .ok_or_else(|| ServerError::failed("Store not installed"))?;
    let plan = mcpm_core::PlanFeature {
        name: draft.name.trim().to_string(),
        description: draft.description.trim().to_string(),
        whitepaper: Some(draft.whitepaper.trim().to_string()).filter(|w| !w.is_empty()),
        modules: draft
            .modules
            .into_iter()
            .map(|m| mcpm_core::PlanModule {
                name: m.name.trim().to_string(),
                description: m.description.trim().to_string(),
                tasks: m.tasks,
                depends_on: m.depends_on,
                owns: m.owns,
            })
            .collect(),
        stages: Vec::new(),
    };
    let tree = store.plan_feature(&caller.name, plan).await.map_err(fail)?;
    Ok(WriteResult {
        message: format!("Planned '{}' with {} modules.", tree.name, tree.modules.len()),
        id: tree.id,
    })
}

/// Revise a plan: one or more operations, atomically or not at all.
#[server]
pub async fn revise_plan(
    feature_id: String,
    ops: Vec<PlanOpDto>,
    caller: server::Extension<Caller>,
) -> Result<WriteResult, ServerError> {
    require_planner(&caller)?;
    let store = server::use_state::<mcpm_core::Store>()
        .ok_or_else(|| ServerError::failed("Store not installed"))?;
    let ops: Vec<mcpm_core::PlanOp> = ops
        .into_iter()
        .map(|op| match op {
            PlanOpDto::AddModule { name, description, tasks, depends_on, owns } => {
                mcpm_core::PlanOp::AddModule { name, description, tasks, depends_on, owns }
            }
            PlanOpDto::AddTask { module_id, name } => {
                mcpm_core::PlanOp::AddTask { module_id, name, note: None }
            }
            PlanOpDto::AddDependency { module_id, depends_on } => {
                mcpm_core::PlanOp::AddDependency { module_id, depends_on }
            }
            PlanOpDto::RemoveDependency { module_id, depends_on } => {
                mcpm_core::PlanOp::RemoveDependency { module_id, depends_on }
            }
            PlanOpDto::UpdateModule { id, description, owns } => mcpm_core::PlanOp::UpdateModule {
                id,
                description: Some(description),
                owns: Some(owns),
            },
            PlanOpDto::UpdateFeature { description } => {
                mcpm_core::PlanOp::UpdateFeature { description }
            }
            PlanOpDto::Rename { id, name } => mcpm_core::PlanOp::Rename { id, name },
            PlanOpDto::Remove { id } => mcpm_core::PlanOp::Remove { id },
        })
        .collect();
    let count = ops.len();
    store.revise_plan(&caller.name, &feature_id, ops).await.map_err(fail)?;
    Ok(WriteResult {
        message: format!("Plan revised ({count} change{}).", if count == 1 { "" } else { "s" }),
        id: feature_id,
    })
}

/// Delete a plan nothing has been done in. See `Store::delete_feature`.
#[server]
pub async fn delete_plan(
    feature_id: String,
    caller: server::Extension<Caller>,
) -> Result<WriteResult, ServerError> {
    require_planner(&caller)?;
    let store = server::use_state::<mcpm_core::Store>()
        .ok_or_else(|| ServerError::failed("Store not installed"))?;
    let ack = store.delete_feature(&caller.name, &feature_id).await.map_err(fail)?;
    Ok(WriteResult { message: ack.message, id: String::new() })
}

/// Park a feature, or take it back off the shelf.
#[server]
pub async fn shelve_plan(
    feature_id: String,
    shelve: bool,
    caller: server::Extension<Caller>,
) -> Result<WriteResult, ServerError> {
    require_planner(&caller)?;
    let store = server::use_state::<mcpm_core::Store>()
        .ok_or_else(|| ServerError::failed("Store not installed"))?;
    let ack = store.shelve_feature(&caller.name, &feature_id, shelve).await.map_err(fail)?;
    Ok(WriteResult { message: ack.message, id: feature_id })
}

/// Rewrite a comment of one's own.
#[server]
pub async fn edit_comment(
    comment_id: String,
    body: String,
    caller: server::Extension<Caller>,
) -> Result<WriteResult, ServerError> {
    let store = server::use_state::<mcpm_core::Store>()
        .ok_or_else(|| ServerError::failed("Store not installed"))?;
    let c = store.edit_comment(caller.actor(), &comment_id, &body).await.map_err(fail)?;
    Ok(WriteResult { message: "Comment updated.".into(), id: c.subject_id })
}

/// Delete a note of one's own, or withdraw an open question.
#[server]
pub async fn delete_comment(
    comment_id: String,
    caller: server::Extension<Caller>,
) -> Result<WriteResult, ServerError> {
    let store = server::use_state::<mcpm_core::Store>()
        .ok_or_else(|| ServerError::failed("Store not installed"))?;
    let ack = store.delete_comment(caller.actor(), &comment_id).await.map_err(fail)?;
    Ok(WriteResult { message: ack.message, id: String::new() })
}

/// Rewrite what an attachment is for.
#[server]
pub async fn describe_attachment(
    attachment_id: String,
    description: String,
    caller: server::Extension<Caller>,
) -> Result<WriteResult, ServerError> {
    require_planner(&caller)?;
    let store = server::use_state::<mcpm_core::Store>()
        .ok_or_else(|| ServerError::failed("Store not installed"))?;
    let att = store
        .describe_attachment(caller.name.as_str(), &attachment_id, &description)
        .await
        .map_err(fail)?;
    Ok(WriteResult { message: format!("Described '{}'.", att.name), id: att.subject_id })
}

/// Remove an attachment: its record now, its bytes right after.
#[server]
pub async fn remove_attachment(
    attachment_id: String,
    caller: server::Extension<Caller>,
) -> Result<WriteResult, ServerError> {
    require_planner(&caller)?;
    let store = server::use_state::<mcpm_core::Store>()
        .ok_or_else(|| ServerError::failed("Store not installed"))?;
    let ack = store.remove_attachment(caller.name.as_str(), &attachment_id).await.map_err(fail)?;
    Ok(WriteResult { message: ack.message, id: String::new() })
}

/// Revise a want's wording and tags. Tags are the console's `#tag`
/// syntax split out already; an empty `body` leaves the wording alone
/// (a composed want's is frozen, and the form does not offer it).
#[server]
pub async fn edit_want(
    want_id: String,
    body: String,
    tags: Vec<String>,
    caller: server::Extension<Caller>,
) -> Result<WriteResult, ServerError> {
    require_planner(&caller)?;
    let store = server::use_state::<mcpm_core::Store>()
        .ok_or_else(|| ServerError::failed("Store not installed"))?;
    let current = store.get_want(&want_id).await.map_err(fail)?;
    // Only what changed is sent on: the store refuses a body edit on a
    // promoted want, and an unchanged body must not count as one.
    let body = Some(body.trim().to_string()).filter(|b| !b.is_empty() && *b != current.body);
    let tags = Some(tags).filter(|t| *t != current.tags);
    if body.is_none() && tags.is_none() {
        return Ok(WriteResult { message: "Nothing changed.".to_string(), id: want_id });
    }
    let edit = mcpm_core::WantEdit { body, tags, state: None, reason: None };
    let want = store.update_want(&caller.name, &want_id, edit).await.map_err(fail)?;
    Ok(WriteResult { message: "Want updated.".to_string(), id: want.id })
}

/// Decline a want (`declined`, with `reason`) or reopen it (`open`).
#[server]
pub async fn set_want_state(
    want_id: String,
    state: String,
    reason: String,
    caller: server::Extension<Caller>,
) -> Result<WriteResult, ServerError> {
    require_planner(&caller)?;
    let store = server::use_state::<mcpm_core::Store>()
        .ok_or_else(|| ServerError::failed("Store not installed"))?;
    let (state, message) = match state.as_str() {
        "open" => (mcpm_core::WantState::Open, "Want reopened."),
        "declined" => (mcpm_core::WantState::Declined, "Want declined."),
        other => return Err(ServerError::failed(format!("Unknown want state '{other}'."))),
    };
    let edit = mcpm_core::WantEdit {
        body: None,
        tags: None,
        state: Some(state),
        reason: Some(reason).filter(|r| !r.trim().is_empty()),
    };
    let want = store.update_want(&caller.name, &want_id, edit).await.map_err(fail)?;
    Ok(WriteResult { message: message.to_string(), id: want.id })
}

/// Remove a loose or declined want. See `Store::delete_want`.
#[server]
pub async fn delete_want(
    want_id: String,
    caller: server::Extension<Caller>,
) -> Result<WriteResult, ServerError> {
    require_planner(&caller)?;
    let store = server::use_state::<mcpm_core::Store>()
        .ok_or_else(|| ServerError::failed("Store not installed"))?;
    let ack = store.delete_want(&caller.name, &want_id).await.map_err(fail)?;
    Ok(WriteResult { message: ack.message, id: String::new() })
}

#[cfg(feature = "server")]
fn want_dto(w: &mcpm_core::Want) -> WantDto {
    WantDto {
        id: w.id.clone(),
        body: w.body.clone(),
        tags: w.tags.clone(),
        status: w.status.clone(),
        note: w.decline_reason.clone().unwrap_or_default(),
        author: w.author.clone(),
        captured: w.created_at.format("%b %-d %H:%M").to_string(),
        features: w
            .features
            .iter()
            .map(|l| WantLinkDto {
                feature_id: l.feature_id.clone(),
                feature_name: l.feature_name.clone(),
                rationale: l.rationale.clone(),
            })
            .collect(),
        attachments: Vec::new(),
    }
}

/// What one capture wrote. Returned to the console so it can say what
/// happened rather than silently refreshing.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct CaptureResult {
    /// Bodies of the wants that landed, in order.
    pub added: Vec<String>,
    /// Tags that did not exist before this capture.
    pub new_tags: Vec<String>,
    /// Lines that held no idea and were skipped.
    pub skipped: usize,
}

/// Author recorded for anything captured through the console. Not an
/// agent name: it says a person typed this, which is the one thing the
/// pool's provenance should never blur.
#[cfg(feature = "server")]
const CONSOLE_AUTHOR: &str = "console";

/// Capture raw composer text: one want per line, `#tag` anywhere in it.
///
/// The console posts the buffer verbatim and the server parses it with
/// the same [`capture`] functions the editor highlights with, so what
/// was colored is what is written. The whole buffer lands in one
/// transaction — a half-captured list is worse than a rejected one.
#[server]
pub async fn capture_wants(
    text: String,
    caller: server::Extension<Caller>,
) -> Result<CaptureResult, ServerError> {
    let store = server::use_state::<mcpm_core::Store>()
        .ok_or_else(|| ServerError::failed("Store not installed"))?;
    let drafts = capture::parse_buffer(&text);
    if drafts.is_empty() {
        return Err(ServerError::failed(
            "Nothing to capture — every line was blank or held only tags.",
        ));
    }
    let skipped = text.lines().filter(|l| !l.trim().is_empty()).count() - drafts.len();

    let known: std::collections::HashSet<String> = store
        .list_tags()
        .await
        .map_err(fail)?
        .into_iter()
        .map(|t| t.name)
        .collect();
    let mut new_tags: Vec<String> = Vec::new();
    for draft in &drafts {
        for tag in &draft.tags {
            if !known.contains(tag) && !new_tags.contains(tag) {
                new_tags.push(tag.clone());
            }
        }
    }

    let wants = store
        .add_wants(
            &caller.name,
            drafts
                .into_iter()
                .map(|d| mcpm_core::WantDraft {
                    body: d.body,
                    tags: d.tags,
                })
                .collect(),
        )
        .await
        .map_err(fail)?;

    Ok(CaptureResult {
        added: wants.into_iter().map(|w| w.body).collect(),
        new_tags,
        skipped,
    })
}

/// The console's window on the knowledge base.
///
/// Its own endpoint rather than a slice of the snapshot: the base grows
/// without bound, and the ranking that makes a loose query useful has
/// to happen in Postgres against the full text index — shipping every
/// entry to the browser to filter there would be both the largest
/// payload the console sends and the worst search it could offer.
#[server]
pub async fn search_knowledge(
    query: String,
    kinds: Vec<String>,
    tags: Vec<String>,
    level: String,
    page: i64,
    include_superseded: bool,
) -> Result<KnowledgePage, ServerError> {
    let store = server::use_state::<mcpm_core::Store>()
        .ok_or_else(|| ServerError::failed("Store not installed"))?;

    let kinds: Vec<mcpm_core::MemoryKind> = kinds
        .iter()
        .filter_map(|k| mcpm_core::MemoryKind::parse(k))
        .collect();
    // A level filter is expressed as an anchor at that level searched
    // `here`, except "project", which is its own singleton shelf. An
    // unrecognized level means no filter rather than no results: the
    // console's "All" arm sends an empty string.
    let scope = match level.as_str() {
        "project" => Some(mcpm_core::MemoryScope::project()),
        _ => None,
    };
    let page = page.max(0);
    const PER_PAGE: i64 = 20;

    let found = store
        .search_memory(&mcpm_core::MemoryQuery {
            text: query,
            kinds,
            tags,
            scope,
            direction: if level == "project" {
                mcpm_core::SearchDirection::Here
            } else {
                mcpm_core::SearchDirection::All
            },
            limit: PER_PAGE,
            offset: page * PER_PAGE,
            include_superseded,
            ..Default::default()
        })
        .await
        .map_err(fail)?;

    // The non-project level filters are applied here rather than in the
    // store, because "every memory at module level" is not an anchor —
    // it is a level predicate, and the store's scoping vocabulary is
    // deliberately about anchors and directions.
    let entries: Vec<MemoryDto> = found
        .hits
        .iter()
        .filter(|h| level.is_empty() || level == "project" || h.memory.level == level)
        .map(|h| memory_dto(&h.memory))
        .collect();

    Ok(KnowledgePage {
        entries,
        total: found.total,
        by_kind: kind_counts(&store).await,
    })
}

/// Everything the graph knows about one entry, for its drawer.
///
/// The lineage defaults to belief-only: a reader opening an entry wants
/// to know what changed, not who tightened a sentence.
#[server]
pub async fn knowledge_detail(memory_id: String) -> Result<MemoryDetail, ServerError> {
    let store = server::use_state::<mcpm_core::Store>()
        .ok_or_else(|| ServerError::failed("Store not installed"))?;
    let h = store.memory_history(&memory_id, true).await.map_err(fail)?;
    let step = |s: &mcpm_core::HistoryStep| LinkDto {
        kind: s.via.map(|k| k.as_str().to_string()).unwrap_or_default(),
        rationale: s.rationale.clone(),
        outgoing: true,
        other: memory_dto(&s.memory),
    };
    Ok(MemoryDetail {
        entry: memory_dto(&h.anchor),
        supersedes: h.supersedes.iter().map(step).collect(),
        superseded_by: h.superseded_by.iter().map(step).collect(),
        relations: h
            .relations
            .iter()
            .map(|r| LinkDto {
                kind: r.kind.as_str().to_string(),
                rationale: r.rationale.clone(),
                outgoing: r.outgoing,
                other: memory_dto(&r.other),
            })
            .collect(),
        suggestions: h.suggestions.iter().map(|s| memory_dto(&s.other)).collect(),
    })
}

/// Core memory → wire DTO. One mapper, so the drawer and the list
/// cannot disagree about what an entry is.
#[cfg(feature = "server")]
fn memory_dto(m: &mcpm_core::Memory) -> MemoryDto {
    MemoryDto {
        id: m.id.clone(),
        level: m.level.clone(),
        subject_id: m.subject_id.clone(),
        subject_name: m.subject_name.clone(),
        kind: m.kind.as_str().to_string(),
        content: m.content.clone(),
        tags: m.tags.clone(),
        author: m.author.clone(),
        written: m.created_at.format("%b %-d %H:%M").to_string(),
        state: m.state.as_str().to_string(),
        touches: m.standing.touches,
        confirms: m.standing.confirms,
        disputes: m.standing.disputes,
        newer_fraction: m.standing.newer_fraction,
    }
}

/// Per-kind totals across the whole base, for the screen's header.
/// A failure here costs the header its numbers, not the page its
/// content — so it degrades to empty rather than failing the request.
#[cfg(feature = "server")]
async fn kind_counts(store: &mcpm_core::Store) -> Vec<KindCount> {
    let mut out = Vec::new();
    for kind in mcpm_core::MemoryKind::ALL {
        let found = store
            .search_memory(&mcpm_core::MemoryQuery {
                kinds: vec![kind],
                limit: 1,
                include_superseded: true,
                ..Default::default()
            })
            .await;
        if let Ok(page) = found {
            out.push(KindCount { kind: kind.as_str().to_string(), count: page.total });
        }
    }
    out
}

/// Create a tag with no want attached — a preset to file later ideas
/// under. Idempotent, so pressing the button twice is not an error.
#[server]
pub async fn create_tag(
    label: String,
    caller: server::Extension<Caller>,
) -> Result<TagDto, ServerError> {
    let store = server::use_state::<mcpm_core::Store>()
        .ok_or_else(|| ServerError::failed("Store not installed"))?;
    let tag = store
        .create_tag(&caller.name, &label)
        .await
        .map_err(fail)?;
    Ok(TagDto {
        name: tag.name,
        label: tag.label,
        uses: tag.uses,
    })
}

#[cfg(feature = "server")]
fn fail(e: mcpm_core::McpmError) -> ServerError {
    ServerError::failed(e.to_string())
}

/// Core document → wire DTO. One mapper, so the whitepaper and the
/// handoffs cannot disagree about what a revision is.
#[cfg(feature = "server")]
fn document_dto(d: &mcpm_core::DocumentView) -> DocumentDto {
    DocumentDto {
        id: d.id.clone(),
        kind: d.kind.as_str().to_string(),
        revision: d.revision,
        title: d.title.clone(),
        body: d.body.clone(),
        author: d.author.clone(),
        written: d.created_at.format("%b %-d %H:%M").to_string(),
    }
}

#[cfg(feature = "server")]
fn announcement_dto(a: &mcpm_core::Announcement) -> AnnouncementDto {
    AnnouncementDto {
        text: a.text.clone(),
        by: a.by.clone(),
        at: a.at.format("%b %-d %H:%M").to_string(),
        subject_id: a.subject_id.clone(),
        subject: a.subject.clone(),
    }
}

#[cfg(feature = "server")]
fn question_dto(q: &mcpm_core::QuestionRef) -> QuestionDto {
    QuestionDto {
        id: q.id.clone(),
        level: q.level.clone(),
        subject_id: q.subject_id.clone(),
        subject_name: q.subject_name.clone(),
        feature_id: q.feature_id.clone().unwrap_or_default(),
        body: q.body.clone(),
        author: q.author.clone(),
        assigned_to: q.assigned_to.clone().unwrap_or_default(),
        asked: q.created_at.format("%b %-d %H:%M").to_string(),
    }
}

#[cfg(feature = "server")]
fn comment_dto(c: &mcpm_core::CommentView) -> CommentDto {
    CommentDto {
        id: c.id.clone(),
        kind: c.kind.as_str().to_string(),
        body: c.body.clone(),
        author: c.author.clone(),
        assigned_to: c.assigned_to.clone().unwrap_or_default(),
        answers: c.answers.clone().unwrap_or_default(),
        resolved: c.resolved,
        resolved_by: c.resolved_by.clone().unwrap_or_default(),
        posted: c.created_at.format("%b %-d %H:%M").to_string(),
        edited: c.edited_at.is_some(),
        attachments: c.attachments.iter().map(attachment_dto).collect(),
    }
}

#[cfg(feature = "server")]
fn attachment_dto(a: &mcpm_core::AttachmentView) -> AttachmentDto {
    AttachmentDto {
        id: a.id.clone(),
        level: a.level.clone(),
        subject_id: a.subject_id.clone(),
        name: a.name.clone(),
        description: a.description.clone(),
        content_type: a.content_type.clone(),
        size_bytes: a.size_bytes,
        size: size_label(a.size_bytes),
        added_by: a.added_by.clone(),
        added: a.created_at.format("%b %-d %H:%M").to_string(),
        via_want_id: a.via_want.as_ref().map(|w| w.id.clone()).unwrap_or_default(),
        via_want: a.via_want.as_ref().map(|w| w.body.clone()).unwrap_or_default(),
        via_module_id: a.via_module.as_ref().map(|m| m.id.clone()).unwrap_or_default(),
        via_module: a.via_module.as_ref().map(|m| m.name.clone()).unwrap_or_default(),
        comment_id: a.comment.as_ref().map(|c| c.id.clone()).unwrap_or_default(),
        comment_author: a.comment.as_ref().map(|c| c.author.clone()).unwrap_or_default(),
        comment_excerpt: a.comment.as_ref().map(|c| c.excerpt.clone()).unwrap_or_default(),
    }
}

/// A byte count as people read one: "812 B", "12.4 KB", "3.1 MB".
pub fn size_label(bytes: i64) -> String {
    let b = bytes.max(0) as f64;
    if b < 1024.0 {
        format!("{bytes} B")
    } else if b < 1024.0 * 1024.0 {
        format!("{:.1} KB", b / 1024.0)
    } else if b < 1024.0 * 1024.0 * 1024.0 {
        format!("{:.1} MB", b / (1024.0 * 1024.0))
    } else {
        format!("{:.2} GB", b / (1024.0 * 1024.0 * 1024.0))
    }
}

/// Pre-format one ledger entry for display.
#[cfg(feature = "server")]
fn format_event(e: &mcpm_core::Event) -> EventDto {
    let p = &e.payload;
    let s = |key: &str| p.get(key).and_then(|v| v.as_str()).unwrap_or("").to_string();
    let (title, body) = match e.kind.as_str() {
        "feature_planned" => (
            format!(
                "Feature plan published: {} modules, {} dependencies",
                p["modules"].as_u64().unwrap_or(0),
                // Plans from before the graph carried a stage count and
                // no edge count; say what the row actually knows.
                p["edges"].as_u64().unwrap_or(0)
            ),
            if p["whitepaper"].as_bool().unwrap_or(false) {
                "With a whitepaper.".to_string()
            } else {
                String::new()
            },
        ),
        "plan_revised" => (
            "Plan revised".to_string(),
            p["ops"]
                .as_array()
                .map(|ops| {
                    ops.iter()
                        .filter_map(|o| o.as_str())
                        .collect::<Vec<_>>()
                        .join("; ")
                })
                .unwrap_or_default(),
        ),
        "module_claimed" => (format!("Module claimed: {}", s("module")), String::new()),
        "premature_claim" => (
            format!("Start denied for {}", s("module")),
            // Name what is actually in the way. How the gate works, and
            // who this event is for, are not the reader's to hold — they
            // can see the edges on the graph. (Rows from before the
            // graph carried stages here; their entries have `name` too.)
            {
                let names: Vec<String> = p["blocking"]
                    .as_array()
                    .map(|b| {
                        b.iter()
                            .filter_map(|m| m.get("name").and_then(|n| n.as_str()))
                            .map(|n| format!("'{n}'"))
                            .collect()
                    })
                    .unwrap_or_default();
                if names.is_empty() {
                    "A prerequisite is still open.".to_string()
                } else {
                    format!("Waiting on {}.", names.join(", "))
                }
            },
        ),
        "task_done" => (format!("Task checked off: {}", s("task")), s("note")),
        "task_skipped" => (format!("Task skipped: {}", s("task")), s("note")),
        "task_added" => (
            format!("Ad hoc task added: {}", s("task")),
            s("note"),
        ),
        // The announcement IS the headline; where it was said goes
        // under it.
        "announcement" => (s("text"), format!("On {}.", s("subject"))),
        "module_done" => (format!("Module complete: {}", s("module")), String::new()),
        "module_unlocked" => (
            format!("Ready: {}", s("module")),
            format!("Released by {}.", s("unlocked_by")),
        ),
        // History from before the graph.
        "stage_unlocked" => (
            format!("Stage unlocked: {}", s("stage")),
            format!("Opened by {}.", s("unlocked_by")),
        ),
        "document_written" => (
            format!(
                "{} written for {}",
                match s("kind").as_str() {
                    "whitepaper" => "Whitepaper",
                    _ => "Handoff",
                },
                s("subject")
            ),
            format!("Revision {}.", p["revision"].as_i64().unwrap_or(1)),
        ),
        "attachment_added" => (
            format!("File attached: {}", s("name")),
            {
                let size = size_label(p["size_bytes"].as_i64().unwrap_or(0));
                let d = s("description");
                if d.is_empty() { size } else { format!("{size} \u{b7} {d}") }
            },
        ),
        "attachment_described" => (format!("File described: {}", s("name")), s("description")),
        "comment_added" => (
            format!("{} commented on {}", s("author"), s("subject")),
            {
                let n = p["files"].as_u64().unwrap_or(0);
                let e = s("excerpt");
                if n > 0 { format!("{e} ({n} file{})", if n == 1 { "" } else { "s" }) } else { e }
            },
        ),
        "question_asked" => (
            format!(
                "Question on {} \u{2014} owed by {}",
                s("subject"),
                p["assigned_to"].as_str().filter(|a| !a.is_empty()).unwrap_or("anyone")
            ),
            s("excerpt"),
        ),
        "question_answered" => (
            format!("{} answered on {}", s("author"), s("subject")),
            {
                let e = s("excerpt");
                if p["released"].as_bool().unwrap_or(false) { format!("{e} \u{2014} module released.") } else { e }
            },
        ),
        "question_withdrawn" => (format!("Question withdrawn on {}", s("subject")), String::new()),
        "comment_edited" => (format!("Comment edited on {}", s("subject")), s("excerpt")),
        "comment_deleted" => (format!("Comment deleted on {}", s("subject")), String::new()),
        "attachment_removed" => (format!("File removed: {}", s("name")), String::new()),
        "blocker_reported" => (format!("Blocker on {}", s("module")), s("description")),
        "module_released" => (
            format!("Module released: {}", s("module")),
            format!("Reason: {}.", s("reason")),
        ),
        "feature_done" => (
            "Feature closed".to_string(),
            format!("'{}' completed every module.", s("name")),
        ),
        "feature_deleted" => (format!("Plan deleted: {}", s("feature")), String::new()),
        "feature_shelved" => (format!("Feature shelved: {}", s("feature")), String::new()),
        "feature_unshelved" => (format!("Feature unshelved: {}", s("feature")), String::new()),
        "want_deleted" => (format!("Want deleted: {}", s("body")), String::new()),
        "wants_promoted" => (
            format!(
                "Composed from {} want(s)",
                p["count"].as_u64().unwrap_or(0)
            ),
            if p["new_feature"].as_bool().unwrap_or(false) {
                "Planned straight out of the want pool.".to_string()
            } else {
                "Folded into an existing feature.".to_string()
            },
        ),
        "want_added" => (format!("Want captured: {}", s("body")), String::new()),
        "want_declined" => (
            format!("Want declined: {}", s("body")),
            format!("Reason: {}.", s("reason")),
        ),
        "want_reopened" => (format!("Want reopened: {}", s("body")), String::new()),
        "want_updated" => (format!("Want revised: {}", s("body")), String::new()),
        "memory_committed" => (
            format!("Memory committed at {} scope", s("level")),
            String::new(),
        ),
        // Never the token, and never its hash — this feed is read by
        // everything with console access. Who may act, and as whom.
        "key_issued" => (
            format!("API key issued for {}", s("agent")),
            format!("{} key {}.", s("role"), s("key_id")),
        ),
        "key_revoked" => (
            format!("API key revoked: {}", s("key_id")),
            String::new(),
        ),
        // Only a CHANGE is recorded, so this reads as the transition it
        // is: "attendance-modes is down" with the status line under it.
        "agent_health" => (
            format!("{} is {}", s("agent"), s("to")),
            format!("{} at {}.", s("detail"), s("url")),
        ),
        other => (other.to_string(), String::new()),
    };
    EventDto {
        seq: e.seq,
        time: e.ts.format("%H:%M").to_string(),
        kind: e.kind.clone(),
        agent: e.agent.clone(),
        feature_id: e.feature_id.clone(),
        subject_id: e.subject_id.clone(),
        title,
        body,
    }
}
