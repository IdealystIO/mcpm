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

/// Execution status shared by features, stages, modules, events, and
/// agents. `Violation` is a display state: a module whose claim bounced
/// off the stage gate (`premature_claim` on the ledger).
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
            Status::Blocked => "gated",
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

pub struct TraceCall {
    pub at: String,
    pub tool: String,
    pub result: String,
}

pub struct Handoff {
    pub title: String,
    pub body: String,
    pub at: String,
    pub from: String,
}

pub struct Module {
    pub name: String,
    pub status: Status,
    pub agent: String,
    pub spawned: String,
    pub in_stage: String,
    pub now: Option<(String, String)>,
    pub block: Option<(String, String)>,
    pub tasks: Vec<Task>,
    pub handoffs: Vec<Handoff>,
    pub trace: Vec<TraceCall>,
}

pub struct Stage {
    pub name: String,
    pub status: Status,
    pub time: String,
    pub modules: Vec<Module>,
}

pub struct EventItem {
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
    pub status: Status,
    pub agent: String,
    pub elapsed: String,
    pub stages: Vec<Stage>,
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
        task_fraction(self.stages.iter().flat_map(|s| s.modules.iter()))
    }

    pub fn pct_label(&self) -> String {
        format!("{}%", (self.fraction() * 100.0).round() as i32)
    }

    pub fn module_count(&self) -> (usize, usize) {
        let all: Vec<_> = self.stages.iter().flat_map(|s| s.modules.iter()).collect();
        (
            all.iter().filter(|m| m.status == Status::Done).count(),
            all.len(),
        )
    }

    pub fn task_count(&self) -> (usize, usize, usize) {
        let (mut done, mut total, mut added) = (0, 0, 0);
        for m in self.stages.iter().flat_map(|s| s.modules.iter()) {
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

    pub fn stage_count(&self) -> (usize, usize) {
        (
            self.stages
                .iter()
                .filter(|s| s.status == Status::Done)
                .count(),
            self.stages.len(),
        )
    }
}

impl Stage {
    pub fn fraction(&self) -> f32 {
        task_fraction(self.modules.iter())
    }

    pub fn done_label(&self) -> String {
        format!(
            "{}/{} modules",
            self.modules
                .iter()
                .filter(|m| m.status == Status::Done)
                .count(),
            self.modules.len()
        )
    }

    pub fn concurrency_note(&self) -> String {
        if self.modules.len() > 1 {
            format!("{} modules run concurrently", self.modules.len())
        } else {
            "single module".to_string()
        }
    }
}

impl Module {
    pub fn fraction(&self) -> f32 {
        task_fraction(std::iter::once(self))
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
        "stage_unlocked" => ("gate", Status::Done),
        "blocker_reported" => ("blocker", Status::Violation),
        "module_released" => ("module", Status::Queued),
        "feature_done" => ("feature", Status::Done),
        "memory_committed" => ("memory", Status::Running),
        "wants_promoted" => ("wants", Status::Planning),
        "want_added" | "want_updated" | "want_reopened" => ("want", Status::Planning),
        "want_declined" => ("want", Status::Queued),
        "tag_created" => ("tag", Status::Planning),
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
        status,
        agent: f.created_by.clone().unwrap_or_else(|| "—".into()),
        elapsed,
        stages: f.stages.iter().map(|s| map_stage(s, f)).collect(),
        // Newest first, like the design's feed.
        events: f
            .events
            .iter()
            .rev()
            .map(|e| {
                let (kind, status) = event_display(&e.kind);
                EventItem {
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

fn map_stage(s: &api::StageDto, f: &api::FeatureDto) -> Stage {
    let any_active = s
        .modules
        .iter()
        .any(|m| m.status == "in_progress" || m.status == "blocked");
    let status = match s.status.as_str() {
        "done" => Status::Done,
        "locked" => Status::Blocked,
        _ if any_active => Status::Running,
        _ => Status::Running,
    };
    let time = match s.status.as_str() {
        "done" => "closed".to_string(),
        "locked" => "not started".to_string(),
        _ => "open".to_string(),
    };
    Stage {
        name: s.name.clone(),
        status,
        time,
        modules: s.modules.iter().map(|m| map_module(m, f)).collect(),
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
            .map(|e| ("Start rejected — stage order violated".to_string(), e.body.clone()))
    } else if m.status == "blocked" {
        module_events
            .iter()
            .rev()
            .find(|e| e.kind == "blocker_reported")
            .map(|e| ("Blocked — waiting on the manager".to_string(), e.body.clone()))
    } else {
        None
    };
    let spawned = module_events
        .iter()
        .find(|e| e.kind == "module_claimed")
        .map(|e| e.time.clone())
        .unwrap_or_default();
    Module {
        name: m.name.clone(),
        status,
        agent: m
            .claimed_by
            .clone()
            .unwrap_or_else(|| "not spawned".to_string()),
        spawned,
        in_stage: "—".to_string(),
        now: None,
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
        handoffs: module_events
            .iter()
            .map(|e| Handoff {
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
        trace: module_events
            .iter()
            .map(|e| TraceCall {
                at: e.time.clone(),
                tool: e.kind.clone(),
                result: if e.kind == "premature_claim" {
                    "denied".to_string()
                } else {
                    "ok".to_string()
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
