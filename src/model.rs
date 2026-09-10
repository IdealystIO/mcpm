//! Live view-model for the MCP Project Console.
//!
//! Nothing here is hardcoded: [`apply_snapshot`] maps the `api` crate's
//! wire snapshot (loaded from the mcpm-web server, which reads the same
//! Postgres store the MCP tools write) into presentation-ready structs,
//! and [`features`]/[`wants`]/[`agents`]/[`project_name`] hand the
//! current data to the views. The console re-renders when [`crate::state::Console::rev`]
//! bumps after a changed snapshot.

use std::cell::RefCell;
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
    /// todo + unclaimed + every prerequisite done.
    pub dispatchable: bool,
    pub handoff: Option<Document>,
    pub summary: Option<String>,
    pub block: Option<(String, String)>,
    pub tasks: Vec<Task>,
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

pub struct Feature {
    pub name: String,
    pub description: String,
    pub status: Status,
    pub agent: String,
    pub elapsed: String,
    /// Topological order: every module after all of its prerequisites,
    /// ties by depth then name.
    pub modules: Vec<Module>,
    pub whitepaper: Option<Document>,
    pub events: Vec<EventItem>,
    /// The loose ideas this feature was composed from.
    pub sources: Vec<WantSource>,
}

// ---------------------------------------------------------------------
// Derived rollups
// ---------------------------------------------------------------------

pub fn task_fraction<'a>(modules: impl Iterator<Item = &'a Module>) -> f32 {
    let (mut done, mut total) = (0usize, 0usize);
    for m in modules {
        for t in &m.tasks {
            total += 1;
            if t.done {
                done += 1;
            }
        }
    }
    if total == 0 {
        0.0
    } else {
        done as f32 / total as f32
    }
}

impl Feature {
    pub fn fraction(&self) -> f32 {
        task_fraction(self.modules.iter())
    }

    pub fn pct_label(&self) -> String {
        format!("{}%", (self.fraction() * 100.0).round() as i32)
    }

    pub fn module_count(&self) -> (usize, usize) {
        (
            self.modules.iter().filter(|m| m.status == Status::Done).count(),
            self.modules.len(),
        )
    }

    pub fn task_count(&self) -> (usize, usize, usize) {
        let (mut done, mut total, mut added) = (0, 0, 0);
        for m in &self.modules {
            for t in &m.tasks {
                total += 1;
                if t.done {
                    done += 1;
                }
                if t.added {
                    added += 1;
                }
            }
        }
        (done, total, added)
    }

    /// Modules an agent could claim right now.
    pub fn ready_count(&self) -> usize {
        self.modules.iter().filter(|m| m.dispatchable).count()
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

struct Current {
    project_name: String,
    features: Rc<Vec<Feature>>,
    agents: Rc<Vec<AgentRow>>,
    wants: Rc<Vec<Want>>,
    tags: Rc<Vec<TagRow>>,
    last_snapshot: Option<api::Snapshot>,
    loaded: bool,
}

thread_local! {
    static CURRENT: RefCell<Current> = RefCell::new(Current {
        project_name: "control-center".into(),
        features: Rc::new(Vec::new()),
        agents: Rc::new(Vec::new()),
        wants: Rc::new(Vec::new()),
        tags: Rc::new(Vec::new()),
        last_snapshot: None,
        loaded: false,
    });
}

pub fn features() -> Rc<Vec<Feature>> {
    CURRENT.with(|c| c.borrow().features.clone())
}

pub fn agents() -> Rc<Vec<AgentRow>> {
    CURRENT.with(|c| c.borrow().agents.clone())
}

/// The pool filtered to what the toolbar asks for, as indices into
/// [`wants`] — indices, not clones, because the table's row components
/// re-read the model rather than carrying data through props (the `ui!`
/// for-each body is an `Fn` closure).
///
/// `query` matches the body case-insensitively, `status` is one of
/// "all" / "open" / "promoted" / "declined", and every tag in `tags`
/// must be present (AND, not OR — filters narrow).
pub fn filter_wants(query: &str, status: &str, tags: &[String]) -> Vec<usize> {
    let needle = query.trim().to_lowercase();
    wants()
        .iter()
        .enumerate()
        .filter(|(_, w)| {
            let state_ok = match status {
                "open" => w.state == WantState::Open,
                "promoted" => w.state == WantState::Promoted,
                "declined" => w.state == WantState::Declined,
                _ => true,
            };
            let text_ok = needle.is_empty() || w.body.to_lowercase().contains(&needle);
            let tags_ok = tags.iter().all(|t| w.tags.iter().any(|own| own == t));
            state_ok && text_ok && tags_ok
        })
        .map(|(i, _)| i)
        .collect()
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
pub enum AttentionTarget {
    /// A module drawer: `(feature, module)`.
    Module(usize, usize),
    /// The want pool screen.
    Pool,
}

/// One row on the overview's attention list.
///
/// Every entry is a **current condition** — a claim still bouncing off
/// a gate, a module still waiting on its manager, ideas still
/// uncomposed — never the memory of one that has since cleared
/// (UX_GUIDELINES rule 21). That is why each is derived from the
/// module's own status rather than from the newest event of some kind:
/// the row disappears by itself the moment the work moves.
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
pub fn attention() -> Vec<Attention> {
    let feats = features();
    let mut rejected = Vec::new();
    let mut blocked = Vec::new();
    for (fi, f) in feats.iter().enumerate() {
        for (mi, m) in f.modules.iter().enumerate() {
            let row = |kind, status, title| Attention {
                kind,
                status,
                title,
                place: f.name.clone(),
                target: AttentionTarget::Module(fi, mi),
            };
            match m.status {
                Status::Violation => {
                    let on = if m.waiting_on.is_empty() {
                        "its prerequisites are done now".to_string()
                    } else {
                        format!("waiting on {}", f.module_names(&m.waiting_on))
                    };
                    rejected.push(row(
                        "gate",
                        Status::Violation,
                        format!("Claim on {} denied \u{2014} {on}", m.name),
                    ))
                }
                Status::Blocked => blocked.push(row(
                    "blocked",
                    Status::Blocked,
                    format!("{} is blocked, waiting on the manager", m.name),
                )),
                _ => {}
            }
        }
    }
    rejected.append(&mut blocked);
    let loose = want_counts().0;
    if loose > 0 {
        rejected.push(Attention {
            kind: "triage",
            status: Status::Planning,
            title: format!("{loose} loose ideas have never been composed or declined"),
            place: "Want pool".to_string(),
            target: AttentionTarget::Pool,
        });
    }
    rejected
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

/// The project-wide ledger: the newest events from every feature at
/// once, as `(feature index, event index)` pairs.
///
/// Merged on `seq` and not on the displayed time, because the time is a
/// clock reading — two features' feeds interleaved on it come out in
/// the wrong order the moment the project has run past midnight.
pub fn recent_events(limit: usize) -> Vec<(usize, usize)> {
    let feats = features();
    let mut all: Vec<(usize, usize, i64)> = feats
        .iter()
        .enumerate()
        .flat_map(|(fi, f)| f.events.iter().enumerate().map(move |(ei, e)| (fi, ei, e.seq)))
        .collect();
    all.sort_by(|a, b| b.2.cmp(&a.2));
    all.truncate(limit);
    all.into_iter().map(|(fi, ei, _)| (fi, ei)).collect()
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

/// Index of a want in the pool by id, or `None` if it is no longer
/// there. The want drawer addresses its target by id — the pool
/// re-sorts under a poll, so a held index would drift onto another
/// idea mid-read.
pub fn want_index(id: &str) -> Option<usize> {
    wants().iter().position(|w| w.id == id)
}

/// The whole want pool, newest first.
pub fn wants() -> Rc<Vec<Want>> {
    CURRENT.with(|c| c.borrow().wants.clone())
}

/// The tag registry, most-used first.
pub fn tags() -> Rc<Vec<TagRow>> {
    CURRENT.with(|c| c.borrow().tags.clone())
}

/// Just the slugs — what `#tag` completion and highlighting match on.
pub fn tag_names() -> Vec<String> {
    tags().iter().map(|t| t.name.clone()).collect()
}

/// Pool counts as `(loose, composed, declined)`.
pub fn want_counts() -> (usize, usize, usize) {
    let wants = wants();
    let count = |state: WantState| wants.iter().filter(|w| w.state == state).count();
    (
        count(WantState::Open),
        count(WantState::Promoted),
        count(WantState::Declined),
    )
}

pub fn project_name() -> String {
    CURRENT.with(|c| c.borrow().project_name.clone())
}

/// Whether at least one snapshot has arrived (distinguishes "connecting"
/// from "the project genuinely has no features yet").
pub fn loaded() -> bool {
    CURRENT.with(|c| c.borrow().loaded)
}

/// Map + store a wire snapshot. Returns true when the data changed
/// (callers bump `Console::rev` to re-render).
pub fn apply_snapshot(snap: api::Snapshot) -> bool {
    let changed = CURRENT.with(|c| {
        let mut cur = c.borrow_mut();
        let changed = cur.last_snapshot.as_ref() != Some(&snap) || !cur.loaded;
        if changed {
            cur.project_name = snap.project.name.clone();
            cur.features = Rc::new(snap.features.iter().map(map_feature).collect());
            cur.agents = Rc::new(snap.agents.iter().map(map_agent).collect());
            cur.wants = Rc::new(snap.wants.iter().map(map_want).collect());
            cur.tags = Rc::new(
                snap.tags
                    .iter()
                    .map(|t| TagRow {
                        name: t.name.clone(),
                        label: t.label.clone(),
                        uses: t.uses,
                    })
                    .collect(),
            );
            cur.last_snapshot = Some(snap);
            cur.loaded = true;
        }
        changed
    });
    changed
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

fn map_feature(f: &api::FeatureDto) -> Feature {
    let status = match f.status.as_str() {
        "done" => Status::Done,
        "in_progress" => Status::Running,
        "shelved" => Status::Queued,
        _ => Status::Planning,
    };
    let elapsed = match (f.events.first(), f.events.last()) {
        (Some(first), Some(last)) if first.seq != last.seq => {
            format!("{} → {}", first.time, last.time)
        }
        (Some(first), _) => first.time.clone(),
        _ => "—".to_string(),
    };
    Feature {
        name: f.name.clone(),
        description: f.description.clone(),
        status,
        agent: f.created_by.clone().unwrap_or_else(|| "—".into()),
        elapsed,
        modules: f.modules.iter().map(|m| map_module(m, f)).collect(),
        whitepaper: f.whitepaper.as_ref().map(map_document),
        // Newest first, like the design's feed.
        events: f
            .events
            .iter()
            .rev()
            .map(|e| {
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
            })
            .collect(),
        sources: f
            .sources
            .iter()
            .map(|s| WantSource { id: s.want_id.clone(), body: s.body.clone() })
            .collect(),
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

fn map_module(m: &api::ModuleDto, f: &api::FeatureDto) -> Module {
    // Events touching this module (or its tasks — matched via payload
    // module linkage is server-side; here subject match suffices).
    let module_events: Vec<&api::EventDto> = f
        .events
        .iter()
        .filter(|e| e.subject_id.as_deref() == Some(m.id.as_str()))
        .collect();
    let rejected = m.status != "done"
        && m.claimed_by.is_none()
        && module_events.iter().any(|e| e.kind == "premature_claim");
    let status = if rejected {
        Status::Violation
    } else {
        module_status(&m.status)
    };
    let block = if rejected {
        module_events
            .iter()
            .rev()
            .find(|e| e.kind == "premature_claim")
            .map(|e| ("Start rejected \u{2014} prerequisites open".to_string(), e.body.clone()))
    } else if m.status == "blocked" {
        module_events
            .iter()
            .rev()
            .find(|e| e.kind == "blocker_reported")
            .map(|e| ("Blocked \u{2014} waiting on the manager".to_string(), e.body.clone()))
    } else {
        None
    };
    let spawned = module_events
        .iter()
        .find(|e| e.kind == "module_claimed")
        .map(|e| e.time.clone())
        .unwrap_or_default();
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
        spawned,
        depends_on: m.depends_on.clone(),
        waiting_on: m.waiting_on.clone(),
        owns: m.owns.clone(),
        depth: m.depth.max(1) as usize,
        dispatchable: m.dispatchable,
        handoff: m.handoff.as_ref().map(map_document),
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
        history: module_events
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
