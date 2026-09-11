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

pub struct Task {
    pub label: String,
    pub done: bool,
    pub added: bool,
}

/// One ledger entry that touched a module, as the drawer's history
/// shows it.
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
    pub tasks: Vec<Task>,
}

/// What a module's drawer adds to its card. Read with [`module_detail`];
/// `None` until [`apply_module`] has landed it.
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

pub struct AgentRow {
    pub id: String,
    pub level: String,
    pub state: Status,
    pub uptime: String,
    pub scope: String,
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
pub struct WantSource {
    pub id: String,
    pub body: String,
}

/// One feature: its board row always, its graph once visited.
///
/// The counts come from the board's rollup and not from `modules`, so
/// a feature row is right before its detail has loaded and stays
/// consistent with the rail after.
pub struct Feature {
    pub id: String,
    pub name: String,
    pub description: String,
    pub status: Status,
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
    pub modules: Rc<Vec<Module>>,
    pub whitepaper: Option<Rc<Document>>,
    /// The loose ideas this feature was composed from.
    pub sources: Rc<Vec<WantSource>>,
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
}

pub fn active_agent_count() -> usize {
    agents().iter().filter(|a| a.state == Status::Running).count()
}

// ---------------------------------------------------------------------
// The live cache
// ---------------------------------------------------------------------

/// One feature's detail as mapped from the wire, shared by reference
/// into every [`Feature`] rebuilt from the board.
struct Detail {
    raw: api::FeatureDetail,
    description: String,
    modules: Rc<Vec<Module>>,
    whitepaper: Option<Rc<Document>>,
    sources: Rc<Vec<WantSource>>,
}

struct Current {
    board: Option<api::Board>,
    project_name: String,
    features: Rc<Vec<Feature>>,
    agents: Rc<Vec<AgentRow>>,
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
    loaded: bool,
}

thread_local! {
    static CURRENT: RefCell<Current> = RefCell::new(Current {
        board: None,
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
        loaded: false,
    });
}

pub fn features() -> Rc<Vec<Feature>> {
    CURRENT.with(|c| c.borrow().features.clone())
}

pub fn agents() -> Rc<Vec<AgentRow>> {
    CURRENT.with(|c| c.borrow().agents.clone())
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

/// A feature's ledger as far as it has been read.
pub fn feed(feature_id: &str) -> Option<Rc<Feed>> {
    CURRENT.with(|c| c.borrow().feeds.get(feature_id).cloned())
}

/// The features the sidebar rail shows: everything still in play, plus
/// whichever one is selected.
///
/// A finished feature is the bulk of a long-lived project and the part
/// nobody is steering, so the rail — which exists to be scanned while
/// work is happening — drops it. `selected` is the exception: opening a
/// completed feature from the all-features screen must not make its own
/// card vanish out from under the reader.
pub fn rail_features(selected: usize) -> Vec<usize> {
    features()
        .iter()
        .enumerate()
        .filter(|(i, f)| f.status != Status::Done || *i == selected)
        .map(|(i, _)| i)
        .collect()
}

/// Where an [`Attention`] row goes when it is opened.
#[derive(Clone)]
pub enum AttentionTarget {
    /// A module drawer: `(feature index, module id)`. The module is
    /// addressed by id because its feature's graph may not be loaded
    /// yet when the row is pressed.
    Module(usize, String),
    /// The want pool screen.
    Pool,
}

/// One row on the overview's attention list.
///
/// Every entry is a **current condition** — a claim still bouncing off
/// a gate, a module still waiting on its manager, ideas still
/// uncomposed — never the memory of one that has since cleared
/// (UX_GUIDELINES rule 21). The server derives each from the module's
/// own state rather than from the newest event of some kind, so the
/// row disappears by itself the moment the work moves.
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

/// Everything across the project that has stopped and is waiting on a
/// person, worst first: rejected claims, then blocked modules, then the
/// uncomposed pool.
///
/// Ordered by how much it costs to leave alone. A rejected claim is an
/// agent that tried and was refused — it is not coming back on its own.
/// A blocked module has already escalated. Loose wants are only ever
/// the tail: nothing is stalled on them.
pub fn attention() -> Rc<Vec<Attention>> {
    CURRENT.with(|c| c.borrow().attention.clone())
}

/// The features still being worked, as indices into [`features`].
pub fn in_play() -> Vec<usize> {
    features()
        .iter()
        .enumerate()
        .filter(|(_, f)| f.status != Status::Done)
        .map(|(i, _)| i)
        .collect()
}

/// The project-wide ledger's newest entries, newest first.
pub fn recent() -> Rc<Vec<EventItem>> {
    CURRENT.with(|c| c.borrow().recent.clone())
}

/// The first feature still in play, for the console to land on.
///
/// Without this the console opens on index 0, which on a mature project
/// is the oldest feature and almost certainly a finished one — the
/// reader's first sight of the board would be work nobody is doing.
pub fn first_open_feature() -> Option<usize> {
    features().iter().position(|f| f.status != Status::Done)
}

/// How many features the rail is leaving out, for the all-features
/// entry to name.
pub fn completed_count() -> usize {
    features().iter().filter(|f| f.status == Status::Done).count()
}

/// The all-features screen's rows, as indices into [`features`].
///
/// `query` matches the feature name case-insensitively; `status` is
/// "all" / "open" / "done". Open is defined as "not complete" rather
/// than as a list of the other states, so a feature can never be
/// unreachable under every filter the way an enumerated list lets
/// happen when a new state is added.
pub fn filter_features(query: &str, status: &str) -> Vec<usize> {
    let needle = query.trim().to_lowercase();
    features()
        .iter()
        .enumerate()
        .filter(|(_, f)| {
            let state_ok = match status {
                "open" => f.status != Status::Done,
                "done" => f.status == Status::Done,
                _ => true,
            };
            let text_ok = needle.is_empty() || f.name.to_lowercase().contains(&needle);
            state_ok && text_ok
        })
        .map(|(i, _)| i)
        .collect()
}

/// The page of the pool the wants screen is showing.
pub fn wants() -> Rc<Vec<Want>> {
    CURRENT.with(|c| c.borrow().wants.clone())
}

/// How many wants the pool's current filters match in total, across
/// every page.
pub fn want_total() -> usize {
    CURRENT.with(|c| c.borrow().want_total)
}

/// One idea by id: from the page on screen, or the one whose drawer is
/// open — which a feature's origin list can open onto an idea no
/// loaded page holds.
pub fn want_by_id(id: &str) -> Option<Want> {
    CURRENT.with(|c| {
        let cur = c.borrow();
        cur.wants
            .iter()
            .find(|w| w.id == id)
            .cloned()
            .or_else(|| cur.open_want.as_ref().filter(|(_, w)| w.id == id).map(|(_, w)| w.clone()))
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

/// Pool counts as `(loose, composed, declined)`, project-wide.
pub fn want_counts() -> (usize, usize, usize) {
    CURRENT.with(|c| c.borrow().want_counts)
}

pub fn project_name() -> String {
    CURRENT.with(|c| c.borrow().project_name.clone())
}

/// Whether the board has arrived at least once (distinguishes
/// "connecting" from "the project genuinely has no features yet").
pub fn loaded() -> bool {
    CURRENT.with(|c| c.borrow().loaded)
}

// ---------------------------------------------------------------------
// Applying reads
// ---------------------------------------------------------------------

/// Store the board. Returns true when the data changed (callers bump
/// `Console::rev` to re-render).
pub fn apply_board(board: api::Board) -> bool {
    CURRENT.with(|c| {
        let mut cur = c.borrow_mut();
        let changed = cur.board.as_ref() != Some(&board) || !cur.loaded;
        if changed {
            cur.project_name = board.project.name.clone();
            cur.agents = Rc::new(board.agents.iter().map(map_agent).collect());
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
            modules: Rc::new(detail.modules.iter().map(|m| map_module(m)).collect()),
            whitepaper: detail.whitepaper.as_ref().map(|d| Rc::new(map_document(d))),
            sources: Rc::new(
                detail
                    .sources
                    .iter()
                    .map(|s| WantSource { id: s.want_id.clone(), body: s.body.clone() })
                    .collect(),
            ),
            raw: detail,
        };
        cur.details.insert(mapped.raw.id.clone(), mapped);
        rebuild(&mut cur);
        true
    })
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
    let features: Vec<Feature> = board
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
            }
        })
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
        "task_added" => ("task", Status::Planning),
        "module_done" => ("module", Status::Done),
        // `stage_unlocked` is history: the ledger still carries rows
        // from before modules had prerequisites of their own.
        "stage_unlocked" | "module_unlocked" => ("gate", Status::Done),
        "document_written" => ("doc", Status::Planning),
        "blocker_reported" => ("blocker", Status::Violation),
        "module_released" => ("module", Status::Queued),
        "feature_done" => ("feature", Status::Done),
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
        tasks: m
            .tasks
            .iter()
            .map(|t| Task {
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
    }
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
