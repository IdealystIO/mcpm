//! Live view-model for the MCP Project Console.
//!
//! Nothing here is hardcoded: the `api` crate's wire reads (served by
//! mcpm-web, which reads the same Postgres store the MCP tools write)
//! are mapped into presentation-ready structs and handed to the views
//! by [`features`], [`agents`], [`wants`] and the rest. The console
//! re-renders when [`crate::state::Console::rev`] bumps after any of
//! them changed.
//!
//! The model is a set of caches, one per read, not one snapshot:
//!
//! - the **board** ([`apply_board`]) — every feature as a row of
//!   counts, the attention list, the newest events, the roster, the
//!   tags, the pool's counts. Always loaded; refetched on every tick.
//! - a **feature detail** per visited feature ([`apply_feature`]) —
//!   its modules, whitepaper and sources. A feature whose detail has
//!   not arrived still has its row: name, status, progress. Only the
//!   graph waits.
//! - a **module detail** per opened module ([`apply_module`]) — the
//!   handoff and history the drawer shows.
//! - a **feed** per feature whose activity tab was opened
//!   ([`apply_events`]) — pages of its ledger, newest first, merged
//!   as they arrive.
//! - the **pool page** ([`set_want_page`]) and the **open want**
//!   ([`set_open_want`]).
//!
//! Every `apply_*` returns whether anything changed, so a refetch that
//! lands identical data costs no re-render.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use runtime_core::{signal, Signal};

/// Execution status shared by features, modules, events, and agents.
/// `Violation` is a display state: a module whose claim bounced off the
/// gate because a prerequisite was still open (`premature_claim` on the
/// ledger).
#[derive(Clone, Copy, PartialEq, Eq, Default)]
pub enum Status {
    Done,
    Running,
    Blocked,
    Violation,
    #[default]
    Queued,
    Planning,
}

impl Status {
    pub fn label(self) -> &'static str {
        match self {
            Status::Done => "complete",
            Status::Running => "running",
            Status::Blocked => "blocked",
            Status::Violation => "rejected",
            Status::Queued => "queued",
            Status::Planning => "planning",
        }
    }
}

#[derive(Clone, PartialEq, Eq)]
pub struct Task {
    pub id: String,
    pub label: String,
    pub done: bool,
    pub added: bool,
}

/// One ledger entry that touched a module, as the drawer's history
/// shows it.
#[derive(Clone, PartialEq, Eq)]
pub struct ModuleEvent {
    pub title: String,
    pub body: String,
    pub at: String,
    pub from: String,
}

/// The current revision of a document: a feature's whitepaper or a
/// module's handoff. Markdown in `body`.
#[derive(Clone, PartialEq, Eq)]
pub struct Document {
    pub revision: i32,
    pub title: String,
    pub body: String,
    pub author: String,
    pub written: String,
}

impl Document {
    /// The muted provenance line under a rendered document.
    pub fn meta(&self) -> String {
        format!("rev {} \u{b7} {} \u{b7} {}", self.revision, self.author, self.written)
    }
}

/// One file attached to a feature or a want.
#[derive(Clone, PartialEq, Eq, Default)]
pub struct Attachment {
    pub id: String,
    pub name: String,
    pub description: String,
    pub content_type: String,
    /// "12.4 KB", formatted by the server.
    pub size: String,
    pub added_by: String,
    pub added: String,
    /// On a feature's list: the idea a file came in through, in its
    /// own words. Empty for the feature's own files.
    pub via_want_id: String,
    pub via_want: String,
    /// On a feature's list: the module the file is attached to.
    pub via_module_id: String,
    pub via_module: String,
    /// The comment it arrived with, when it did.
    pub comment_id: String,
    pub comment_author: String,
    pub comment_excerpt: String,
}

/// An open question, wherever it sits.
#[derive(Clone, PartialEq, Eq, Default)]
pub struct Question {
    pub id: String,
    /// feature | want | module
    pub level: String,
    pub subject_id: String,
    pub subject_name: String,
    pub feature_id: String,
    pub body: String,
    pub author: String,
    /// Empty for anyone.
    pub assigned_to: String,
    pub asked: String,
}

/// One comment in a discussion.
#[derive(Clone, PartialEq, Eq, Default)]
pub struct Comment {
    pub id: String,
    /// note | question | answer
    pub kind: String,
    pub body: String,
    pub author: String,
    pub assigned_to: String,
    pub answers: String,
    pub resolved: bool,
    pub resolved_by: String,
    pub posted: String,
    pub edited: bool,
    pub attachments: Vec<Attachment>,
}

impl Attachment {
    /// The muted line under a file name: size, type, who, when.
    pub fn meta(&self) -> String {
        let mut parts = vec![self.size.clone()];
        if !self.content_type.is_empty() && self.content_type != "application/octet-stream" {
            parts.push(self.content_type.clone());
        }
        parts.push(self.added_by.clone());
        parts.push(self.added.clone());
        parts.join(" \u{b7} ")
    }
}

/// What an agent last said it was doing, as a card or a roster line
/// shows it: the words, who, and when.
#[derive(Clone, PartialEq, Eq, Default)]
pub struct Word {
    pub text: String,
    pub by: String,
    pub at: String,
    /// The module or feature it was said on, by name.
    pub subject: String,
}

impl Word {
    /// `"text" · by · at`, the whole fact on one line.
    pub fn line(&self) -> String {
        format!("\u{201c}{}\u{201d} \u{b7} {} \u{b7} {}", self.text, self.by, self.at)
    }
}

#[derive(Clone, PartialEq, Eq)]
pub struct Module {
    pub id: String,
    pub name: String,
    pub description: String,
    pub status: Status,
    pub agent: String,
    pub spawned: String,
    /// Prerequisite module ids.
    pub depends_on: Vec<String>,
    /// The prerequisites not yet done. Empty means the gate is open.
    pub waiting_on: Vec<String>,
    /// Path prefixes this module writes to; empty = undeclared.
    pub owns: Vec<String>,
    /// Longest path from a root, 1-based — the graph's column.
    pub depth: usize,
    pub summary: Option<String>,
    pub block: Option<(String, String)>,
    /// Open questions on this module.
    pub open_questions: Vec<Question>,
    /// The latest thing an agent announced on this module.
    pub last_word: Option<Word>,
    /// When the holder last wrote to the ledger, as Unix seconds;
    /// `None` = no reading (unclaimed, or a holder that has never
    /// written). Measured against [`now_secs`] at read time.
    pub last_heard: Option<i64>,
    pub tasks: Vec<Task>,
}

/// How recently an agent must have written for a thing to be drawn as
/// MOVING rather than merely held. Twenty minutes: a build or an e2e
/// run fits inside it; a quota-parked box does not.
pub const LIVE_WINDOW_SECS: i64 = 20 * 60;

/// `true` when a reading exists and is inside [`LIVE_WINDOW_SECS`].
pub fn within_live_window(quiet_secs: i64) -> bool {
    (0..LIVE_WINDOW_SECS).contains(&quiet_secs)
}

/// Seconds between a stamp and `now`; -1 for no stamp. Never
/// negative: a stamp from the future reads as "just now".
pub fn quiet_secs(last_heard: Option<i64>, now: i64) -> i64 {
    last_heard.map(|t| (now - t).max(0)).unwrap_or(-1)
}

/// "3m", "2h 10m", "1d 4h" — how long something has been quiet, for
/// the card that is held but not moving.
pub fn quiet_label(secs: i64) -> String {
    let secs = secs.max(0);
    let (d, h, m) = (secs / 86_400, (secs % 86_400) / 3_600, (secs % 3_600) / 60);
    match (d, h, m) {
        (0, 0, m) => format!("{m}m"),
        (0, h, m) => format!("{h}h {m}m"),
        (d, h, _) => format!("{d}d {h}h"),
    }
}

/// What a module's drawer adds to its card. Read with [`module_detail`];
/// `None` until [`apply_module`] has landed it.
#[derive(Clone, PartialEq, Eq)]
pub struct ModuleDetail {
    pub handoff: Option<Document>,
    /// Every ledger entry whose subject is this module, oldest first.
    pub history: Vec<ModuleEvent>,
}

/// Where a module stands against its prerequisites — the axis the graph
/// colours, separate from [`Status`] (which is what the module itself
/// is doing).
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Readiness {
    /// Every prerequisite is done: ready to claim, or already claimed.
    Open,
    /// At least one prerequisite is still open.
    Waiting,
    /// The module itself is done.
    Done,
}

#[derive(Clone, PartialEq, Eq, Default)]
pub struct EventItem {
    /// Ledger sequence. The only orderable key an event carries — the
    /// `time` string is a clock reading, so merging two features' feeds
    /// on it would interleave them wrong across a day boundary.
    pub seq: i64,
    pub time: String,
    pub kind: String,
    pub status: Status,
    pub title: String,
    pub body: String,
    pub agent: String,
    pub tool: String,
}

/// One feature's ledger as far as it has been read: newest first, and
/// whether the oldest row here is the oldest there is.
#[derive(Clone, PartialEq, Eq)]
pub struct Feed {
    pub events: Vec<EventItem>,
    pub exhausted: bool,
}

impl Feed {
    /// The paging cursor: the oldest seq loaded, for the next older page.
    pub fn oldest(&self) -> Option<i64> {
        self.events.last().map(|e| e.seq)
    }
}

#[derive(Clone, PartialEq, Eq)]
pub struct AgentRow {
    pub id: String,
    pub level: String,
    /// Running while it holds a claim.
    pub state: Status,
    pub uptime: String,
    pub scope: String,
    /// Whether the machine behind it is up, when a check is registered.
    pub health: Option<AgentHealthRow>,
    /// When it last registered or announced, as a stamp.
    pub last_seen: String,
    /// Its newest ledger write, as Unix seconds; `None` = never wrote.
    pub last_heard: Option<i64>,
    /// The latest thing it announced.
    pub last_word: Option<Word>,
}

impl AgentRow {
    /// Spinning while it holds a claim, wrote to the ledger inside the
    /// live window, and its box — if checked — is up. A quota-parked
    /// box holds its claim and says nothing: still dot.
    pub fn live_at(&self, now: i64) -> bool {
        self.state == Status::Running
            && within_live_window(quiet_secs(self.last_heard, now))
            && self.health.as_ref().is_none_or(|h| h.up)
    }

    /// "quiet 47m" once a held claim has gone silent past the window.
    pub fn quiet_at(&self, now: i64) -> Option<String> {
        let quiet = quiet_secs(self.last_heard, now);
        (self.state == Status::Running && quiet >= LIVE_WINDOW_SECS)
            .then(|| format!("quiet {}", quiet_label(quiet)))
    }
}

/// The server's verdict on an agent's machine, ready to draw: the tone
/// is borrowed from [`Status`] because the dot and badge palette is
/// keyed on it, and the line is the whole fact ("down since Sep 18
/// 14:02 · HTTP 503").
#[derive(Clone, PartialEq, Eq)]
pub struct AgentHealthRow {
    pub tone: Status,
    pub label: String,
    pub line: String,
    pub up: bool,
}

/// One idea in the pool. `state` is derived server-side: an open want
/// is loose, a promoted one names the feature(s) that absorbed it, a
/// declined one carries the reason it was turned down.
#[derive(Clone, PartialEq, Eq, Default)]
pub struct Want {
    pub id: String,
    pub body: String,
    pub tags: Vec<String>,
    pub state: WantState,
    pub note: String,
    pub author: String,
    pub captured: String,
    /// `(feature name, how this want was read into it)`.
    pub features: Vec<(String, String)>,
    /// The want's files. Only the drawer's read carries them; a page
    /// of the pool leaves this empty.
    pub attachments: Vec<Attachment>,
}

#[derive(Clone, Copy, PartialEq, Eq, Default)]
pub enum WantState {
    #[default]
    Open,
    Promoted,
    Declined,
}

impl WantState {
    /// The console's shared status vocabulary, so wants pick up the same
    /// badge and dot colors as everything else.
    pub fn status(self) -> Status {
        match self {
            WantState::Open => Status::Planning,
            WantState::Promoted => Status::Done,
            WantState::Declined => Status::Queued,
        }
    }
}

/// One tag in the registry.
#[derive(Clone, PartialEq, Eq)]
pub struct TagRow {
    pub name: String,
    pub label: String,
    pub uses: i64,
}

/// One want a feature was composed from, seen from the feature.
///
/// No rationale here: the link's rationale is read off the want itself
/// (`Want::features`) when its drawer opens, so a feature's origin list
/// and the pool cannot disagree about why an idea was read in.
#[derive(Clone, PartialEq, Eq, Default)]
pub struct WantSource {
    pub id: String,
    pub body: String,
}

/// One feature: its board row always, its graph once visited.
///
/// The counts come from the board's rollup and not from `modules`, so
/// a feature row is right before its detail has loaded and stays
/// consistent with the rail after.
#[derive(Clone, PartialEq, Eq)]
pub struct Feature {
    pub id: String,
    pub name: String,
    pub description: String,
    pub status: Status,
    /// Parked. Shown as queued like any other idle feature; the menu
    /// offers the reverse verb.
    pub shelved: bool,
    pub agent: String,
    pub elapsed: String,
    pub modules_done: usize,
    pub modules_total: usize,
    pub modules_ready: usize,
    pub tasks_done: usize,
    pub tasks_total: usize,
    pub tasks_added: usize,
    /// Whether the detail below has arrived. Until it has, `modules` is
    /// empty because nothing has been read — not because none exist.
    pub detail_loaded: bool,
    /// Topological order: every module after all of its prerequisites,
    /// ties by depth then name. Shared with the detail cache.
    pub modules: Rc<Vec<Rc<Module>>>,
    pub whitepaper: Option<Rc<Document>>,
    /// The loose ideas this feature was composed from.
    pub sources: Rc<Vec<WantSource>>,
    /// The feature's files, then its source wants' files.
    pub attachments: Rc<Vec<Attachment>>,
    /// Open questions on the feature itself.
    pub open_questions: Rc<Vec<Question>>,
    /// The newest announcement anywhere in the feature.
    pub last_word: Option<Word>,
    /// Modules an agent holds right now.
    pub modules_running: usize,
    /// When a HOLDER of one of its modules last wrote to the ledger,
    /// as Unix seconds; `None` = nobody holds one, or no holder has
    /// written.
    pub last_heard: Option<i64>,
    /// The roadmap item it delivers part of. `None` is LOOSE, which is
    /// normal and gets no badge — only a binding is worth showing.
    pub roadmap_item: Option<RoadEdge>,
    /// "MMM D HH:MM" of when it shipped, or empty. A feature can be
    /// done with this empty: the work landed, the roadmap has not let
    /// it out.
    pub released: String,
    /// Unshipped roadmap items holding its release. Non-empty is the
    /// HELD badge, and it clears itself when they ship (rule 21).
    pub held_by: Vec<RoadEdge>,
}

/// Where the product is going: one item's worth, as the screen draws
/// it. `state` is the SERVER's derivation — the console never
/// recomputes it, for the same reason readiness is not recomputed here.
#[derive(Clone, PartialEq, Debug)]
pub struct RoadItem {
    pub id: String,
    pub name: String,
    /// One paragraph, product terms. The card's body.
    pub intent: String,
    /// Optional long form, Markdown.
    pub vision: String,
    pub horizon: String,
    pub state: RoadState,
    /// "MMM D HH:MM" of when it shipped, or empty.
    pub shipped_at: String,
    pub shipped_by: String,
    pub shelved: bool,
    pub depth: i32,
    pub depends_on: Vec<RoadEdge>,
    /// What waits on this one.
    pub unlocks: Vec<RoadEdge>,
    pub features: Vec<RoadFeature>,
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub struct RoadEdge {
    pub item_id: String,
    pub name: String,
    /// A hard edge holds the WORK as well as the ship door.
    pub hard: bool,
    pub shipped: bool,
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub struct RoadFeature {
    pub id: String,
    pub name: String,
    pub status: String,
    pub released: bool,
    pub modules_done: usize,
    pub modules_total: usize,
}

/// A roadmap item's derived state. Ordered by how much it asks of the
/// reader: `Ready` is the one with a verb attached.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum RoadState {
    Future,
    Held,
    Active,
    Ready,
    Shipped,
    Shelved,
}

impl RoadState {
    pub fn parse(s: &str) -> RoadState {
        match s {
            "held" => RoadState::Held,
            "active" => RoadState::Active,
            "ready" => RoadState::Ready,
            "shipped" => RoadState::Shipped,
            "shelved" => RoadState::Shelved,
            _ => RoadState::Future,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            RoadState::Future => "Future",
            RoadState::Held => "Held",
            RoadState::Active => "Building",
            RoadState::Ready => "Ready to ship",
            RoadState::Shipped => "Shipped",
            RoadState::Shelved => "Shelved",
        }
    }

    /// The status this state reads as, so the roadmap borrows the
    /// board's tones rather than inventing a second palette for the
    /// same six ideas.
    pub fn status(self) -> Status {
        match self {
            RoadState::Future | RoadState::Shelved => Status::Queued,
            RoadState::Held => Status::Violation,
            RoadState::Active => Status::Running,
            RoadState::Ready => Status::Planning,
            RoadState::Shipped => Status::Done,
        }
    }
}

/// The roadmap as the screen holds it.
#[derive(Clone, PartialEq, Debug, Default)]
pub struct Roadmap {
    /// Topological order, as the server returned it.
    pub items: Vec<RoadItem>,
    /// Features bound to no item. Normal — a sprint, a bugfix.
    pub loose_features: Vec<RoadFeature>,
}

impl Roadmap {
    pub fn item(&self, id: &str) -> Option<&RoadItem> {
        self.items.iter().find(|i| i.id == id)
    }

    /// How many items are waiting on something that has not shipped —
    /// the count the nav badge carries, because a held item is the one
    /// state on this screen a person may need to act on.
    pub fn ready_count(&self) -> usize {
        self.items.iter().filter(|i| i.state == RoadState::Ready).count()
    }
}

impl RoadItem {
    /// `2/3 features released`, or the blank state.
    pub fn feature_line(&self) -> String {
        if self.features.is_empty() {
            return "No features bound".to_string();
        }
        let out = self.features.iter().filter(|f| f.released).count();
        format!("{out}/{} features released", self.features.len())
    }

}

// ---------------------------------------------------------------------
// Derived rollups
// ---------------------------------------------------------------------

impl Feature {
    pub fn fraction(&self) -> f32 {
        if self.tasks_total == 0 {
            0.0
        } else {
            self.tasks_done as f32 / self.tasks_total as f32
        }
    }

    pub fn pct_label(&self) -> String {
        format!("{}%", (self.fraction() * 100.0).round() as i32)
    }

    pub fn module_count(&self) -> (usize, usize) {
        (self.modules_done, self.modules_total)
    }

    pub fn task_count(&self) -> (usize, usize, usize) {
        (self.tasks_done, self.tasks_total, self.tasks_added)
    }

    /// Modules an agent could claim right now.
    pub fn ready_count(&self) -> usize {
        self.modules_ready
    }

    /// The one-line rollup every feature row and header shows.
    pub fn meta_line(&self) -> String {
        let (done, total) = self.module_count();
        format!(
            "{done}/{total} modules \u{b7} {} ready \u{b7} {} of tasks",
            self.ready_count(),
            self.pct_label(),
        )
    }

    /// Index of a module in [`Feature::modules`] by id.
    pub fn module_index(&self, id: &str) -> Option<usize> {
        self.modules.iter().position(|m| m.id == id)
    }

    /// A module's name by id, for the graph's "waits on" lines. Falls
    /// back to the id so a dangling reference is visible rather than
    /// blank.
    pub fn module_name(&self, id: &str) -> String {
        self.module_index(id)
            .map(|i| self.modules[i].name.clone())
            .unwrap_or_else(|| id.to_string())
    }

    /// Names of the given module ids, joined for display.
    pub fn module_names(&self, ids: &[String]) -> String {
        ids.iter()
            .map(|id| self.module_name(id))
            .collect::<Vec<_>>()
            .join(", ")
    }
}

impl Module {
    /// Where the module stands against its prerequisites.
    pub fn readiness(&self) -> Readiness {
        if self.status == Status::Done {
            Readiness::Done
        } else if self.waiting_on.is_empty() {
            Readiness::Open
        } else {
            Readiness::Waiting
        }
    }

    pub fn task_label(&self) -> String {
        format!(
            "{}/{} tasks",
            self.tasks.iter().filter(|t| t.done).count(),
            self.tasks.len()
        )
    }

    /// Held AND moving: an agent has it and wrote to the ledger inside
    /// the live window, measured at `now` (see [`now_secs`]).
    pub fn live_at(&self, now: i64) -> bool {
        self.status == Status::Running && within_live_window(quiet_secs(self.last_heard, now))
    }

    /// Held but NOT moving: the holder has been silent past the
    /// window. `None` while it is live, unclaimed, or unmeasured.
    pub fn quiet_for_at(&self, now: i64) -> Option<String> {
        let quiet = quiet_secs(self.last_heard, now);
        (self.status == Status::Running && quiet >= LIVE_WINDOW_SECS).then(|| quiet_label(quiet))
    }

    /// The agent slot as a card shows it — who holds the module, and
    /// for how long it has been quiet — with the tone that goes with it.
    pub fn agent_line_at(&self, now: i64) -> (String, AgentTone) {
        match self.quiet_for_at(now) {
            Some(quiet) => (format!("{} \u{b7} quiet {quiet}", self.agent), AgentTone::Quiet),
            None => (
                self.agent.clone(),
                match self.agent.as_str() {
                    "ready" => AgentTone::Ready,
                    "waiting" => AgentTone::Waiting,
                    _ => AgentTone::Plain,
                },
            ),
        }
    }
}

/// How a module's agent slot reads: the holder, the readiness word
/// standing in for one, or a holder that has gone quiet.
#[derive(Clone, Copy, PartialEq, Eq, Default)]
pub enum AgentTone {
    #[default]
    Plain,
    Ready,
    Waiting,
    Quiet,
}

impl Feature {
    /// Something in it is moving right now: a module is held and the
    /// ledger heard from the feature inside the live window.
    pub fn live_at(&self, now: i64) -> bool {
        self.modules_running > 0 && within_live_window(quiet_secs(self.last_heard, now))
    }
}

/// Agents holding a claim right now.
pub fn active_agent_count(agents: &[Rc<AgentRow>]) -> usize {
    agents.iter().filter(|a| a.state == Status::Running).count()
}

/// Agents whose machine is known to be up — or, for one with no health
/// check registered, one that holds a claim, which is the best the
/// ledger alone can say.
pub fn live_agent_count(agents: &[Rc<AgentRow>]) -> usize {
    agents
        .iter()
        .filter(|a| match &a.health {
            Some(h) => h.up,
            None => a.state == Status::Running,
        })
        .count()
}

/// The features still being worked, as indices into the list given.
pub fn in_play_of(features: &[Rc<Feature>]) -> Vec<usize> {
    features
        .iter()
        .enumerate()
        .filter(|(_, f)| f.status != Status::Done)
        .map(|(i, _)| i)
        .collect()
}

/// The rail's rows over the list given: everything still in play,
/// plus whichever one is selected — see [`rail_features`].
pub fn rail_features_of(features: &[Rc<Feature>], selected: usize) -> Vec<usize> {
    features
        .iter()
        .enumerate()
        .filter(|(i, f)| f.status != Status::Done || *i == selected)
        .map(|(i, _)| i)
        .collect()
}

/// The all-features screen's rows over the list given — see
/// [`filter_features`].
pub fn filter_features_of(features: &[Rc<Feature>], query: &str, status: &str) -> Vec<usize> {
    let needle = query.trim().to_lowercase();
    features
        .iter()
        .enumerate()
        .filter(|(_, f)| match status {
            "open" => f.status != Status::Done,
            "done" => f.status == Status::Done,
            _ => true,
        })
        .filter(|(_, f)| needle.is_empty() || f.name.to_lowercase().contains(&needle))
        .map(|(i, _)| i)
        .collect()
}

// ---------------------------------------------------------------------
// The live cache
// ---------------------------------------------------------------------

/// One feature's detail as mapped from the wire, shared by reference
/// into every [`Feature`] rebuilt from the board.
struct Detail {
    raw: api::FeatureDetail,
    description: String,
    modules: Rc<Vec<Rc<Module>>>,
    whitepaper: Option<Rc<Document>>,
    sources: Rc<Vec<WantSource>>,
    attachments: Rc<Vec<Attachment>>,
    open_questions: Rc<Vec<Question>>,
}

struct Current {
    board: Option<api::Board>,
    /// The server's clock minus this process's, in seconds, from the
    /// newest board. Every liveness reading is taken against the
    /// server's idea of now, so a reader whose machine is minutes off
    /// still sees the same spinners the host would.
    clock_offset: Option<i64>,
    project_name: String,
    features: Rc<Vec<Rc<Feature>>>,
    agents: Rc<Vec<Rc<AgentRow>>>,
    tags: Rc<Vec<TagRow>>,
    attention: Rc<Vec<Attention>>,
    recent: Rc<Vec<EventItem>>,
    want_counts: (usize, usize, usize),
    /// By feature id.
    details: HashMap<String, Detail>,
    /// By module id.
    modules: HashMap<String, (api::ModuleDetail, Rc<ModuleDetail>)>,
    /// By feature id.
    feeds: HashMap<String, Rc<Feed>>,
    /// The page of the pool the wants screen is showing.
    want_page: Option<api::WantPage>,
    wants: Rc<Vec<Want>>,
    want_total: usize,
    /// The idea whose drawer is open, whatever page it is on.
    open_want: Option<(api::WantDto, Want)>,
    /// Discussions by subject id, as far as each has been read.
    threads: HashMap<String, (api::CommentPage, Rc<Vec<Comment>>)>,
    /// Every open question in the project, oldest first.
    questions: Rc<Vec<Question>>,
    /// The name this console's writes are recorded under.
    you: String,
    /// The roadmap, as far as it has been read. Its own fetch, not the
    /// board's: the board is read on every tick and must stay scalars.
    roadmap_raw: Option<api::RoadmapDto>,
    roadmap: Rc<Roadmap>,
    loaded: bool,
}

thread_local! {
    static CURRENT: RefCell<Current> = RefCell::new(Current {
        board: None,
        clock_offset: None,
        project_name: "control-center".into(),
        features: Rc::new(Vec::new()),
        agents: Rc::new(Vec::new()),
        tags: Rc::new(Vec::new()),
        attention: Rc::new(Vec::new()),
        recent: Rc::new(Vec::new()),
        want_counts: (0, 0, 0),
        details: HashMap::new(),
        modules: HashMap::new(),
        feeds: HashMap::new(),
        want_page: None,
        wants: Rc::new(Vec::new()),
        want_total: 0,
        open_want: None,
        threads: HashMap::new(),
        questions: Rc::new(Vec::new()),
        you: String::new(),
        roadmap_raw: None,
        roadmap: Rc::new(Roadmap { items: Vec::new(), loose_features: Vec::new() }),
        loaded: false,
    });
}

pub fn features() -> Rc<Vec<Rc<Feature>>> {
    CURRENT.with(|c| c.borrow().features.clone())
}

/// This process's clock, in seconds. On web it counts from page load,
/// which is why it is only ever read through [`now_secs`].
fn local_secs() -> i64 {
    (runtime_core::time::now_micros() / 1_000_000) as i64
}

/// Now, as Unix seconds on the SERVER's clock: the local clock plus
/// the offset the newest board established. Before any board has
/// arrived there is nothing to measure, and the local clock stands in.
pub fn now_secs() -> i64 {
    local_secs() + CURRENT.with(|c| c.borrow().clock_offset).unwrap_or(0)
}

/// The feature at `index`'s id, or `None` past the end.
pub fn feature_id_at(index: usize) -> Option<String> {
    features().get(index).map(|f| f.id.clone())
}

/// Whether a feature's detail has been read.
pub fn has_detail(feature_id: &str) -> bool {
    CURRENT.with(|c| c.borrow().details.contains_key(feature_id))
}

/// A module's drawer contents, once [`apply_module`] has landed them.
pub fn module_detail(module_id: &str) -> Option<Rc<ModuleDetail>> {
    CURRENT.with(|c| c.borrow().modules.get(module_id).map(|(_, d)| d.clone()))
}

/// Whether a subject's discussion has been read at all.
pub fn has_thread(subject_id: &str) -> bool {
    CURRENT.with(|c| c.borrow().threads.contains_key(subject_id))
}

/// One comment by id, from whichever thread on screen holds it.
pub fn comment_by_id(id: &str) -> Option<Comment> {
    CURRENT.with(|c| {
        c.borrow()
            .threads
            .values()
            .flat_map(|(_, t)| t.iter())
            .find(|x| x.id == id)
            .cloned()
    })
}

/// The name this console's writes are recorded under.
pub fn you() -> String {
    CURRENT.with(|c| c.borrow().you.clone())
}

/// A feature's ledger as far as it has been read.
pub fn feed(feature_id: &str) -> Option<Rc<Feed>> {
    CURRENT.with(|c| c.borrow().feeds.get(feature_id).cloned())
}

/// Where an [`Attention`] row goes when it is opened.
#[derive(Clone, PartialEq, Eq, Default)]
pub enum AttentionTarget {
    /// A module drawer: `(feature index, module id)`. The module is
    /// addressed by id because its feature's graph may not be loaded
    /// yet when the row is pressed.
    Module(usize, String),
    /// The want pool screen.
    #[default]
    Pool,
    /// An open question: `(level, subject id, feature id)` — see
    /// `Console::open_question`.
    Question(String, String, String),
}

/// One row on the overview's attention list.
///
/// Every entry is a **current condition** — a claim still bouncing off
/// a gate, a module still waiting on its manager, ideas still
/// uncomposed — never the memory of one that has since cleared
/// (UX_GUIDELINES rule 21). The server derives each from the module's
/// own state rather than from the newest event of some kind, so the
/// row disappears by itself the moment the work moves.
#[derive(Clone, PartialEq, Eq, Default)]
pub struct Attention {
    /// Short kind word for the row's pill.
    pub kind: &'static str,
    /// Status whose tone colors the row.
    pub status: Status,
    /// What is stuck, in one line.
    pub title: String,
    /// Where it is stuck — the feature's name, or the pool's.
    pub place: String,
    /// What opening the row goes to.
    pub target: AttentionTarget,
}

/// The first feature still in play, for the console to land on.
///
/// Without this the console opens on index 0, which on a mature project
/// is the oldest feature and almost certainly a finished one — the
/// reader's first sight of the board would be work nobody is doing.
pub fn first_open_feature() -> Option<usize> {
    features().iter().position(|f| f.status != Status::Done)
}

/// The page of the pool the wants screen is showing.
pub fn wants() -> Rc<Vec<Want>> {
    CURRENT.with(|c| c.borrow().wants.clone())
}

/// One idea by id: the one whose drawer is open, else from the page on
/// screen. The open one first because it is the fuller read — it
/// carries the want's files, which a page row does not — and because
/// a feature's origin list can open onto an idea no loaded page holds.
pub fn want_by_id(id: &str) -> Option<Want> {
    CURRENT.with(|c| {
        let cur = c.borrow();
        cur.open_want
            .as_ref()
            .filter(|(_, w)| w.id == id)
            .map(|(_, w)| w.clone())
            .or_else(|| cur.wants.iter().find(|w| w.id == id).cloned())
    })
}

/// One attachment by id, wherever it is on screen: on a feature's
/// list (its own or a source want's) or on the open want.
pub fn attachment_by_id(id: &str) -> Option<Attachment> {
    CURRENT.with(|c| {
        let cur = c.borrow();
        cur.features
            .iter()
            .flat_map(|f| f.attachments.iter())
            .find(|a| a.id == id)
            .cloned()
            .or_else(|| {
                cur.open_want
                    .as_ref()
                    .and_then(|(_, w)| w.attachments.iter().find(|a| a.id == id).cloned())
            })
    })
}

/// The tag registry, most-used first.
pub fn tags() -> Rc<Vec<TagRow>> {
    CURRENT.with(|c| c.borrow().tags.clone())
}

/// Just the slugs — what `#tag` completion and highlighting match on.
pub fn tag_names() -> Vec<String> {
    tags().iter().map(|t| t.name.clone()).collect()
}

/// Whether the board has arrived at least once (distinguishes
/// "connecting" from "the project genuinely has no features yet").
pub fn loaded() -> bool {
    CURRENT.with(|c| c.borrow().loaded)
}

// ---------------------------------------------------------------------
// What the screen subscribes to
// ---------------------------------------------------------------------

/// The cache as signals: one per kind of thing on screen, written by
/// [`publish`] after a read lands. Every write is equality-guarded, so
/// a poll that returns the same board wakes nobody, and a tick on one
/// feature's feed wakes only the readers of feeds. A view reads these
/// through a `memo` of the one entity it draws and restyles in place;
/// nothing keys a `switch` on "something changed somewhere".
///
/// Created once in `app()` — a signal made inside a component body is
/// owned by that body's scope and dies with it, which is why these are
/// not lazily made in the cache.
#[derive(Clone, Copy)]
pub struct Data {
    pub project: Signal<String>,
    /// Whether the board has arrived at least once.
    pub loaded: Signal<bool>,
    /// The name this console's writes are recorded under.
    pub you: Signal<String>,
    /// Every feature, oldest first, with its graph once visited.
    pub features: Signal<Rc<Vec<Rc<Feature>>>>,
    pub agents: Signal<Rc<Vec<Rc<AgentRow>>>>,
    pub attention: Signal<Rc<Vec<Attention>>>,
    /// The project-wide ledger's newest entries, newest first.
    pub recent: Signal<Rc<Vec<EventItem>>>,
    pub tags: Signal<Rc<Vec<TagRow>>>,
    /// `(loose, composed, declined)`.
    pub want_counts: Signal<(usize, usize, usize)>,
    /// Feeds by feature id, as far as each has been read.
    pub feeds: Signal<Rc<HashMap<String, Rc<Feed>>>>,
    /// Drawer contents by module id.
    pub modules: Signal<Rc<HashMap<String, Rc<ModuleDetail>>>>,
    /// Discussions by subject id.
    pub threads: Signal<Rc<HashMap<String, Rc<Vec<Comment>>>>>,
    /// The page of the pool the wants screen is showing.
    pub wants: Signal<Rc<Vec<Want>>>,
    pub want_total: Signal<usize>,
    /// The idea whose drawer is open, whatever page it is on.
    pub open_want: Signal<Option<Rc<Want>>>,
    /// Now on the server's clock, as Unix seconds, in steps of
    /// [`CLOCK_STEP_SECS`]. What every liveness reading subscribes to,
    /// so a spinner stops by itself when its box has been quiet for
    /// the window — without a fetch, and without rebuilding anything
    /// but the ring.
    pub clock: Signal<i64>,
    /// Where the product is going. Fetched when the roadmap screen is
    /// on or a feature's header needs its binding, refetched on a
    /// roadmap or feature tick.
    pub roadmap: Signal<Rc<Roadmap>>,
}

/// How often [`Data::clock`] moves. Coarse on purpose: a reading only
/// changes meaning at the twenty-minute window and the minute label.
pub const CLOCK_STEP_SECS: i64 = 30;

impl Data {
    pub fn new() -> Self {
        Self {
            project: signal("control-center".to_string()),
            loaded: signal(false),
            you: signal(String::new()),
            features: signal(Rc::new(Vec::new())),
            agents: signal(Rc::new(Vec::new())),
            attention: signal(Rc::new(Vec::new())),
            recent: signal(Rc::new(Vec::new())),
            tags: signal(Rc::new(Vec::new())),
            want_counts: signal((0, 0, 0)),
            feeds: signal(Rc::new(HashMap::new())),
            modules: signal(Rc::new(HashMap::new())),
            threads: signal(Rc::new(HashMap::new())),
            wants: signal(Rc::new(Vec::new())),
            want_total: signal(0),
            open_want: signal(None),
            clock: signal(0),
            roadmap: signal(Rc::new(Roadmap::default())),
        }
    }

    /// Move the clock to the current step. A no-op inside a step.
    pub fn tick_clock(&self) {
        self.clock.set(clock_step(now_secs()));
    }

    /// One feature by index, as a `Copy` closure over the root signal.
    /// A view's memos and `rx!` bindings call this rather than chaining
    /// off a memo of their own: a memo's first compute is a staged
    /// write, and a dependent built in the same turn would read the
    /// committed (empty) value and compute twice.
    pub fn feature(self, index: usize) -> impl Fn() -> Option<Rc<Feature>> + Copy {
        move || self.features.get().get(index).cloned()
    }

    /// One module by feature and module index — see [`Data::feature`].
    pub fn module(self, feature: usize, module: usize) -> impl Fn() -> Option<Rc<Module>> + Copy {
        move || self.features.get().get(feature).and_then(|f| f.modules.get(module).cloned())
    }

    /// One agent by name — see [`Data::feature`].
    pub fn agent(self, name: String) -> impl Fn() -> Option<Rc<AgentRow>> + Clone {
        move || self.agents.get().iter().find(|a| a.id == name).cloned()
    }
}

/// `now` rounded down to its [`CLOCK_STEP_SECS`] step.
pub fn clock_step(now: i64) -> i64 {
    now - now.rem_euclid(CLOCK_STEP_SECS)
}

impl Default for Data {
    fn default() -> Self {
        Self::new()
    }
}

/// Copy the cache into the signals. Every set compares against what
/// the signal holds, so only what changed reaches the screen. The
/// cache borrow is released before the first write: a write stages a
/// flush, and nothing that runs in it may find the cache borrowed.
pub fn publish(data: Data) {
    let (project, loaded, you, features, agents, attention, recent, tags, want_counts) =
        CURRENT.with(|c| {
            let cur = c.borrow();
            (
                cur.project_name.clone(),
                cur.loaded,
                cur.you.clone(),
                cur.features.clone(),
                cur.agents.clone(),
                cur.attention.clone(),
                cur.recent.clone(),
                cur.tags.clone(),
                cur.want_counts,
            )
        });
    let (feeds, modules, threads, wants, want_total, open_want) = CURRENT.with(|c| {
        let cur = c.borrow();
        (
            Rc::new(cur.feeds.clone()),
            Rc::new(cur.modules.iter().map(|(k, (_, d))| (k.clone(), d.clone())).collect::<HashMap<_, _>>()),
            Rc::new(cur.threads.iter().map(|(k, (_, t))| (k.clone(), t.clone())).collect::<HashMap<_, _>>()),
            cur.wants.clone(),
            cur.want_total,
            cur.open_want.as_ref().map(|(_, w)| Rc::new(w.clone())),
        )
    });
    data.project.set(project);
    data.loaded.set(loaded);
    data.you.set(you);
    data.features.set(features);
    data.agents.set(agents);
    data.attention.set(attention);
    data.recent.set(recent);
    data.tags.set(tags);
    data.want_counts.set(want_counts);
    data.feeds.set(feeds);
    data.modules.set(modules);
    data.threads.set(threads);
    data.wants.set(wants);
    data.want_total.set(want_total);
    data.open_want.set(open_want);
    data.roadmap.set(CURRENT.with(|c| c.borrow().roadmap.clone()));
    data.tick_clock();
}

// ---------------------------------------------------------------------
// Applying reads
// ---------------------------------------------------------------------

/// Store the board. Returns true when the data changed (callers bump
/// `Console::rev` to re-render).
pub fn apply_board(mut board: api::Board) -> bool {
    CURRENT.with(|c| {
        let mut cur = c.borrow_mut();
        // The clock is the one field that differs on every read; it
        // is taken off the board before the comparison so an idle
        // poll is a no-op, and kept as an offset for `now_secs`.
        if board.now > 0 {
            cur.clock_offset = Some(board.now - local_secs());
        }
        board.now = 0;
        let changed = cur.board.as_ref() != Some(&board) || !cur.loaded;
        if changed {
            cur.project_name = board.project.name.clone();
            cur.agents = Rc::new(board.agents.iter().map(|a| Rc::new(map_agent(a))).collect());
            cur.tags = Rc::new(
                board
                    .tags
                    .iter()
                    .map(|t| TagRow {
                        name: t.name.clone(),
                        label: t.label.clone(),
                        uses: t.uses,
                    })
                    .collect(),
            );
            cur.recent = Rc::new(board.recent.iter().map(map_event).collect());
            cur.questions = Rc::new(board.questions.iter().map(map_question).collect());
            cur.you = board.you.clone();
            cur.want_counts = (
                board.wants.open.max(0) as usize,
                board.wants.promoted.max(0) as usize,
                board.wants.declined.max(0) as usize,
            );
            cur.board = Some(board);
            cur.loaded = true;
            rebuild(&mut cur);
        }
        changed
    })
}

/// Store one feature's detail. Returns true when it changed.
pub fn apply_feature(detail: api::FeatureDetail) -> bool {
    CURRENT.with(|c| {
        let mut cur = c.borrow_mut();
        if cur.details.get(&detail.id).is_some_and(|d| d.raw == detail) {
            return false;
        }
        let mapped = Detail {
            description: detail.description.clone(),
            modules: Rc::new(detail.modules.iter().map(|m| Rc::new(map_module(m))).collect()),
            whitepaper: detail.whitepaper.as_ref().map(|d| Rc::new(map_document(d))),
            sources: Rc::new(
                detail
                    .sources
                    .iter()
                    .map(|s| WantSource { id: s.want_id.clone(), body: s.body.clone() })
                    .collect(),
            ),
            attachments: Rc::new(detail.attachments.iter().map(map_attachment).collect()),
            open_questions: Rc::new(detail.open_questions.iter().map(map_question).collect()),
            raw: detail,
        };
        cur.details.insert(mapped.raw.id.clone(), mapped);
        rebuild(&mut cur);
        true
    })
}

/// Store the roadmap. Returns true when it changed — an idle poll
/// returns an identical DTO and nothing on screen is rebuilt for it.
pub fn apply_roadmap(road: api::RoadmapDto) -> bool {
    CURRENT.with(|c| {
        let mut cur = c.borrow_mut();
        if cur.roadmap_raw.as_ref() == Some(&road) {
            return false;
        }
        cur.roadmap = Rc::new(Roadmap {
            items: road
                .items
                .iter()
                .map(|i| RoadItem {
                    id: i.id.clone(),
                    name: i.name.clone(),
                    intent: i.intent.clone(),
                    vision: i.vision.clone(),
                    horizon: i.horizon.clone(),
                    state: RoadState::parse(&i.state),
                    shipped_at: i.shipped_at.clone(),
                    shipped_by: i.shipped_by.clone(),
                    shelved: i.shelved,
                    depth: i.depth,
                    depends_on: i.depends_on.iter().map(map_edge).collect(),
                    unlocks: i.unlocks.iter().map(map_edge).collect(),
                    features: i.features.iter().map(map_road_feature).collect(),
                })
                .collect(),
            loose_features: road.loose_features.iter().map(map_road_feature).collect(),
        });
        cur.roadmap_raw = Some(road);
        true
    })
}

/// The roadmap as last read.
pub fn roadmap() -> Rc<Roadmap> {
    CURRENT.with(|c| c.borrow().roadmap.clone())
}

fn map_edge(e: &api::RoadmapEdgeDto) -> RoadEdge {
    RoadEdge {
        item_id: e.item_id.clone(),
        name: e.name.clone(),
        hard: e.hard,
        shipped: e.shipped,
    }
}

fn map_road_feature(f: &api::RoadmapFeatureDto) -> RoadFeature {
    RoadFeature {
        id: f.id.clone(),
        name: f.name.clone(),
        status: f.status.clone(),
        released: f.released,
        modules_done: f.modules_done.max(0) as usize,
        modules_total: f.modules_total.max(0) as usize,
    }
}

/// Store one module's drawer contents. Returns true when they changed.
pub fn apply_module(detail: api::ModuleDetail) -> bool {
    CURRENT.with(|c| {
        let mut cur = c.borrow_mut();
        if cur.modules.get(&detail.id).is_some_and(|(raw, _)| *raw == detail) {
            return false;
        }
        let mapped = ModuleDetail {
            handoff: detail.handoff.as_ref().map(map_document),
            history: detail
                .history
                .iter()
                .map(|e| ModuleEvent {
                    title: e.title.clone(),
                    body: e.body.clone(),
                    at: e.time.clone(),
                    from: if e.kind == "premature_claim" {
                        "server.gate".to_string()
                    } else {
                        e.agent.clone().unwrap_or_default()
                    },
                })
                .collect(),
        };
        cur.modules.insert(detail.id.clone(), (detail, Rc::new(mapped)));
        true
    })
}

/// Merge one page of a feature's ledger into its feed. Returns true
/// when the feed changed.
///
/// A page is either the newest page (a refresh, or the first read) or
/// an older one (the reader paged back). Both are merged on `seq`, so
/// a refresh that overlaps what is held adds only the new rows. A
/// newest page that does NOT overlap — more events landed since the
/// last read than fit in a page — replaces the feed rather than being
/// stitched onto it with a gap the reader cannot see.
pub fn apply_events(page: api::EventPage) -> bool {
    CURRENT.with(|c| {
        let mut cur = c.borrow_mut();
        let incoming: Vec<EventItem> = page.events.iter().map(map_event).collect();
        let held = cur.feeds.get(&page.feature_id);
        let newest_page = incoming.first().map(|e| e.seq)
            >= held.and_then(|f| f.events.first().map(|e| e.seq));
        let overlaps = held.is_some_and(|f| {
            incoming.iter().any(|e| f.events.iter().any(|h| h.seq == e.seq))
        });
        let (mut events, exhausted) = match held {
            Some(f) if overlaps || !newest_page => {
                let mut merged: Vec<EventItem> = f
                    .events
                    .iter()
                    .filter(|h| !incoming.iter().any(|e| e.seq == h.seq))
                    .map(clone_event)
                    .collect();
                merged.extend(incoming);
                // Paging back reaches the end; a refresh says nothing
                // about it.
                (merged, if newest_page { f.exhausted } else { !page.more })
            }
            _ => (incoming, !page.more),
        };
        events.sort_by(|a, b| b.seq.cmp(&a.seq));
        let changed = match held {
            Some(f) => {
                f.exhausted != exhausted
                    || f.events.len() != events.len()
                    || f.events.iter().zip(&events).any(|(a, b)| a.seq != b.seq)
            }
            None => true,
        };
        if changed {
            cur.feeds.insert(page.feature_id, Rc::new(Feed { events, exhausted }));
        }
        changed
    })
}

/// Store one subject's discussion. Returns true when it changed.
pub fn apply_comments(page: api::CommentPage) -> bool {
    CURRENT.with(|c| {
        let mut cur = c.borrow_mut();
        if cur.threads.get(&page.subject_id).is_some_and(|(raw, _)| *raw == page) {
            return false;
        }
        let mapped = Rc::new(page.comments.iter().map(map_comment).collect());
        cur.threads.insert(page.subject_id.clone(), (page, mapped));
        true
    })
}

/// Store the page of the pool the wants screen asked for. Returns true
/// when it changed.
pub fn set_want_page(page: api::WantPage) -> bool {
    CURRENT.with(|c| {
        let mut cur = c.borrow_mut();
        if cur.want_page.as_ref() == Some(&page) {
            return false;
        }
        cur.wants = Rc::new(page.wants.iter().map(map_want).collect());
        cur.want_total = page.total.max(0) as usize;
        cur.want_page = Some(page);
        true
    })
}

/// Store the idea whose drawer is open. Returns true when it changed.
pub fn set_open_want(want: api::WantDto) -> bool {
    CURRENT.with(|c| {
        let mut cur = c.borrow_mut();
        if cur.open_want.as_ref().is_some_and(|(raw, _)| *raw == want) {
            return false;
        }
        let mapped = map_want(&want);
        cur.open_want = Some((want, mapped));
        true
    })
}

/// Recompose the feature list from the board and the detail cache.
fn rebuild(cur: &mut Current) {
    let Some(board) = cur.board.as_ref() else { return };
    let features: Vec<Rc<Feature>> = board
        .features
        .iter()
        .map(|r| {
            let detail = cur.details.get(&r.id);
            let status = match r.status.as_str() {
                "done" => Status::Done,
                "in_progress" => Status::Running,
                "shelved" => Status::Queued,
                _ => Status::Planning,
            };
            let elapsed = match (r.started.as_str(), r.last_activity.as_str()) {
                ("", _) => "\u{2014}".to_string(),
                (a, b) if a == b || b.is_empty() => a.to_string(),
                (a, b) => format!("{a} \u{2192} {b}"),
            };
            Feature {
                id: r.id.clone(),
                name: r.name.clone(),
                description: detail.map(|d| d.description.clone()).unwrap_or_default(),
                status,
                shelved: r.status == "shelved",
                agent: r.created_by.clone().unwrap_or_else(|| "\u{2014}".into()),
                elapsed,
                modules_done: r.modules_done.max(0) as usize,
                modules_total: r.modules_total.max(0) as usize,
                modules_ready: r.modules_ready.max(0) as usize,
                tasks_done: r.tasks_done.max(0) as usize,
                tasks_total: r.tasks_total.max(0) as usize,
                tasks_added: r.tasks_added.max(0) as usize,
                detail_loaded: detail.is_some(),
                modules: detail.map(|d| d.modules.clone()).unwrap_or_default(),
                whitepaper: detail.and_then(|d| d.whitepaper.clone()),
                sources: detail.map(|d| d.sources.clone()).unwrap_or_default(),
                attachments: detail.map(|d| d.attachments.clone()).unwrap_or_default(),
                open_questions: detail.map(|d| d.open_questions.clone()).unwrap_or_default(),
                last_word: r.last_word.as_ref().map(map_word),
                modules_running: r.modules_running.max(0) as usize,
                last_heard: r.last_heard,
                roadmap_item: r.roadmap_item.as_ref().map(map_edge),
                released: r.released.clone(),
                held_by: r.held_by.iter().map(map_edge).collect(),
            }
        })
        .map(Rc::new)
        .collect();
    let index_of = |id: &str| features.iter().position(|f| f.id == id);
    let mut attention: Vec<Attention> = board
        .attention
        .iter()
        .filter_map(|a| {
            let fi = index_of(&a.feature_id)?;
            let target = AttentionTarget::Module(fi, a.module_id.clone());
            Some(match a.kind.as_str() {
                "rejected" => Attention {
                    kind: "gate",
                    status: Status::Violation,
                    title: format!(
                        "Claim on {} denied \u{2014} {}",
                        a.module_name,
                        if a.waiting_on.is_empty() {
                            "its prerequisites are done now".to_string()
                        } else {
                            format!("waiting on {}", a.waiting_on.join(", "))
                        }
                    ),
                    place: a.feature_name.clone(),
                    target,
                },
                _ => Attention {
                    kind: "blocked",
                    status: Status::Blocked,
                    title: format!("{} is blocked, waiting on the manager", a.module_name),
                    place: a.feature_name.clone(),
                    target,
                },
            })
        })
        .collect();
    // Every open question, oldest first, above the loose-ideas nudge:
    // somebody is waiting on each of these.
    for q in cur.questions.iter() {
        let owed = if q.assigned_to.is_empty() { "anyone".to_string() } else { q.assigned_to.clone() };
        attention.push(Attention {
            kind: "question",
            status: Status::Blocked,
            title: format!("{} asked on {}: {}", q.author, q.subject_name, q.body),
            place: format!("owed by {owed}"),
            target: AttentionTarget::Question(q.level.clone(), q.subject_id.clone(), q.feature_id.clone()),
        });
    }
    let loose = cur.want_counts.0;
    if loose > 0 {
        attention.push(Attention {
            kind: "triage",
            status: Status::Planning,
            title: format!("{loose} loose ideas have never been composed or declined"),
            place: "Want pool".to_string(),
            target: AttentionTarget::Pool,
        });
    }
    cur.features = Rc::new(features);
    cur.attention = Rc::new(attention);
}

// ---------------------------------------------------------------------
// Wire → view mapping
// ---------------------------------------------------------------------

fn module_status(s: &str) -> Status {
    match s {
        "done" => Status::Done,
        "in_progress" => Status::Running,
        "blocked" => Status::Blocked,
        _ => Status::Queued,
    }
}

fn event_display(kind: &str) -> (&'static str, Status) {
    match kind {
        "feature_planned" | "plan_revised" => ("plan", Status::Planning),
        "module_claimed" => ("claim", Status::Running),
        "premature_claim" => ("gate", Status::Violation),
        "task_done" => ("task", Status::Done),
        "task_skipped" => ("task", Status::Queued),
        "task_added" => ("ad hoc", Status::Planning),
        "announcement" => ("says", Status::Running),
        "module_done" => ("module", Status::Done),
        // `stage_unlocked` is history: the ledger still carries rows
        // from before modules had prerequisites of their own.
        "stage_unlocked" | "module_unlocked" => ("gate", Status::Done),
        "document_written" => ("doc", Status::Planning),
        "blocker_reported" => ("blocker", Status::Violation),
        "module_released" => ("module", Status::Queued),
        "feature_done" => ("feature", Status::Done),
        "feature_deleted" | "feature_shelved" => ("feature", Status::Queued),
        "feature_unshelved" => ("feature", Status::Planning),
        "want_deleted" => ("want", Status::Queued),
        "memory_committed" => ("memory", Status::Running),
        "wants_promoted" => ("wants", Status::Planning),
        "want_added" | "want_updated" | "want_reopened" => ("want", Status::Planning),
        "want_declined" => ("want", Status::Queued),
        "tag_created" => ("tag", Status::Planning),
        "key_issued" => ("key", Status::Planning),
        "key_revoked" => ("key", Status::Queued),
        _ => ("event", Status::Queued),
    }
}

fn map_event(e: &api::EventDto) -> EventItem {
    let (kind, status) = event_display(&e.kind);
    EventItem {
        seq: e.seq,
        time: e.time.clone(),
        kind: kind.to_string(),
        status,
        title: e.title.clone(),
        body: e.body.clone(),
        agent: e.agent.clone().unwrap_or_default(),
        tool: e.kind.clone(),
    }
}

fn clone_event(e: &EventItem) -> EventItem {
    EventItem {
        seq: e.seq,
        time: e.time.clone(),
        kind: e.kind.clone(),
        status: e.status,
        title: e.title.clone(),
        body: e.body.clone(),
        agent: e.agent.clone(),
        tool: e.tool.clone(),
    }
}

fn map_document(d: &api::DocumentDto) -> Document {
    Document {
        revision: d.revision,
        title: d.title.clone(),
        body: d.body.clone(),
        author: d.author.clone(),
        written: d.written.clone(),
    }
}

fn map_module(m: &api::ModuleDto) -> Module {
    let status = if m.rejected {
        Status::Violation
    } else {
        module_status(&m.status)
    };
    let block = (!m.block_title.is_empty())
        .then(|| (m.block_title.clone(), m.block_body.clone()));
    // The agent slot doubles as the readiness word while nobody holds
    // the module: what a reader wants from an unclaimed card is whether
    // it could be claimed, not that it has not been.
    let agent = match m.claimed_by.clone() {
        Some(a) => a,
        None if m.status == "done" => "\u{2014}".to_string(),
        None if m.dispatchable => "ready".to_string(),
        None if !m.waiting_on.is_empty() => "waiting".to_string(),
        None => "not spawned".to_string(),
    };
    Module {
        id: m.id.clone(),
        name: m.name.clone(),
        description: m.description.clone(),
        status,
        agent,
        spawned: m.spawned.clone(),
        depends_on: m.depends_on.clone(),
        waiting_on: m.waiting_on.clone(),
        owns: m.owns.clone(),
        depth: m.depth.max(1) as usize,
        summary: m.summary.clone().filter(|s| !s.trim().is_empty()),
        block,
        open_questions: m.open_questions.iter().map(map_question).collect(),
        last_word: m.last_word.as_ref().map(map_word),
        last_heard: m.last_heard,
        tasks: m
            .tasks
            .iter()
            .map(|t| Task {
                id: t.id.clone(),
                label: t.name.clone(),
                done: t.status == "done" || t.status == "skipped",
                added: t.origin == "discovered",
            })
            .collect(),
    }
}

fn map_want(w: &api::WantDto) -> Want {
    Want {
        id: w.id.clone(),
        body: w.body.clone(),
        tags: w.tags.clone(),
        state: match w.status.as_str() {
            "promoted" => WantState::Promoted,
            "declined" => WantState::Declined,
            _ => WantState::Open,
        },
        note: w.note.clone(),
        author: w.author.clone(),
        captured: w.captured.clone(),
        features: w
            .features
            .iter()
            .map(|l| (l.feature_name.clone(), l.rationale.clone()))
            .collect(),
        attachments: w.attachments.iter().map(map_attachment).collect(),
    }
}

fn map_question(q: &api::QuestionDto) -> Question {
    Question {
        id: q.id.clone(),
        level: q.level.clone(),
        subject_id: q.subject_id.clone(),
        subject_name: q.subject_name.clone(),
        feature_id: q.feature_id.clone(),
        body: q.body.clone(),
        author: q.author.clone(),
        assigned_to: q.assigned_to.clone(),
        asked: q.asked.clone(),
    }
}

fn map_comment(c: &api::CommentDto) -> Comment {
    Comment {
        id: c.id.clone(),
        kind: c.kind.clone(),
        body: c.body.clone(),
        author: c.author.clone(),
        assigned_to: c.assigned_to.clone(),
        answers: c.answers.clone(),
        resolved: c.resolved,
        resolved_by: c.resolved_by.clone(),
        posted: c.posted.clone(),
        edited: c.edited,
        attachments: c.attachments.iter().map(map_attachment).collect(),
    }
}

fn map_attachment(a: &api::AttachmentDto) -> Attachment {
    Attachment {
        id: a.id.clone(),
        name: a.name.clone(),
        description: a.description.clone(),
        content_type: a.content_type.clone(),
        size: a.size.clone(),
        added_by: a.added_by.clone(),
        added: a.added.clone(),
        via_want_id: a.via_want_id.clone(),
        via_want: a.via_want.clone(),
        via_module_id: a.via_module_id.clone(),
        via_module: a.via_module.clone(),
        comment_id: a.comment_id.clone(),
        comment_author: a.comment_author.clone(),
        comment_excerpt: a.comment_excerpt.clone(),
    }
}

fn map_agent(a: &api::AgentDto) -> AgentRow {
    AgentRow {
        id: a.name.clone(),
        level: a.role.clone(),
        state: if a.active_claims > 0 {
            Status::Running
        } else {
            Status::Done
        },
        uptime: format!("{} claim(s)", a.active_claims),
        scope: if a.claim_names.is_empty() {
            "No live claims.".to_string()
        } else {
            format!("Holds: {}", a.claim_names)
        },
        health: a.health.as_ref().map(map_health),
        last_seen: a.last_seen.clone(),
        last_heard: a.last_heard,
        last_word: a.last_word.as_ref().map(map_word),
    }
}

fn map_word(w: &api::AnnouncementDto) -> Word {
    Word {
        text: w.text.clone(),
        by: w.by.clone(),
        at: w.at.clone(),
        subject: w.subject.clone(),
    }
}

fn map_health(h: &api::AgentHealthDto) -> AgentHealthRow {
    // Neutral for the two "nothing there" answers, danger for the one
    // that means a registered box is not serving, warning for the one
    // that is about the path to it rather than the box.
    let (tone, label) = match h.state.as_str() {
        "up" => (Status::Done, "up"),
        "down" => (Status::Violation, "down"),
        "gone" => (Status::Queued, "gone"),
        "unreachable" => (Status::Planning, "unreachable"),
        _ => (Status::Queued, "checking"),
    };
    let mut line = if h.since.is_empty() {
        format!("{label} \u{b7} not probed yet")
    } else {
        format!("{label} since {}", h.since)
    };
    if !h.detail.is_empty() && h.state != "up" {
        line.push_str(" \u{b7} ");
        line.push_str(&h.detail);
    }
    AgentHealthRow { tone, label: label.to_string(), line, up: h.state == "up" }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn page(feature: &str, seqs: &[i64], more: bool) -> api::EventPage {
        api::EventPage {
            feature_id: feature.to_string(),
            events: seqs
                .iter()
                .map(|&seq| api::EventDto { seq, kind: "task_done".into(), ..Default::default() })
                .collect(),
            more,
        }
    }

    fn seqs(feature: &str) -> Vec<i64> {
        feed(feature).map(|f| f.events.iter().map(|e| e.seq).collect()).unwrap_or_default()
    }

    // The feed is assembled from pages that arrive in either direction:
    // the newest page again on every tick, an older page when the
    // reader pages back. Both merge on seq; neither duplicates a row.
    #[test]
    fn pages_merge_on_seq_in_both_directions() {
        let f = "feat_merge";
        assert!(apply_events(page(f, &[30, 29, 28], true)));
        assert_eq!(seqs(f), vec![30, 29, 28]);
        assert!(!feed(f).unwrap().exhausted);

        // A refresh that overlaps adds only what is new.
        assert!(apply_events(page(f, &[32, 31, 30], true)));
        assert_eq!(seqs(f), vec![32, 31, 30, 29, 28]);
        // And the same refresh again changes nothing.
        assert!(!apply_events(page(f, &[32, 31, 30], true)));

        // Paging back appends, and reaching the end says so.
        assert!(apply_events(page(f, &[27, 26], false)));
        assert_eq!(seqs(f), vec![32, 31, 30, 29, 28, 27, 26]);
        assert!(feed(f).unwrap().exhausted);

        // A later refresh does not un-exhaust a fully read feed.
        assert!(apply_events(page(f, &[33, 32, 31], true)));
        assert!(feed(f).unwrap().exhausted);
    }

    // More landed since the last read than fit in a page: the newest
    // page shares nothing with what is held. Stitching them together
    // would hide a gap, so the page replaces the feed.
    #[test]
    fn a_newest_page_that_does_not_overlap_replaces_the_feed() {
        let f = "feat_gap";
        assert!(apply_events(page(f, &[10, 9, 8], false)));
        assert!(feed(f).unwrap().exhausted);
        assert!(apply_events(page(f, &[90, 89, 88], true)));
        assert_eq!(seqs(f), vec![90, 89, 88]);
        assert!(!feed(f).unwrap().exhausted, "the gap means there is older history to page to");
    }

    // A feature's row is complete before its graph has been read: the
    // counts come from the board, and only the module list waits.
    #[test]
    fn a_feature_row_stands_before_its_detail() {
        let mut board = api::Board::default();
        board.features.push(api::FeatureRollupDto {
            id: "feat_row".into(),
            name: "Row".into(),
            status: "in_progress".into(),
            modules_done: 1,
            modules_total: 3,
            modules_ready: 1,
            tasks_done: 2,
            tasks_total: 8,
            ..Default::default()
        });
        assert!(apply_board(board));
        let f = features();
        let row = f.iter().find(|f| f.id == "feat_row").expect("the board's row");
        assert!(!row.detail_loaded);
        assert_eq!(row.module_count(), (1, 3));
        assert_eq!(row.pct_label(), "25%");

        assert!(apply_feature(api::FeatureDetail {
            id: "feat_row".into(),
            modules: vec![api::ModuleDto { id: "mod_a".into(), ..Default::default() }],
            ..Default::default()
        }));
        let f = features();
        let row = f.iter().find(|f| f.id == "feat_row").unwrap();
        assert!(row.detail_loaded);
        assert_eq!(row.modules.len(), 1);
        // The counts still come from the board, not the module list.
        assert_eq!(row.module_count(), (1, 3));
    }
}
