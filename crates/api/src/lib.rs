//! Wire types + server functions for the Control Center console.
//!
//! The console (wasm) calls [`load_snapshot`] and renders whatever the
//! store holds — nothing is hardcoded client-side. The server body
//! (feature `server`) reads through `mcpm_core::Store`, the same
//! gatekeeper the MCP tools use, and pre-formats event titles/bodies so
//! the UI stays a dumb renderer.

pub mod capture;

pub use capture::{parse_buffer, parse_line, scan_tags, tag_fragment_at, utf16_to_byte, TagSpan,
    WantDraftDto};

use serde::{Deserialize, Serialize};
use server::{server, subscription, ServerError};

// ---------------------------------------------------------------------
// Wire types
// ---------------------------------------------------------------------

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct Snapshot {
    pub project: ProjectDto,
    pub features: Vec<FeatureDto>,
    pub agents: Vec<AgentDto>,
    /// The whole want pool — loose ideas, in every state.
    pub wants: Vec<WantDto>,
    /// The tag registry, most-used first.
    pub tags: Vec<TagDto>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct ProjectDto {
    pub name: String,
    pub description: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct FeatureDto {
    pub id: String,
    pub name: String,
    /// planning | in_progress | done | shelved
    pub status: String,
    pub created_by: Option<String>,
    pub stages: Vec<StageDto>,
    /// Ascending seq. Pre-formatted for display.
    pub events: Vec<EventDto>,
    /// The wants this feature was composed from, oldest link first.
    pub sources: Vec<WantSourceDto>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct StageDto {
    pub id: String,
    pub name: String,
    pub position: i32,
    /// Derived server-side: locked | unlocked | done
    pub status: String,
    pub modules: Vec<ModuleDto>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ModuleDto {
    pub id: String,
    pub name: String,
    pub description: String,
    /// todo | in_progress | blocked | done
    pub status: String,
    pub claimed_by: Option<String>,
    pub summary: Option<String>,
    pub tasks: Vec<TaskDto>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct TaskDto {
    pub id: String,
    pub name: String,
    /// open | done | skipped
    pub status: String,
    /// planned | discovered
    pub origin: String,
    pub note: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct EventDto {
    pub seq: i64,
    /// "HH:MM"
    pub time: String,
    /// The raw event type (module_claimed, premature_claim, …).
    pub kind: String,
    pub agent: Option<String>,
    pub subject_id: Option<String>,
    pub title: String,
    pub body: String,
}

/// One idea in the pool.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
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
}

/// One committed event, announced. The console does not read anything
/// out of this beyond "something landed, and here is how far the ledger
/// has got" — it refetches the snapshot it already knows how to apply.
///
/// Carrying `seq` rather than an empty ping is what makes the client
/// idempotent: two notifications for the same seq (a reconnect
/// replaying, say) collapse into one refetch.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq)]
pub struct Tick {
    /// The `events.seq` of the row that fired the trigger.
    pub seq: i64,
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
    let seqs = store.watch_events().await;
    async_stream_compat(seqs).map(|seq| Tick { seq }).right_stream()
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
    seqs: std::result::Result<impl futures_core::Stream<Item = i64>, mcpm_core::McpmError>,
) -> impl futures_core::Stream<Item = i64> {
    use futures_util::StreamExt as _;
    match seqs {
        Ok(stream) => stream.left_stream(),
        Err(err) => {
            eprintln!("mcpm-web: event listener unavailable, console will poll: {err}");
            futures_util::stream::empty().right_stream()
        }
    }
}

/// The console's one read: the whole board, presentation-ready.
#[server]
pub async fn load_snapshot() -> Result<Snapshot, ServerError> {
    let store = server::use_state::<mcpm_core::Store>()
        .ok_or_else(|| ServerError::failed("Store not installed"))?;
    let raw = store
        .snapshot()
        .await
        .map_err(|e| ServerError::failed(e.to_string()))?;
    let project = ProjectDto {
        name: raw["project"]["name"].as_str().unwrap_or("control-center").to_string(),
        description: raw["project"]["description"].as_str().unwrap_or("").to_string(),
    };

    let mut features = Vec::new();
    for rollup in store.rollups().await.map_err(fail)? {
        let tree = store.feature_tree(&rollup.id).await.map_err(fail)?;
        let events = store
            .get_events(Some(&rollup.id), 0, 500)
            .await
            .map_err(fail)?;
        features.push(FeatureDto {
            id: tree.id,
            name: tree.name,
            status: tree.status,
            created_by: feature_creator(&store, &rollup.id).await,
            stages: tree
                .stages
                .into_iter()
                .map(|s| StageDto {
                    id: s.id,
                    name: s.name,
                    position: s.position,
                    status: s.status,
                    modules: s
                        .modules
                        .into_iter()
                        .map(|m| ModuleDto {
                            id: m.id,
                            name: m.name,
                            description: m.description,
                            status: m.status,
                            claimed_by: m.claimed_by,
                            summary: m.summary,
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
                        })
                        .collect(),
                })
                .collect(),
            events: events.iter().map(format_event).collect(),
            sources: store
                .wants_of_feature(&rollup.id)
                .await
                .map_err(fail)?
                .into_iter()
                .map(|w| WantSourceDto {
                    rationale: w
                        .features
                        .iter()
                        .find(|l| l.feature_id == rollup.id)
                        .map(|l| l.rationale.clone())
                        .unwrap_or_default(),
                    want_id: w.id,
                    body: w.body,
                })
                .collect(),
        });
    }

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
        })
        .collect();

    let wants = store
        .list_wants("", mcpm_core::WantFilter::All, &[], 500)
        .await
        .map_err(fail)?
        .wants
        .into_iter()
        .map(|w| WantDto {
            id: w.id,
            body: w.body,
            tags: w.tags,
            status: w.status,
            note: w.decline_reason.unwrap_or_default(),
            author: w.author,
            captured: w.created_at.format("%b %-d %H:%M").to_string(),
            features: w
                .features
                .into_iter()
                .map(|l| WantLinkDto {
                    feature_id: l.feature_id,
                    feature_name: l.feature_name,
                    rationale: l.rationale,
                })
                .collect(),
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

    Ok(Snapshot {
        project,
        features,
        agents,
        wants,
        tags,
    })
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

#[cfg(feature = "server")]
async fn feature_creator(store: &mcpm_core::Store, feature_id: &str) -> Option<String> {
    // The feature_planned event's agent is the planning manager.
    store
        .get_events(Some(feature_id), 0, 5)
        .await
        .ok()?
        .into_iter()
        .find(|e| e.kind == "feature_planned")
        .and_then(|e| e.agent)
}

/// Pre-format one ledger entry for display.
#[cfg(feature = "server")]
fn format_event(e: &mcpm_core::Event) -> EventDto {
    let p = &e.payload;
    let s = |key: &str| p.get(key).and_then(|v| v.as_str()).unwrap_or("").to_string();
    let (title, body) = match e.kind.as_str() {
        "feature_planned" => (
            format!(
                "Feature plan published: {} stages, {} modules",
                p["stages"].as_u64().unwrap_or(0),
                p["modules"].as_u64().unwrap_or(0)
            ),
            String::new(),
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
        "module_claimed" => (
            format!("Module claimed: {}", s("module")),
            format!("Stage '{}'.", s("stage")),
        ),
        "premature_claim" => (
            format!("Start denied for {}", s("module")),
            // Name the stage that is actually in the way. How the gate
            // works, and who this event is for, are not the reader's to
            // hold — they can see the stage order on the board.
            p["blocking"]
                .as_array()
                .and_then(|b| b.first())
                .and_then(|st| st.get("name"))
                .and_then(|n| n.as_str())
                .map(|name| format!("Stage '{name}' is still open."))
                .unwrap_or_else(|| "An earlier stage is still open.".to_string()),
        ),
        "task_done" => (format!("Task checked off: {}", s("task")), s("note")),
        "task_skipped" => (format!("Task skipped: {}", s("task")), s("note")),
        "task_added" => (
            format!("Worker added a task: {}", s("task")),
            String::new(),
        ),
        "module_done" => (
            format!("Module complete: {}", s("module")),
            format!("Stage '{}'.", s("stage")),
        ),
        "stage_unlocked" => (
            format!("Stage unlocked: {}", s("stage")),
            format!("Opened by {}.", s("unlocked_by")),
        ),
        "blocker_reported" => (format!("Blocker on {}", s("module")), s("description")),
        "module_released" => (
            format!("Module released: {}", s("module")),
            format!("Reason: {}.", s("reason")),
        ),
        "feature_done" => (
            "Feature closed".to_string(),
            format!("'{}' completed all stages.", s("name")),
        ),
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
        other => (other.to_string(), String::new()),
    };
    EventDto {
        seq: e.seq,
        time: e.ts.format("%H:%M").to_string(),
        kind: e.kind.clone(),
        agent: e.agent.clone(),
        subject_id: e.subject_id.clone(),
        title,
        body,
    }
}
