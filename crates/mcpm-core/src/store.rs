//! The Postgres store. Every architecture-doc invariant is enforced
//! here, inside a transaction, in exactly one place each:
//!
//! - **The stage gate** is checked only in [`Store::claim_module`], in
//!   the same transaction that takes the claim — and a rejected claim
//!   still COMMITS its `premature_claim` event, so the manager learns
//!   about the mistake even if the worker never reports back.
//! - **Completion is proven, not asserted**: `complete_module` refuses
//!   while tasks are open; `complete_feature` refuses while stages are
//!   unfinished; finishing a stage's last module emits `stage_unlocked`.
//! - **Everything leaves a trace**: every mutation appends an event and
//!   every completion/blocker commits a memory.
//! - **A delegated identity is confined to one module**, and that is
//!   checked in [`require_claim`] — the same choke point the claim
//!   itself goes through, so a write cannot land inside scope by
//!   skipping the check rather than passing it.

use serde_json::json;
use std::sync::Arc;

use sqlx::postgres::PgPoolOptions;
use tokio::sync::{broadcast, OnceCell};
use sqlx::{PgPool, Postgres, Row, Transaction};

use crate::error::{McpmError, ErrorCode};
use crate::ids::{id_level, new_id, new_memory_id, new_want_id, normalize_tag, Level};
use crate::keys::{
    Actor, ApiKeyInfo, Delegation, IssuedKey, KeyIdentity, KeyRole, MintRequest, MintedWorker,
};
use crate::types::*;

/// The Postgres NOTIFY channel the `events_notify` trigger announces
/// on. Named once here; the trigger names it in
/// `0005_notify_channel_rename.sql` (0004 installed it under the old
/// name and is frozen — sqlx checksums applied migrations).
const EVENT_CHANNEL: &str = "mcpm_events";

/// How long a minted worker identity lives when the manager does not
/// say. Long enough for a real module, short enough that a token
/// forgotten in a prompt is not a standing grant.
const DEFAULT_DELEGATION_TTL_MINUTES: i64 = 240;
/// The floor and ceiling on a caller-chosen TTL. The ceiling matters
/// more: a manager that asks for a week has misunderstood what the
/// token is for, and clamping is friendlier than refusing mid-dispatch.
const MIN_DELEGATION_TTL_MINUTES: i64 = 5;
const MAX_DELEGATION_TTL_MINUTES: i64 = 1440;
/// How many live worker keys `issue_worker_key` will let exist. A key
/// is a STANDING credential — it outlives the box it was issued to
/// unless somebody revokes it — so the failure this bounds is a
/// dispatch loop quietly accumulating credentials, which raises nothing
/// at the time and is only visible in `--list-keys` weeks later. The
/// operator's `--issue-key` is not capped: a human at a shell is the
/// thing that fixes a deployment that has hit this.
const MAX_LIVE_WORKER_KEYS: i64 = 64;

/// What every [`Briefing`] tells a worker about its checklist.
///
/// Measured over 37 modules and 268 ticks, the median tick landed 81%
/// of the way through its module and only 1.5% arrived in the first
/// 40%. That is structural rather than sloppy: `complete_module`
/// refuses with `TASKS_OPEN`, so ticking reads as an exit requirement
/// and the only gate sits at the end. Telling a worker to be tidier
/// does not move a gate. Durability does — these workers run on
/// reclaimable instances, where a mid-module reclaim loses the whole
/// checklist and the resuming agent cannot tell "not started" from
/// "done but unrecorded".
const CLAIM_GUIDANCE: &str = "Tick each task as you finish it. A task ticked when it \
     is done survives an interruption; one ticked at the end only survives if you \
     get there. On a spot instance the replacement sees your checklist, not your \
     intentions.";

type Result<T> = std::result::Result<T, McpmError>;
type Tx<'a> = Transaction<'a, Postgres>;

#[derive(Clone)]
pub struct Store {
    pool: PgPool,
    /// Kept so the event listener can open its OWN connection. It must
    /// not come from `pool`: a listener holds its connection for the
    /// life of the subscription, so borrowing one would permanently
    /// spend a query slot per subscriber and eventually starve the
    /// pool of the connections ordinary reads need.
    url: String,
    /// The process-wide fan-out of `EVENT_CHANNEL`, started on first
    /// use. One Postgres connection serves every subscriber, however
    /// many consoles are open.
    events: Arc<OnceCell<broadcast::Sender<i64>>>,
}

impl Store {
    /// Connect and run migrations.
    pub async fn connect(database_url: &str) -> Result<Store> {
        let pool = PgPoolOptions::new()
            .max_connections(4)
            .connect(database_url)
            .await
            // `Configuration` is how sqlx reports a URL it could not
            // parse, and its own text names whatever token it choked
            // on — classically "invalid port number" for a password
            // beginning with ':', which points at nothing the operator
            // typed. Name the real cause: percent-encoding the
            // userinfo is the fix in nearly every case, and inspecting
            // the URL to find that out is what gets a credential
            // pasted into a terminal.
            .map_err(|err| match err {
                sqlx::Error::Configuration(inner) => McpmError::internal(format!(
                    "could not parse DATABASE_URL ({inner}). If the password contains any of \
                     : / ? # [ ] @ it must be percent-encoded — an unencoded ':' is read as \
                     the start of a port and an unencoded '?' as the start of a query \
                     string. Parsed as: {}",
                    crate::redact_url(database_url)
                )),
                other => McpmError::from(other),
            })?;
        sqlx::migrate!("./migrations")
            .run(&pool)
            .await
            .map_err(McpmError::internal)?;
        Ok(Store {
            pool,
            url: database_url.to_string(),
            events: Arc::new(OnceCell::new()),
        })
    }

    pub fn pool(&self) -> &PgPool {
        &self.pool
    }

    /// Idempotently create the singleton project row.
    pub async fn ensure_project(&self, name: &str, repo_path: &str, description: &str) -> Result<()> {
        sqlx::query(
            "INSERT INTO project (id, name, repo_path, description) VALUES (1, $1, $2, $3)
             ON CONFLICT (id) DO NOTHING",
        )
        .bind(name)
        .bind(repo_path)
        .bind(description)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    // -----------------------------------------------------------------
    // Orientation
    // -----------------------------------------------------------------

    /// Register/refresh the agent and return the full orientation.
    /// A stream of committed event sequence numbers, one per row the
    /// `events_notify` trigger announces.
    ///
    /// This is the ONLY cross-process liveness signal that works here:
    /// the MCP server and the console's web host are separate
    /// processes against one database, so an in-process broadcast
    /// would carry the console's own writes and silently miss every
    /// agent's. Postgres LISTEN/NOTIFY crosses that boundary, and
    /// fires on commit, so nothing is announced that did not land.
    ///
    /// Every caller shares ONE listener on its own connection. Giving
    /// each subscriber its own would spend a Postgres connection per
    /// open console for as long as it stayed open, which starves the
    /// query pool and takes ordinary reads down with it.
    ///
    /// A slow subscriber that falls behind skips to the newest seq
    /// rather than closing: the tick is a nudge to refetch, so missing
    /// intermediate numbers costs nothing.
    pub async fn watch_events(&self) -> Result<impl futures_core::Stream<Item = i64>> {
        let tx = self
            .events
            .get_or_try_init(|| async {
                let mut listener = sqlx::postgres::PgListener::connect(&self.url).await?;
                listener.listen(EVENT_CHANNEL).await?;
                let (tx, _rx) = broadcast::channel(256);
                let feed = tx.clone();
                tokio::spawn(async move {
                    let mut stream = listener.into_stream();
                    while let Some(Ok(note)) = futures_util::StreamExt::next(&mut stream).await {
                        if let Ok(seq) = note.payload().parse::<i64>() {
                            // Err just means nobody is listening yet.
                            let _ = feed.send(seq);
                        }
                    }
                });
                Ok::<_, McpmError>(tx)
            })
            .await?;
        let mut rx = tx.subscribe();
        Ok(async_stream::stream! {
            loop {
                match rx.recv().await {
                    Ok(seq) => yield seq,
                    Err(broadcast::error::RecvError::Lagged(_)) => continue,
                    Err(broadcast::error::RecvError::Closed) => break,
                }
            }
        })
    }

    pub async fn get_context(&self, agent: &str, role: &str) -> Result<Context> {
        sqlx::query(
            "INSERT INTO agents (name, role) VALUES ($1, $2)
             ON CONFLICT (name) DO UPDATE SET role = $2, last_seen = now()",
        )
        .bind(agent)
        .bind(role)
        .execute(&self.pool)
        .await?;

        let project = sqlx::query("SELECT name, repo_path, description FROM project WHERE id = 1")
            .fetch_optional(&self.pool)
            .await?
            .map(|r| ProjectInfo {
                name: r.get("name"),
                repo_path: r.get("repo_path"),
                description: r.get("description"),
            })
            .unwrap_or(ProjectInfo {
                name: "(uninitialized project)".into(),
                repo_path: String::new(),
                description: String::new(),
            });

        let features = self.feature_rollups().await?;
        let your_claims = self.claims_of(agent).await?;
        let open_wants: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM wants w WHERE w.state = 'open'
               AND NOT EXISTS (SELECT 1 FROM want_features wf WHERE wf.want_id = w.id)",
        )
        .fetch_one(&self.pool)
        .await?;

        let suggested_next = if let Some(c) = your_claims.first() {
            format!(
                "You hold a claim on '{}' ({}) with {} open task(s) — resume it: work the \
                 checklist with complete_task, then complete_module.",
                c.module_name, c.module_id, c.open_tasks
            )
        } else if role == "manager" {
            if features.is_empty() && open_wants > 0 {
                format!(
                    "No features yet, but {open_wants} want(s) are waiting. Read them with \
                     list_wants, find the themes, and compose a group into a feature with \
                     promote_wants."
                )
            } else if features.is_empty() {
                "No features planned yet. Create one with plan_feature (stages in order, \
                 modules per stage, tasks per module) — or capture loose ideas first with \
                 add_want and compose them later."
                    .to_string()
            } else {
                "Poll your feature with feature_status, then dispatch every module next_work \
                 returns."
                    .to_string()
            }
        } else {
            "You hold no claim. Ask your manager for a module id, then claim_module it."
                .to_string()
        };

        Ok(Context {
            project,
            you: AgentIdentity {
                name: agent.to_string(),
                role: role.to_string(),
            },
            features,
            your_claims,
            open_wants,
            suggested_next,
        })
    }

    /// Read-only project snapshot (the `project://status` resource and
    /// the dashboard's entry point): project info + per-feature rollups.
    pub async fn snapshot(&self) -> Result<serde_json::Value> {
        let project = sqlx::query("SELECT name, repo_path, description FROM project WHERE id = 1")
            .fetch_optional(&self.pool)
            .await?
            .map(|r| {
                json!({
                    "name": r.get::<String, _>("name"),
                    "repo_path": r.get::<String, _>("repo_path"),
                    "description": r.get::<String, _>("description"),
                })
            })
            .unwrap_or(serde_json::Value::Null);
        let features = self.feature_rollups().await?;
        Ok(json!({ "project": project, "features": features }))
    }

    /// Public rollups (used by `next_work` notes and the MCP prompts).
    pub async fn rollups(&self) -> Result<Vec<FeatureRollup>> {
        self.feature_rollups().await
    }

    /// Every registered agent with its live claim count (dashboard roster).
    pub async fn agents_overview(&self) -> Result<Vec<AgentOverview>> {
        let rows = sqlx::query(
            "SELECT a.name, a.role,
                    (SELECT COUNT(*) FROM modules m
                     WHERE m.claimed_by = a.name AND m.status IN ('in_progress','blocked'))
                        AS active_claims,
                    (SELECT COALESCE(string_agg(m.name, ', '), '') FROM modules m
                     WHERE m.claimed_by = a.name AND m.status IN ('in_progress','blocked'))
                        AS claim_names
             FROM agents a ORDER BY a.first_seen",
        )
        .fetch_all(&self.pool)
        .await?;
        Ok(rows
            .into_iter()
            .map(|r| AgentOverview {
                name: r.get("name"),
                role: r.get("role"),
                active_claims: r.get("active_claims"),
                claim_names: r.get("claim_names"),
            })
            .collect())
    }

    async fn feature_rollups(&self) -> Result<Vec<FeatureRollup>> {
        let rows = sqlx::query(
            "SELECT f.id, f.name, f.status,
               (SELECT COUNT(*) FROM stages s WHERE s.feature_id = f.id) AS stages_total,
               (SELECT COUNT(*) FROM stages s WHERE s.feature_id = f.id
                  AND EXISTS (SELECT 1 FROM modules m WHERE m.stage_id = s.id)
                  AND NOT EXISTS (SELECT 1 FROM modules m
                                  WHERE m.stage_id = s.id AND m.status <> 'done')) AS stages_done,
               (SELECT COUNT(*) FROM modules m JOIN stages s ON s.id = m.stage_id
                  WHERE s.feature_id = f.id) AS modules_total,
               (SELECT COUNT(*) FROM modules m JOIN stages s ON s.id = m.stage_id
                  WHERE s.feature_id = f.id AND m.status = 'done') AS modules_done,
               (SELECT COUNT(*) FROM tasks t JOIN modules m ON m.id = t.module_id
                  JOIN stages s ON s.id = m.stage_id WHERE s.feature_id = f.id) AS tasks_total,
               (SELECT COUNT(*) FROM tasks t JOIN modules m ON m.id = t.module_id
                  JOIN stages s ON s.id = m.stage_id
                  WHERE s.feature_id = f.id AND t.status IN ('done','skipped')) AS tasks_done
             FROM features f ORDER BY f.created_at",
        )
        .fetch_all(&self.pool)
        .await?;
        Ok(rows
            .into_iter()
            .map(|r| FeatureRollup {
                id: r.get("id"),
                name: r.get("name"),
                status: r.get("status"),
                stages_done: r.get("stages_done"),
                stages_total: r.get("stages_total"),
                modules_done: r.get("modules_done"),
                modules_total: r.get("modules_total"),
                tasks_done: r.get("tasks_done"),
                tasks_total: r.get("tasks_total"),
            })
            .collect())
    }

    async fn claims_of(&self, agent: &str) -> Result<Vec<ClaimRef>> {
        let rows = sqlx::query(
            "SELECT m.id AS module_id, m.name AS module_name,
                    f.id AS feature_id, f.name AS feature_name,
                    (SELECT COUNT(*) FROM tasks t
                     WHERE t.module_id = m.id AND t.status = 'open') AS open_tasks
             FROM modules m
             JOIN stages s ON s.id = m.stage_id
             JOIN features f ON f.id = s.feature_id
             WHERE m.claimed_by = $1 AND m.status IN ('in_progress', 'blocked')
             ORDER BY f.created_at, s.position",
        )
        .bind(agent)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows
            .into_iter()
            .map(|r| ClaimRef {
                module_id: r.get("module_id"),
                module_name: r.get("module_name"),
                feature_id: r.get("feature_id"),
                feature_name: r.get("feature_name"),
                open_tasks: r.get("open_tasks"),
            })
            .collect())
    }

    // -----------------------------------------------------------------
    // Planning
    // -----------------------------------------------------------------

    /// Create the feature and its entire tree in one transaction — a
    /// half-written plan is never visible to workers.
    pub async fn plan_feature(&self, agent: &str, plan: PlanFeature) -> Result<FeatureTree> {
        validate_plan(&plan)?;
        let mut tx = self.pool.begin().await?;
        let feature_id = insert_plan(&mut tx, agent, &plan).await?;
        tx.commit().await?;
        self.feature_tree(&feature_id).await
    }

    /// Batch plan surgery, applied atomically or not at all. Refuses
    /// ops that rewrite history (removing completed/claimed work).
    pub async fn revise_plan(&self, agent: &str, feature_id: &str, ops: Vec<PlanOp>) -> Result<FeatureTree> {
        if ops.is_empty() {
            return Err(plan_invalid("revise_plan needs at least one operation."));
        }
        let mut tx = self.pool.begin().await?;
        // Serialize revisions per feature.
        let found = sqlx::query("SELECT id FROM features WHERE id = $1 FOR UPDATE")
            .bind(feature_id)
            .fetch_optional(&mut *tx)
            .await?;
        if found.is_none() {
            return Err(McpmError::not_found("feature", feature_id));
        }
        let mut summaries: Vec<String> = Vec::new();
        for op in ops {
            let summary = apply_plan_op(&mut tx, agent, feature_id, op).await?;
            summaries.push(summary);
        }
        record_event(
            &mut tx,
            "plan_revised",
            Some(feature_id),
            Some(feature_id),
            Some(agent),
            json!({ "ops": summaries }),
        )
        .await?;
        tx.commit().await?;
        self.feature_tree(feature_id).await
    }

    /// Close the feature. Refuses while any stage is unfinished.
    pub async fn complete_feature(&self, agent: &str, feature_id: &str, summary: &str) -> Result<Ack> {
        let mut tx = self.pool.begin().await?;
        let row = sqlx::query("SELECT id, name, status FROM features WHERE id = $1 FOR UPDATE")
            .bind(feature_id)
            .fetch_optional(&mut *tx)
            .await?
            .ok_or_else(|| McpmError::not_found("feature", feature_id))?;
        let name: String = row.get("name");
        let status: String = row.get("status");
        if status == "done" {
            return Err(McpmError::new(
                ErrorCode::AlreadyDone,
                format!("Feature '{name}' is already complete."),
                json!({ "feature_id": feature_id }),
                "Nothing to do. Report completion to your operator.",
            ));
        }
        let remaining = sqlx::query(
            "SELECT s.id AS stage_id, s.name AS stage_name, s.position,
                    m.id AS module_id, m.name AS module_name, m.status
             FROM stages s JOIN modules m ON m.stage_id = s.id
             WHERE s.feature_id = $1 AND m.status <> 'done'
             ORDER BY s.position, m.name",
        )
        .bind(feature_id)
        .fetch_all(&mut *tx)
        .await?;
        if !remaining.is_empty() {
            let items: Vec<serde_json::Value> = remaining
                .iter()
                .map(|r| {
                    json!({
                        "stage": r.get::<String, _>("stage_name"),
                        "stage_position": r.get::<i32, _>("position"),
                        "module_id": r.get::<String, _>("module_id"),
                        "module": r.get::<String, _>("module_name"),
                        "status": r.get::<String, _>("status"),
                    })
                })
                .collect();
            return Err(McpmError::new(
                ErrorCode::StagesIncomplete,
                format!(
                    "Feature '{}' still has {} unfinished module(s).",
                    name,
                    items.len()
                ),
                json!({ "remaining": items }),
                "Dispatch the listed modules (next_work tells you which are ready now) and \
                 complete them before closing the feature.",
            ));
        }
        sqlx::query("UPDATE features SET status = 'done', summary = $2 WHERE id = $1")
            .bind(feature_id)
            .bind(summary)
            .execute(&mut *tx)
            .await?;
        insert_memory(
            &mut tx,
            Level::Feature,
            feature_id,
            MemoryKind::Outcome,
            summary,
            &["system", "summary"],
            agent,
        )
        .await?;
        record_event(
            &mut tx,
            "feature_done",
            Some(feature_id),
            Some(feature_id),
            Some(agent),
            json!({ "name": name }),
        )
        .await?;
        tx.commit().await?;
        Ok(Ack::new(format!(
            "Feature '{name}' is complete. The summary was committed as a feature-scope memory."
        )))
    }

    // -----------------------------------------------------------------
    // Dispatch
    // -----------------------------------------------------------------

    /// The dispatch decision, computed server-side: every module that is
    /// todo + unclaimed + in the lowest unfinished stage.
    pub async fn next_work(&self, feature_id: &str) -> Result<NextWork> {
        let tree = self.feature_tree(feature_id).await?;
        let mut dispatchable = Vec::new();
        let mut in_flight = Vec::new();
        let mut blocked = Vec::new();
        for stage in &tree.stages {
            for module in &stage.modules {
                if module.dispatchable {
                    let mem_count: i64 = sqlx::query(
                        "SELECT COUNT(*) AS n FROM memories
                         WHERE (level = 'feature' AND subject_id = $1)
                            OR (level = 'stage' AND subject_id = $2)",
                    )
                    .bind(feature_id)
                    .bind(&stage.id)
                    .fetch_one(&self.pool)
                    .await?
                    .get("n");
                    dispatchable.push(WorkItem {
                        module_id: module.id.clone(),
                        module_name: module.name.clone(),
                        description: module.description.clone(),
                        stage_id: stage.id.clone(),
                        stage_name: stage.name.clone(),
                        stage_position: stage.position,
                        tasks: module
                            .tasks
                            .iter()
                            .map(|t| TaskView {
                                id: t.id.clone(),
                                name: t.name.clone(),
                                status: t.status.clone(),
                                origin: t.origin.clone(),
                                note: t.note.clone(),
                            })
                            .collect(),
                        relevant_memories: mem_count,
                    });
                } else if module.status == "in_progress" {
                    in_flight.push(ClaimRef {
                        module_id: module.id.clone(),
                        module_name: module.name.clone(),
                        feature_id: tree.id.clone(),
                        feature_name: tree.name.clone(),
                        open_tasks: module.tasks.iter().filter(|t| t.status == "open").count()
                            as i64,
                    });
                } else if module.status == "blocked" {
                    blocked.push(format!(
                        "{} ({}) — blocked, claimed by {}",
                        module.name,
                        module.id,
                        module.claimed_by.as_deref().unwrap_or("nobody")
                    ));
                }
            }
        }
        let note = if !dispatchable.is_empty() {
            format!(
                "Dispatch one worker per module below ({} ready). They can run concurrently — \
                 they are all in the same unlocked stage.",
                dispatchable.len()
            )
        } else if tree.status == "done" {
            "The feature is complete — nothing to dispatch.".to_string()
        } else if !blocked.is_empty() {
            "Nothing is dispatchable: blocked modules need your attention (revise the plan, \
             clear the blocker, or release the module)."
                .to_string()
        } else if !in_flight.is_empty() {
            "Nothing new to dispatch: work is in flight. Wait for workers to finish, then \
             poll feature_status."
                .to_string()
        } else {
            "Nothing dispatchable and nothing in flight — every module is done. Close the \
             feature with complete_feature."
                .to_string()
        };
        Ok(NextWork {
            feature_id: feature_id.to_string(),
            dispatchable,
            in_flight,
            blocked,
            note,
        })
    }

    /// The manager's poll: full tree plus every event after the cursor.
    pub async fn feature_status(&self, feature_id: &str, events_since: i64) -> Result<FeatureStatus> {
        let feature = self.feature_tree(feature_id).await?;
        let events = self.get_events(Some(feature_id), events_since, 200).await?;
        let events_cursor = events.last().map(|e| e.seq).unwrap_or(events_since);
        Ok(FeatureStatus {
            feature,
            events,
            events_cursor,
        })
    }

    // -----------------------------------------------------------------
    // Execution
    // -----------------------------------------------------------------

    /// Take an exclusive claim. THE gate check lives here, inside the
    /// claiming transaction. A rejection still commits its
    /// `premature_claim` event.
    ///
    /// The result carries [`CLAIM_GUIDANCE`]: this is the one call a
    /// worker makes before it has any habits for the module, so it is
    /// where the habit is worth setting.
    pub async fn claim_module(
        &self,
        actor: impl Into<Actor>,
        module_id: &str,
    ) -> Result<Briefing> {
        let actor = actor.into();
        require_scope(&actor, module_id)?;
        let agent: &str = &actor.name;
        let mut tx = self.pool.begin().await?;
        let row = sqlx::query(
            "SELECT m.id, m.name, m.description, m.status, m.claimed_by, m.summary,
                    s.id AS stage_id, s.name AS stage_name, s.position,
                    f.id AS feature_id, f.name AS feature_name
             FROM modules m
             JOIN stages s ON s.id = m.stage_id
             JOIN features f ON f.id = s.feature_id
             WHERE m.id = $1
             FOR UPDATE OF m",
        )
        .bind(module_id)
        .fetch_optional(&mut *tx)
        .await?
        .ok_or_else(|| McpmError::not_found("module", module_id))?;

        let module_name: String = row.get("name");
        let status: String = row.get("status");
        let claimed_by: Option<String> = row.get("claimed_by");
        let stage_id: String = row.get("stage_id");
        let stage_name: String = row.get("stage_name");
        let position: i32 = row.get("position");
        let feature_id: String = row.get("feature_id");
        let feature_name: String = row.get("feature_name");

        // --- The gate -------------------------------------------------
        let blocking = sqlx::query(
            "SELECT s.id AS stage_id, s.name AS stage_name, s.position,
                    m.id AS module_id, m.name AS module_name, m.status, m.claimed_by
             FROM stages s JOIN modules m ON m.stage_id = s.id
             WHERE s.feature_id = $1 AND s.position < $2 AND m.status <> 'done'
             ORDER BY s.position, m.name",
        )
        .bind(&feature_id)
        .bind(position)
        .fetch_all(&mut *tx)
        .await?;
        if !blocking.is_empty() {
            let mut stages: Vec<serde_json::Value> = Vec::new();
            for r in &blocking {
                let sid: String = r.get("stage_id");
                let entry = json!({
                    "id": r.get::<String, _>("module_id"),
                    "name": r.get::<String, _>("module_name"),
                    "status": r.get::<String, _>("status"),
                    "claimed_by": r.get::<Option<String>, _>("claimed_by"),
                });
                match stages.iter_mut().find(|s| s["stage"] == json!(sid)) {
                    Some(stage) => stage["open_modules"]
                        .as_array_mut()
                        .expect("open_modules is an array")
                        .push(entry),
                    None => stages.push(json!({
                        "stage": sid,
                        "name": r.get::<String, _>("stage_name"),
                        "position": r.get::<i32, _>("position"),
                        "open_modules": [entry],
                    })),
                }
            }
            let first_blocking = stages[0]["name"].as_str().unwrap_or("?").to_string();
            record_event(
                &mut tx,
                "premature_claim",
                Some(&feature_id),
                Some(module_id),
                Some(agent),
                json!({
                    "module": module_name,
                    "stage_position": position,
                    "blocking": stages,
                }),
            )
            .await?;
            // Commit so the rejection itself is on the record.
            tx.commit().await?;
            return Err(McpmError::new(
                ErrorCode::StageLocked,
                format!(
                    "Module '{module_name}' ({module_id}) is in stage {position} of \
                     '{feature_name}', but stage '{first_blocking}' is incomplete."
                ),
                json!({ "blocking": stages }),
                "Do not begin work on this module. Report STAGE_LOCKED to your manager agent \
                 and end your turn. The manager should re-dispatch you after the earlier \
                 stages complete; this attempt has already been recorded as a \
                 premature_claim event.",
            ));
        }

        // --- Claim states --------------------------------------------
        match (status.as_str(), claimed_by.as_deref()) {
            ("done", _) => {
                return Err(McpmError::new(
                    ErrorCode::AlreadyDone,
                    format!("Module '{module_name}' is already complete."),
                    json!({ "module_id": module_id }),
                    "Do not redo completed work. Report back to your manager.",
                ));
            }
            (_, Some(holder)) if holder != agent => {
                return Err(McpmError::new(
                    ErrorCode::AlreadyClaimed,
                    format!("Module '{module_name}' is already claimed by '{holder}'."),
                    json!({ "module_id": module_id, "claimed_by": holder }),
                    "Do not duplicate work. Report back to your manager; if that worker is \
                     dead, the manager can have it release_module first.",
                ));
            }
            _ => {}
        }
        // todo → claim; own blocked/in_progress → resume (idempotent for
        // a restarted worker).
        sqlx::query("UPDATE modules SET claimed_by = $2, status = 'in_progress' WHERE id = $1")
            .bind(module_id)
            .bind(agent)
            .execute(&mut *tx)
            .await?;
        sqlx::query("UPDATE features SET status = 'in_progress' WHERE id = $1 AND status = 'planning'")
            .bind(&feature_id)
            .execute(&mut *tx)
            .await?;
        record_event(
            &mut tx,
            "module_claimed",
            Some(&feature_id),
            Some(module_id),
            Some(agent),
            json!({ "module": module_name, "stage": stage_name }),
        )
        .await?;
        tx.commit().await?;

        // --- The briefing --------------------------------------------
        let tasks = self.tasks_of(module_id).await?;
        // Everything filed above this module, project conventions
        // included — `up` terminates at the project shelf, so a worker
        // is briefed with the standing rules and not just the ones
        // somebody happened to pin to this feature.
        let ancestor_memories = self
            .search_memory(&MemoryQuery {
                scope: Some(MemoryScope {
                    level: Level::Module,
                    id: module_id.to_string(),
                }),
                direction: SearchDirection::Up,
                limit: 25,
                // Dead ends included. A worker that does not know X was
                // tried and refuted will propose X.
                include_refuted: true,
                ..Default::default()
            })
            .await?
            .hits
            .into_iter()
            .map(|h| h.memory)
            .collect();
        let upstream = sqlx::query(
            "SELECT m.id, m.name, s.position, m.summary
             FROM modules m JOIN stages s ON s.id = m.stage_id
             WHERE s.feature_id = $1 AND s.position < $2
               AND m.status = 'done' AND m.summary IS NOT NULL
             ORDER BY s.position, m.name",
        )
        .bind(&feature_id)
        .bind(position)
        .fetch_all(&self.pool)
        .await?;
        Ok(Briefing {
            module: ModuleView {
                id: module_id.to_string(),
                name: module_name,
                description: row.get("description"),
                status: "in_progress".into(),
                claimed_by: Some(agent.to_string()),
                summary: row.get("summary"),
                dispatchable: false,
                tasks,
            },
            stage_id,
            stage_name,
            stage_position: position,
            feature_id,
            feature_name,
            ancestor_memories,
            upstream_summaries: upstream
                .into_iter()
                .map(|r| UpstreamSummary {
                    module_id: r.get("id"),
                    module_name: r.get("name"),
                    stage_position: r.get("position"),
                    summary: r.get::<Option<String>, _>("summary").unwrap_or_default(),
                })
                .collect(),
            guidance: CLAIM_GUIDANCE.to_string(),
        })
    }

    /// Check off (or skip, with a reason) one task.
    pub async fn complete_task(
        &self,
        actor: impl Into<Actor>,
        task_id: &str,
        outcome: TaskOutcome,
        note: Option<&str>,
    ) -> Result<Ack> {
        let actor = actor.into();
        let agent: &str = &actor.name;
        if outcome == TaskOutcome::Skipped && note.map(str::trim).unwrap_or("").is_empty() {
            return Err(McpmError::new(
                ErrorCode::SkipNeedsReason,
                "Skipping a task requires a reason in `note`.",
                json!({ "task_id": task_id }),
                "Say why the task doesn't need doing — the reason becomes part of the record.",
            ));
        }
        let mut tx = self.pool.begin().await?;
        let row = sqlx::query(
            "SELECT t.id, t.name, t.status, m.id AS module_id, m.name AS module_name,
                    m.claimed_by, s.feature_id
             FROM tasks t JOIN modules m ON m.id = t.module_id
             JOIN stages s ON s.id = m.stage_id
             WHERE t.id = $1 FOR UPDATE OF t",
        )
        .bind(task_id)
        .fetch_optional(&mut *tx)
        .await?
        .ok_or_else(|| McpmError::not_found("task", task_id))?;
        require_claim(&row, &actor, "check off its tasks")?;
        let task_name: String = row.get("name");
        let module_id: String = row.get("module_id");
        let feature_id: String = row.get("feature_id");
        let new_status = match outcome {
            TaskOutcome::Done => "done",
            TaskOutcome::Skipped => "skipped",
        };
        sqlx::query("UPDATE tasks SET status = $2, note = COALESCE($3, note) WHERE id = $1")
            .bind(task_id)
            .bind(new_status)
            .bind(note)
            .execute(&mut *tx)
            .await?;
        record_event(
            &mut tx,
            if new_status == "done" { "task_done" } else { "task_skipped" },
            Some(&feature_id),
            Some(task_id),
            Some(agent),
            json!({ "task": task_name, "module_id": module_id, "note": note }),
        )
        .await?;
        let open: i64 = sqlx::query("SELECT COUNT(*) AS n FROM tasks WHERE module_id = $1 AND status = 'open'")
            .bind(&module_id)
            .fetch_one(&mut *tx)
            .await?
            .get("n");
        tx.commit().await?;
        Ok(Ack::with(
            format!("Task '{task_name}' {new_status}. {open} task(s) still open in this module."),
            json!({ "open_tasks": open, "module_id": module_id }),
        ))
    }

    /// A worker extends its own checklist (`origin: discovered`).
    pub async fn add_task(
        &self,
        actor: impl Into<Actor>,
        module_id: &str,
        name: &str,
        note: Option<&str>,
    ) -> Result<Ack> {
        let actor = actor.into();
        let agent: &str = &actor.name;
        let mut tx = self.pool.begin().await?;
        let row = sqlx::query(
            "SELECT m.id AS module_id, m.name AS module_name, m.status, m.claimed_by, s.feature_id
             FROM modules m JOIN stages s ON s.id = m.stage_id
             WHERE m.id = $1 FOR UPDATE OF m",
        )
        .bind(module_id)
        .fetch_optional(&mut *tx)
        .await?
        .ok_or_else(|| McpmError::not_found("module", module_id))?;
        let status: String = row.get("status");
        if status == "done" {
            return Err(McpmError::new(
                ErrorCode::AlreadyDone,
                "The module is already complete; its checklist is closed.",
                json!({ "module_id": module_id }),
                "If new work surfaced after completion, tell your manager — it belongs in a \
                 plan revision, not a closed checklist.",
            ));
        }
        require_claim(&row, &actor, "extend its checklist")?;
        let feature_id: String = row.get("feature_id");
        let task_id = new_id(Level::Task);
        sqlx::query(
            "INSERT INTO tasks (id, module_id, name, origin, note, position, created_by)
             VALUES ($1, $2, $3, 'discovered', $4,
                     (SELECT COALESCE(MAX(position), -1) + 1 FROM tasks WHERE module_id = $2), $5)",
        )
        .bind(&task_id)
        .bind(module_id)
        .bind(name)
        .bind(note)
        .bind(agent)
        .execute(&mut *tx)
        .await?;
        record_event(
            &mut tx,
            "task_added",
            Some(&feature_id),
            Some(&task_id),
            Some(agent),
            json!({ "task": name, "module_id": module_id, "origin": "discovered" }),
        )
        .await?;
        tx.commit().await?;
        Ok(Ack::with(
            format!("Task '{name}' added to the checklist as discovered work ({task_id})."),
            json!({ "task_id": task_id }),
        ))
    }

    /// The exit interview: refuses while tasks are open, commits the
    /// summary as a module-scope memory, and unlocks the next stage
    /// when this was the last module standing.
    /// Finish a module. `used` names the memories the work actually
    /// leaned on.
    ///
    /// The exit doors are the best place to attest: the agent has just
    /// finished and knows what helped, and — unlike at search time —
    /// the OUTCOME is known. A touch recorded here rode work that
    /// succeeded, which is the closest thing to accuracy evidence
    /// available without asking anyone to grade anything.
    pub async fn complete_module(
        &self,
        actor: impl Into<Actor>,
        module_id: &str,
        summary: &str,
        used: &[String],
    ) -> Result<Ack> {
        let actor = actor.into();
        let agent: &str = &actor.name;
        if summary.trim().is_empty() {
            return Err(plan_invalid(
                "complete_module requires a non-empty summary — it becomes the memory the \
                 next stage's workers read.",
            ));
        }
        let mut tx = self.pool.begin().await?;
        let row = sqlx::query(
            "SELECT m.id AS module_id, m.name AS module_name, m.status, m.claimed_by,
                    s.id AS stage_id, s.name AS stage_name, s.position, s.feature_id
             FROM modules m JOIN stages s ON s.id = m.stage_id
             WHERE m.id = $1 FOR UPDATE OF m",
        )
        .bind(module_id)
        .fetch_optional(&mut *tx)
        .await?
        .ok_or_else(|| McpmError::not_found("module", module_id))?;
        let status: String = row.get("status");
        if status == "done" {
            return Err(McpmError::new(
                ErrorCode::AlreadyDone,
                "The module is already complete.",
                json!({ "module_id": module_id }),
                "Nothing to do.",
            ));
        }
        require_claim(&row, &actor, "complete it")?;
        let module_name: String = row.get("module_name");
        let stage_id: String = row.get("stage_id");
        let stage_name: String = row.get("stage_name");
        let position: i32 = row.get("position");
        let feature_id: String = row.get("feature_id");

        let open = sqlx::query(
            "SELECT id, name FROM tasks WHERE module_id = $1 AND status = 'open' ORDER BY position",
        )
        .bind(module_id)
        .fetch_all(&mut *tx)
        .await?;
        if !open.is_empty() {
            let items: Vec<serde_json::Value> = open
                .iter()
                .map(|r| json!({ "id": r.get::<String, _>("id"), "name": r.get::<String, _>("name") }))
                .collect();
            return Err(McpmError::new(
                ErrorCode::TasksOpen,
                format!(
                    "Module '{}' still has {} open task(s).",
                    module_name,
                    items.len()
                ),
                json!({ "open_tasks": items }),
                "Finish each listed task (complete_task with outcome 'done'), or skip it with \
                 an explicit reason (outcome 'skipped' + note). Then complete_module again.",
            ));
        }

        sqlx::query("UPDATE modules SET status = 'done', summary = $2 WHERE id = $1")
            .bind(module_id)
            .bind(summary)
            .execute(&mut *tx)
            .await?;
        // A delegated identity outlives nothing. Retiring inside the
        // completing transaction is what makes "the token dies with the
        // module" true rather than nearly true: a token that survived a
        // rolled-back completion would be a standing grant nobody
        // issued.
        retire_delegations(&mut tx, module_id).await?;
        insert_memory(
            &mut tx,
            Level::Module,
            module_id,
            MemoryKind::Outcome,
            summary,
            &["system", "summary"],
            agent,
        )
        .await?;
        record_event(
            &mut tx,
            "module_done",
            Some(&feature_id),
            Some(module_id),
            Some(agent),
            json!({ "module": module_name, "stage": stage_name }),
        )
        .await?;
        // Attested use, inside the same transaction as the completion —
        // so a touch recorded here is one that rode work that finished.
        attest_used(&mut tx, agent, used, "carried this module").await?;

        // Stage completion → unlock the next stage.
        let stage_open: i64 = sqlx::query(
            "SELECT COUNT(*) AS n FROM modules WHERE stage_id = $1 AND status <> 'done'",
        )
        .bind(&stage_id)
        .fetch_one(&mut *tx)
        .await?
        .get("n");
        let mut unlocked: Option<String> = None;
        if stage_open == 0 {
            let next = sqlx::query(
                "SELECT id, name, position FROM stages
                 WHERE feature_id = $1 AND position > $2 ORDER BY position LIMIT 1",
            )
            .bind(&feature_id)
            .bind(position)
            .fetch_optional(&mut *tx)
            .await?;
            if let Some(next) = next {
                let next_name: String = next.get("name");
                record_event(
                    &mut tx,
                    "stage_unlocked",
                    Some(&feature_id),
                    Some(&next.get::<String, _>("id")),
                    Some(agent),
                    json!({
                        "stage": next_name,
                        "position": next.get::<i32, _>("position"),
                        "unlocked_by": format!("stage '{stage_name}' completing"),
                    }),
                )
                .await?;
                unlocked = Some(next_name);
            }
        }
        tx.commit().await?;
        let message = match &unlocked {
            Some(next) => format!(
                "Module '{module_name}' complete — that closed stage '{stage_name}' and \
                 unlocked stage '{next}'. Your summary is on the record for downstream workers."
            ),
            None if stage_open == 0 => format!(
                "Module '{module_name}' complete — that closed stage '{stage_name}', the last \
                 stage of the feature. The manager can now complete_feature."
            ),
            None => format!(
                "Module '{module_name}' complete. Stage '{stage_name}' has other modules still \
                 in flight."
            ),
        };
        Ok(Ack::with(message, json!({ "stage_unlocked": unlocked })))
    }

    /// Flag the module blocked; keeps the claim, records the blocker as
    /// a memory, raises `blocker_reported` for the manager.
    /// Report a blocker. `misled_by` names memories that turned out to
    /// be wrong or misleading — the disconfirming half of the same
    /// signal `complete_module` records.
    ///
    /// It records a touch rather than a dispute: "I relied on this and
    /// got stuck" is weaker than "I checked this and it is wrong", and
    /// conflating them would let every dead end withdraw a memory
    /// nobody had actually examined.
    pub async fn report_blocker(
        &self,
        actor: impl Into<Actor>,
        module_id: &str,
        description: &str,
        misled_by: &[String],
    ) -> Result<Ack> {
        let actor = actor.into();
        let agent: &str = &actor.name;
        let mut tx = self.pool.begin().await?;
        let row = sqlx::query(
            "SELECT m.id AS module_id, m.name AS module_name, m.status, m.claimed_by, s.feature_id
             FROM modules m JOIN stages s ON s.id = m.stage_id
             WHERE m.id = $1 FOR UPDATE OF m",
        )
        .bind(module_id)
        .fetch_optional(&mut *tx)
        .await?
        .ok_or_else(|| McpmError::not_found("module", module_id))?;
        require_claim(&row, &actor, "report a blocker on it")?;
        let module_name: String = row.get("module_name");
        let feature_id: String = row.get("feature_id");
        sqlx::query("UPDATE modules SET status = 'blocked' WHERE id = $1")
            .bind(module_id)
            .execute(&mut *tx)
            .await?;
        insert_memory(
            &mut tx,
            Level::Module,
            module_id,
            MemoryKind::Gotcha,
            description,
            &["system", "blocker"],
            agent,
        )
        .await?;
        attest_used(&mut tx, agent, misled_by, "relied on this and got stuck").await?;
        record_event(
            &mut tx,
            "blocker_reported",
            Some(&feature_id),
            Some(module_id),
            Some(agent),
            json!({ "module": module_name, "description": description }),
        )
        .await?;
        tx.commit().await?;
        Ok(Ack::new(format!(
            "Blocker recorded on '{module_name}'. Your claim is kept; the manager will see \
             blocker_reported on its next poll. Stop work until it responds."
        )))
    }

    /// Hand the module back with task states intact — the honorable
    /// exit for a worker that cannot finish.
    pub async fn release_module(
        &self,
        actor: impl Into<Actor>,
        module_id: &str,
        reason: &str,
    ) -> Result<Ack> {
        let actor = actor.into();
        let agent: &str = &actor.name;
        let mut tx = self.pool.begin().await?;
        let row = sqlx::query(
            "SELECT m.id AS module_id, m.name AS module_name, m.status, m.claimed_by, s.feature_id
             FROM modules m JOIN stages s ON s.id = m.stage_id
             WHERE m.id = $1 FOR UPDATE OF m",
        )
        .bind(module_id)
        .fetch_optional(&mut *tx)
        .await?
        .ok_or_else(|| McpmError::not_found("module", module_id))?;
        require_claim(&row, &actor, "release it")?;
        let module_name: String = row.get("module_name");
        let feature_id: String = row.get("feature_id");
        sqlx::query("UPDATE modules SET status = 'todo', claimed_by = NULL WHERE id = $1")
            .bind(module_id)
            .execute(&mut *tx)
            .await?;
        // The module is going back in the pool, so the identity minted
        // to work it stops resolving with it.
        retire_delegations(&mut tx, module_id).await?;
        record_event(
            &mut tx,
            "module_released",
            Some(&feature_id),
            Some(module_id),
            Some(agent),
            json!({ "module": module_name, "reason": reason }),
        )
        .await?;
        tx.commit().await?;
        Ok(Ack::new(format!(
            "Module '{module_name}' released back to the pool with its task states intact. \
             The next claimant inherits the checklist and everything committed to memory."
        )))
    }

    // -----------------------------------------------------------------
    // Memory
    // -----------------------------------------------------------------

    /// Pin a searchable note to one node of the tree.
    pub async fn commit_memory(
        &self,
        agent: &str,
        scope: MemoryScope,
        kind: MemoryKind,
        content: &str,
        tags: &[String],
        supersedes: &[Supersede],
    ) -> Result<Memory> {
        let scope = scope.normalized();
        self.subject_name(scope.level, &scope.id)
            .await?
            .ok_or_else(|| McpmError::not_found(scope.level.as_str(), &scope.id))?;

        // Idempotency. Agents retry, and a retry is not a new belief —
        // it must not become a second row that then has to be
        // superseded by hand. Only when nothing new is being asserted:
        // a repeat that ALSO declares a supersession is a real write.
        if supersedes.is_empty() {
            let existing: Option<String> = sqlx::query_scalar(
                "SELECT id FROM memories
                 WHERE level = $1 AND subject_id = $2 AND content = $3 AND author = $4
                 ORDER BY created_at LIMIT 1",
            )
            .bind(scope.level.as_str())
            .bind(&scope.id)
            .bind(content)
            .bind(agent)
            .fetch_optional(&self.pool)
            .await?;
            if let Some(id) = existing {
                return self
                    .memory_by_id(&id)
                    .await?
                    .ok_or_else(|| McpmError::internal("memory vanished mid-commit"));
            }
        }

        let mut tx = self.pool.begin().await?;
        let tag_refs: Vec<&str> = tags.iter().map(String::as_str).collect();
        let id =
            insert_memory(&mut tx, scope.level, &scope.id, kind, content, &tag_refs, agent).await?;

        // The supersession edges, inside the same transaction as the
        // memory they belong to: a correction that landed without its
        // link would be an orphan claiming to be the current truth
        // while the thing it replaced still read as current too.
        for sup in supersedes {
            let target: Option<String> =
                sqlx::query_scalar("SELECT content FROM memories WHERE id = $1")
                    .bind(&sup.memory_id)
                    .fetch_optional(&mut *tx)
                    .await?;
            let target = target.ok_or_else(|| McpmError::not_found("memory", &sup.memory_id))?;
            if target == content {
                return Err(McpmError::new(
                    ErrorCode::PlanInvalid,
                    format!(
                        "The new memory is byte-identical to {} — superseding it would add a \
                         step to the lineage that changed nothing.",
                        sup.memory_id
                    ),
                    json!({ "memory_id": sup.memory_id }),
                    "If the wording needed no change, you are agreeing with it: confirm_memory \
                     records that. If it needed a change, make the change.",
                ));
            }
            sqlx::query(
                "INSERT INTO memory_edges (from_id, to_id, kind, rationale, author)
                 VALUES ($1, $2, $3, $4, $5)",
            )
            .bind(&id)
            .bind(&sup.memory_id)
            .bind(sup.kind.as_str())
            .bind(&sup.rationale)
            .bind(agent)
            .execute(&mut *tx)
            .await?;
            record_event(
                &mut tx,
                "memory_superseded",
                None,
                Some(&sup.memory_id),
                Some(agent),
                json!({ "by": id, "kind": sup.kind.as_str(), "rationale": sup.rationale }),
            )
            .await?;
        }
        let feature_id = self.feature_of(scope.level, &scope.id).await?;
        record_event(
            &mut tx,
            "memory_committed",
            feature_id.as_deref(),
            Some(&scope.id),
            Some(agent),
            json!({ "level": scope.level.as_str(), "kind": kind.as_str(), "memory_id": id }),
        )
        .await?;
        tx.commit().await?;
        // Read it back through the same projection every other caller
        // sees, so the returned memory carries its derived state and
        // standing rather than a hand-built approximation of them.
        self.memory_by_id(&id)
            .await?
            .ok_or_else(|| McpmError::internal("committed memory vanished before it could be read"))
    }

    /// Search the knowledge base.
    ///
    /// Three routes to a match, unioned, because they fail differently:
    ///
    /// - **Lexemes.** The stemmed `tsvector`, with each term OR'd
    ///   against its synonyms so "db" reaches a note that says
    ///   "postgres".
    /// - **Any-term.** The same lexemes OR'd together, so a four-word
    ///   question still finds the note that answers three of them.
    ///   Without this a longer, more natural query returns strictly
    ///   less than a shorter one.
    /// - **Trigrams**, per term, catching typos and truncations the
    ///   stemmer cannot recover from.
    ///
    /// That produces textual relevance. It is then multiplied by the
    /// entry's standing — the evidence agents have given it, decayed by
    /// how much the project has learned since it was written. The
    /// multiplier never reaches zero (see `rank_floor`), so a stale,
    /// disputed memory still surfaces for a caller who searches its
    /// exact words. Nothing here is ever hidden by arithmetic; only
    /// ranked lower.
    ///
    /// Superseded and disputed entries are out of the results unless
    /// [`MemoryQuery::include_superseded`] asks for them.
    pub async fn search_memory(&self, q: &MemoryQuery) -> Result<MemoryPage> {
        let scope = q.scope.clone().map(MemoryScope::normalized);
        let (unscoped, sets) = match (&scope, q.direction) {
            (None, _) | (_, SearchDirection::All) => (true, SubjectSets::default()),
            (Some(scope), SearchDirection::Down) if scope.level == Level::Project => {
                (true, SubjectSets::default())
            }
            (Some(scope), dir) => (false, self.subject_sets(scope, dir).await?),
        };

        let terms = lexemes(&q.text);
        // Three tsqueries, not two. `raw` carries only the words the
        // caller actually typed: a note using their word must outrank
        // one reached through a synonym, or expansion turns every
        // database question into every database note.
        let (strict, raw, loose) = if terms.is_empty() {
            (String::new(), String::new(), String::new())
        } else {
            let groups = self.expand(&terms).await?;
            (groups.join(" & "), terms.join(" | "), groups.join(" | "))
        };
        // Trigrams get a stricter list. A short function word scores a
        // PERFECT similarity against any text containing it — measured,
        // "the" scores 1.00 against a sentence with "the" in it — so a
        // natural-language question would rank every entry by whichever
        // one happened to contain its stop words, and the longest,
        // most conversational queries would be the worst served.
        //
        // Four characters is where trigram similarity starts meaning
        // something: below it there are too few trigrams for a match to
        // be evidence of anything. Short terms still reach the tsquery,
        // which stems and stop-words them properly.
        let fuzzy: Vec<String> = terms.iter().filter(|t| t.len() >= 4).cloned().collect();
        let kinds: Vec<String> = q.kinds.iter().map(|k| k.as_str().to_string()).collect();
        let limit = if q.limit <= 0 { 20 } else { q.limit.clamp(1, 200) };
        let offset = q.offset.max(0);

        // One CTE so the count and the page come from the same
        // predicate. Two statements would let a write between them
        // report "1-20 of 19".
        let sql = format!("
            WITH {ctes},
            matched AS (
                SELECT {fields},
                       (CASE WHEN cardinality($9::text[]) = 0 THEN 1.0
                             ELSE 3.0 * ts_rank(mem.tsv, to_tsquery('english', $6))
                                + 2.0 * ts_rank(mem.tsv, to_tsquery('english', $8))
                                + 1.0 * ts_rank(mem.tsv, to_tsquery('english', $7))
                                + 1.5 * (SELECT COALESCE(MAX(word_similarity(t2, mem.content)), 0)
                                         FROM unnest($20::text[]) AS t2)
                        END)::real AS relevance
                {joins}
                WHERE ($1 OR (mem.level = 'project' AND $2)
                          OR (mem.level = 'feature' AND mem.subject_id = ANY($3))
                          OR (mem.level = 'stage'   AND mem.subject_id = ANY($4))
                          OR (mem.level = 'module'  AND mem.subject_id = ANY($5))
                          OR (mem.level = 'task'    AND mem.subject_id = ANY($10)))
                  -- Fuzzy matching is per TERM, not on the whole query
                  -- string: two typos in one phrase drag the phrase's
                  -- similarity below any usable threshold, while each
                  -- misspelled word on its own still scores well against
                  -- the word it meant.
                  --
                  -- An explicit threshold rather than the `<%` operator,
                  -- whose cutoff is a session GUC we cannot set reliably
                  -- from a pool. 0.5 sits in a wide gap: measured, a real
                  -- transposition scores 0.57 and unrelated text 0.00.
                  AND (cardinality($9::text[]) = 0
                       OR mem.tsv @@ to_tsquery('english', $7)
                       OR EXISTS (SELECT 1 FROM unnest($20::text[]) AS t2
                                  WHERE word_similarity(t2, mem.content) > 0.5))
                  AND (cardinality($11::text[]) = 0 OR mem.kind = ANY($11))
                  AND (cardinality($12::text[]) = 0 OR mem.tags @> $12)
                  AND ($13::text IS NULL OR mem.author = $13)
                  AND ($14::timestamptz IS NULL OR mem.created_at >= $14)
                  AND ($15::timestamptz IS NULL OR mem.created_at < $15)
                  AND ($18
                       OR (COALESCE(ed.superseded_by, 0) = 0
                           AND COALESCE(sig.disputes, 0) <= COALESCE(sig.confirms, 0))
                       -- A refuted entry rides along when asked for: the
                       -- successor says what is true now, and this says
                       -- what was already tried and did not work.
                       OR ($19 AND EXISTS (SELECT 1 FROM memory_edges r
                                           WHERE r.to_id = mem.id AND r.kind = 'refutes')))
            )
            SELECT *, COUNT(*) OVER () AS total,
                   (relevance * multiplier)::real AS score
            FROM matched
            ORDER BY score DESC, created_at DESC
            LIMIT $16 OFFSET $17",
            ctes = MEMORY_CTES, fields = MEMORY_FIELDS, joins = MEMORY_JOINS);

        let rows = sqlx::query(&sql)
            .bind(unscoped)
            .bind(sets.project)
            .bind(&sets.features)
            .bind(&sets.stages)
            .bind(&sets.modules)
            .bind(&strict)
            .bind(&loose)
            .bind(&raw)
            .bind(&terms)
            .bind(&sets.tasks)
            .bind(&kinds)
            .bind(&q.tags)
            .bind(&q.author)
            .bind(q.since)
            .bind(q.until)
            .bind(limit)
            .bind(offset)
            .bind(q.include_superseded)
            .bind(q.include_refuted)
            .bind(&fuzzy)
            .fetch_all(&self.pool)
            .await?;

        let total = rows.first().map(|r| r.get::<i64, _>("total")).unwrap_or(0);
        Ok(MemoryPage {
            total,
            hits: rows
                .iter()
                .map(|r| MemoryHit { relevance: r.get::<f32, _>("score"), memory: map_memory(r) })
                .collect(),
        })
    }

    /// Expand each term into a parenthesised `(term|synonym|…)` group,
    /// ready to be joined into a tsquery.
    ///
    /// The last term also gets a `:*` prefix match, so a half-typed word
    /// in a live search box narrows instead of returning nothing — which
    /// is what makes the console's field feel like search rather than a
    /// submit button.
    async fn expand(&self, terms: &[String]) -> Result<Vec<String>> {
        let rows = sqlx::query("SELECT term, synonyms FROM knowledge_synonyms WHERE term = ANY($1)")
            .bind(terms)
            .fetch_all(&self.pool)
            .await?;

        let mut out = Vec::with_capacity(terms.len());
        for (i, term) in terms.iter().enumerate() {
            let mut group: Vec<String> = vec![term.clone()];
            if let Some(row) = rows.iter().find(|r| r.get::<String, _>("term") == *term) {
                group.extend(row.get::<Vec<String>, _>("synonyms"));
            }
            if i + 1 == terms.len() {
                // Prefix-match the word still being typed. Only the
                // last one: mid-query terms are finished words, and
                // prefixing them would quietly widen every result.
                group[0] = format!("{term}:*");
            }
            out.push(format!("({})", group.join(" | ")));
        }
        Ok(out)
    }


    // -----------------------------------------------------------------
    // The memory graph
    // -----------------------------------------------------------------
    //
    // Everything here is append-only. Nothing updates a memory and
    // nothing deletes one; a correction is a NEW memory carrying a
    // supersession edge, and evidence accumulates as rows rather than
    // as a number on the memory it is about. See KNOWLEDGE.md.

    /// Attest that these memories were used. Bulk, because an agent that
    /// leaned on five entries should say so once.
    ///
    /// Unknown ids are reported rather than ignored: an agent touching
    /// an id that does not exist has misread something, and silently
    /// accepting it would hide that.
    pub async fn touch_memories(&self, agent: &str, ids: &[String], note: &str) -> Result<Ack> {
        if ids.is_empty() {
            return Err(McpmError::new(
                ErrorCode::PlanInvalid,
                "touch_memory needs at least one memory id.",
                json!({}),
                "Pass the ids of the memories you actually used. If you used none, call \
                 nothing — an empty touch is not a claim.",
            ));
        }
        // One batch id for the whole call: a bulk touch is one act of
        // "I leaned on these together", and that co-occurrence is what
        // the suggestion query later reads.
        let batch = uuid::Uuid::new_v4();
        let mut tx = self.pool.begin().await?;
        let mut recorded = Vec::new();
        for id in ids {
            let exists: Option<String> = sqlx::query_scalar("SELECT id FROM memories WHERE id = $1")
                .bind(id)
                .fetch_optional(&mut *tx)
                .await?;
            if exists.is_none() {
                return Err(McpmError::not_found("memory", id));
            }
            insert_signal(&mut tx, id, agent, SignalKind::Touch, note, Some(batch)).await?;
            recorded.push(id.clone());
        }
        tx.commit().await?;
        Ok(Ack {
            ok: true,
            message: format!("Recorded {} touch(es).", recorded.len()),
            data: json!({ "touched": recorded }),
        })
    }

    /// "I checked this and it holds."
    pub async fn confirm_memory(&self, agent: &str, id: &str, note: &str) -> Result<Ack> {
        self.attest(agent, id, SignalKind::Confirm, note).await
    }

    /// "This is wrong." Does not delete anything: it takes the memory
    /// out of default results and leaves it findable, because a wrong
    /// belief the project once held is part of the record.
    pub async fn dispute_memory(&self, agent: &str, id: &str, note: &str) -> Result<Ack> {
        if note.trim().is_empty() {
            return Err(McpmError::new(
                ErrorCode::SkipNeedsReason,
                "A dispute needs a reason.",
                json!({ "memory_id": id }),
                "Say what is wrong with it. A dispute with no reason cannot be resolved by \
                 anyone but you, and it takes the memory out of circulation until someone does.",
            ));
        }
        self.attest(agent, id, SignalKind::Dispute, note).await
    }

    /// The shared body of confirm/dispute, including the independence
    /// rule: an agent cannot corroborate or refute its own memory.
    /// Grading your own work is not evidence, and letting it count would
    /// make the confirm count a measure of how many memories an eager
    /// agent wrote.
    async fn attest(
        &self,
        agent: &str,
        id: &str,
        kind: SignalKind,
        note: &str,
    ) -> Result<Ack> {
        let author: Option<String> = sqlx::query_scalar("SELECT author FROM memories WHERE id = $1")
            .bind(id)
            .fetch_optional(&self.pool)
            .await?;
        let author = author.ok_or_else(|| McpmError::not_found("memory", id))?;
        if author == agent {
            return Err(McpmError::new(
                ErrorCode::PlanInvalid,
                format!("You wrote {id}; you cannot {} your own memory.", kind.as_str()),
                json!({ "memory_id": id, "author": author }),
                "Corroboration needs independence. If you have learned this is wrong, commit a \
                 new memory that supersedes it with kind='refutes' instead — that records both \
                 what changed and why.",
            ));
        }
        let mut tx = self.pool.begin().await?;
        insert_signal(&mut tx, id, agent, kind, note, None).await?;
        tx.commit().await?;
        Ok(Ack {
            ok: true,
            message: format!("Recorded {} on {id}.", kind.as_str()),
            data: json!({ "memory_id": id, "signal": kind.as_str() }),
        })
    }

    /// A memory's lineage in both directions.
    ///
    /// `belief_only` collapses the clerical steps (`revises`,
    /// `consolidates`): asking what the project used to think and
    /// getting three rephrasings of one idea buries the one place the
    /// idea actually changed.
    pub async fn memory_history(&self, id: &str, belief_only: bool) -> Result<MemoryHistory> {
        let anchor = self
            .memory_by_id(id)
            .await?
            .ok_or_else(|| McpmError::not_found("memory", id))?;
        Ok(MemoryHistory {
            anchor,
            supersedes: self.walk(id, true, belief_only).await?,
            superseded_by: self.walk(id, false, belief_only).await?,
            relations: self.relations_of(id).await?,
            suggestions: self.suggestions_for(id).await?,
        })
    }

    /// Declare a standing relation between two memories.
    ///
    /// Separate from supersession on purpose: this describes how two
    /// things sit together and retires neither. Supersession stays
    /// commit-time-only, which is what keeps that spine acyclic.
    pub async fn relate_memories(
        &self,
        agent: &str,
        from: &str,
        to: &str,
        kind: EdgeKind,
        rationale: &str,
    ) -> Result<Ack> {
        if kind.supersedes() {
            return Err(McpmError::new(
                ErrorCode::PlanInvalid,
                format!("'{}' retires a memory; it cannot be declared between two that already exist.", kind.as_str()),
                json!({ "kind": kind.as_str() }),
                "Supersession is declared when the replacement is COMMITTED — pass it to \
                 commit_memory's `supersedes`. That is what guarantees the history has no \
                 cycles. Use refines, depends_on, contradicts or relates_to here.",
            ));
        }
        if from == to {
            return Err(McpmError::new(
                ErrorCode::PlanInvalid,
                "A memory cannot relate to itself.".to_string(),
                json!({ "memory_id": from }),
                "Pass two different memory ids.",
            ));
        }
        for id in [from, to] {
            if self.memory_by_id(id).await?.is_none() {
                return Err(McpmError::not_found("memory", id));
            }
        }
        let done = sqlx::query(
            "INSERT INTO memory_edges (from_id, to_id, kind, rationale, author)
             VALUES ($1, $2, $3, $4, $5) ON CONFLICT (from_id, to_id) DO NOTHING",
        )
        .bind(from)
        .bind(to)
        .bind(kind.as_str())
        .bind(rationale)
        .bind(agent)
        .execute(&self.pool)
        .await?;
        Ok(Ack {
            ok: true,
            message: if done.rows_affected() > 0 {
                format!("{from} {} {to}.", kind.as_str())
            } else {
                format!("{from} and {to} were already linked.")
            },
            data: json!({ "from": from, "to": to, "kind": kind.as_str() }),
        })
    }

    /// Declared standing relations on a memory, both directions.
    async fn relations_of(&self, id: &str) -> Result<Vec<Relation>> {
        let sql = format!("
            WITH {ctes}
            SELECT {fields}, e.kind AS edge_kind, e.rationale, e.author AS edge_author,
                   (e.from_id = $1) AS outgoing
            {joins}
            JOIN memory_edges e
              ON (e.from_id = $1 AND e.to_id = mem.id)
              OR (e.to_id = $1 AND e.from_id = mem.id)
            WHERE e.kind NOT IN ('replaces', 'refutes', 'revises', 'consolidates')
            ORDER BY e.at DESC",
            ctes = MEMORY_CTES, fields = MEMORY_FIELDS, joins = MEMORY_JOINS);
        let rows = sqlx::query(&sql).bind(id).fetch_all(&self.pool).await?;
        Ok(rows
            .iter()
            .filter_map(|r| {
                Some(Relation {
                    kind: EdgeKind::parse(r.get::<String, _>("edge_kind").as_str())?,
                    outgoing: r.get("outgoing"),
                    rationale: r.get("rationale"),
                    author: r.get("edge_author"),
                    other: map_memory(r),
                })
            })
            .collect())
    }

    /// Relations nobody has declared, inferred from agents having used
    /// two memories in the same breath.
    ///
    /// Deliberately returned as candidates and never written. Co-use is
    /// the weakest evidence a relation exists — two memories can be
    /// read together for a hundred reasons — so it earns a question,
    /// not an edge. Anything already declared or superseded is excluded:
    /// suggesting what somebody has said trains a reader to ignore the
    /// list.
    async fn suggestions_for(&self, id: &str) -> Result<Vec<Suggestion>> {
        let sql = format!("
            WITH {ctes},
            mine AS (
                SELECT DISTINCT batch FROM memory_signals
                WHERE memory_id = $1 AND kind = 'touch' AND batch IS NOT NULL
            ),
            co AS (
                SELECT s.memory_id, COUNT(DISTINCT s.agent) AS co_touches
                FROM memory_signals s JOIN mine ON mine.batch = s.batch
                WHERE s.memory_id <> $1 AND s.kind = 'touch'
                GROUP BY s.memory_id
            )
            SELECT {fields}, co.co_touches
            {joins}
            JOIN co ON co.memory_id = mem.id
            WHERE NOT EXISTS (SELECT 1 FROM memory_edges e
                              WHERE (e.from_id = $1 AND e.to_id = mem.id)
                                 OR (e.to_id = $1 AND e.from_id = mem.id))
              AND COALESCE(ed.superseded_by, 0) = 0
            ORDER BY co.co_touches DESC, mem.created_at DESC
            LIMIT 5",
            ctes = MEMORY_CTES, fields = MEMORY_FIELDS, joins = MEMORY_JOINS);
        let rows = sqlx::query(&sql).bind(id).fetch_all(&self.pool).await?;
        Ok(rows
            .iter()
            .map(|r| Suggestion { other: map_memory(r), co_touches: r.get("co_touches") })
            .collect())
    }

    /// Walk the supersession spine. `backwards` follows what this
    /// memory superseded; otherwise what superseded it.
    async fn walk(&self, id: &str, backwards: bool, belief_only: bool) -> Result<Vec<HistoryStep>> {
        // Edges always point from newer to older (they can only be
        // declared when the newer memory is committed), so this
        // recursion is guaranteed to terminate — there is no cycle to
        // guard against. The depth cap is belt and braces.
        let (seed, step) = if backwards {
            ("e.from_id = $1", "e.from_id = h.next_id")
        } else {
            ("e.to_id = $1", "e.to_id = h.next_id")
        };
        let next = if backwards { "e.to_id" } else { "e.from_id" };
        let sql = format!("
            WITH {ctes},
            RECURSIVE_PLACEHOLDER
            SELECT {fields}, h.depth, h.via, h.rationale
            {joins}
            JOIN hops h ON h.next_id = mem.id
            ORDER BY h.depth",
            ctes = MEMORY_CTES, fields = MEMORY_FIELDS, joins = MEMORY_JOINS);
        // The recursive term has to come first in the WITH list, so it
        // is spliced in rather than appended.
        let hops = format!("
            hops AS (
                WITH RECURSIVE r(next_id, via, rationale, depth) AS (
                    SELECT {next}, e.kind, e.rationale, 1
                    FROM memory_edges e WHERE {seed}
                    UNION ALL
                    SELECT {next}, e.kind, e.rationale, r.depth + 1
                    FROM memory_edges e JOIN r ON {step_r}
                    WHERE r.depth < 64
                )
                SELECT * FROM r
            )",
            next = next,
            seed = seed,
            step_r = step.replace("h.next_id", "r.next_id"));
        let sql = sql.replace("RECURSIVE_PLACEHOLDER", hops.trim_start());

        let rows = sqlx::query(&sql).bind(id).fetch_all(&self.pool).await?;
        Ok(rows
            .iter()
            .filter_map(|r| {
                let via = EdgeKind::parse(r.get::<String, _>("via").as_str());
                if belief_only && !via.map(EdgeKind::changes_belief).unwrap_or(true) {
                    return None;
                }
                Some(HistoryStep {
                    memory: map_memory(r),
                    via,
                    rationale: r.get("rationale"),
                })
            })
            .collect())
    }

    /// One memory by id, scored, whatever its state.
    pub async fn memory_by_id(&self, id: &str) -> Result<Option<Memory>> {
        let sql = format!(
            "WITH {ctes} SELECT {fields} {joins} WHERE mem.id = $1",
            ctes = MEMORY_CTES, fields = MEMORY_FIELDS, joins = MEMORY_JOINS
        );
        let row = sqlx::query(&sql).bind(id).fetch_optional(&self.pool).await?;
        Ok(row.as_ref().map(map_memory))
    }

    // -----------------------------------------------------------------
    // Wants — the idea pool
    // -----------------------------------------------------------------

    /// Capture a loose idea. Deliberately the cheapest write in the API:
    /// a body and optional tags, no structure demanded — structure is
    /// what `promote_wants` adds later, once several wants compose.
    pub async fn add_want(&self, agent: &str, body: &str, tags: &[String]) -> Result<Want> {
        let mut wants = self
            .add_wants(
                agent,
                vec![WantDraft {
                    body: body.to_string(),
                    tags: tags.to_vec(),
                }],
            )
            .await?;
        Ok(wants.remove(0))
    }

    /// Capture several loose ideas at once, atomically — the shape a
    /// person dumping a list has, and the one the console's composer
    /// posts. Every tag used is registered on the way through, so a tag
    /// typed for the first time exists from then on.
    ///
    /// All or nothing: one bad line rejects the batch rather than
    /// leaving half a list in the pool, because the half that landed and
    /// the half that didn't are indistinguishable afterwards.
    pub async fn add_wants(&self, agent: &str, drafts: Vec<WantDraft>) -> Result<Vec<Want>> {
        if drafts.is_empty() {
            return Err(plan_invalid("Nothing to capture — send at least one want."));
        }
        for (i, draft) in drafts.iter().enumerate() {
            if draft.body.trim().is_empty() {
                return Err(plan_invalid(format!(
                    "Want {} has an empty body — say what you want, however loosely. \
                     Nothing was written.",
                    i + 1
                )));
            }
        }

        let mut tx = self.pool.begin().await?;
        let mut ids = Vec::with_capacity(drafts.len());
        for draft in &drafts {
            let tags = register_tags(&mut tx, agent, &draft.tags).await?;
            let id = new_want_id();
            sqlx::query("INSERT INTO wants (id, body, tags, author) VALUES ($1, $2, $3, $4)")
                .bind(&id)
                .bind(draft.body.trim())
                .bind(&tags)
                .bind(agent)
                .execute(&mut *tx)
                .await?;
            record_event(
                &mut tx,
                "want_added",
                None,
                Some(&id),
                Some(agent),
                json!({ "body": draft.body.trim(), "tags": tags }),
            )
            .await?;
            ids.push(id);
        }
        tx.commit().await?;

        // Re-read in capture order (select_wants sorts newest-first).
        let mut wants = self
            .select_wants("", WantFilter::All, &[], &ids, None, ids.len() as i64)
            .await?;
        wants.sort_by_key(|w| ids.iter().position(|id| *id == w.id).unwrap_or(usize::MAX));
        Ok(wants)
    }

    // --- Tags ---------------------------------------------------------

    /// The tag registry, most-used first — the console's autocomplete
    /// vocabulary and the agent-facing filing index.
    pub async fn list_tags(&self) -> Result<Vec<TagInfo>> {
        let rows = sqlx::query(
            "SELECT t.name, t.label, t.created_by, t.created_at,
                    (SELECT COUNT(*) FROM wants w WHERE t.name = ANY(w.tags)) AS uses
             FROM tags t
             ORDER BY uses DESC, t.name",
        )
        .fetch_all(&self.pool)
        .await?;
        Ok(rows
            .into_iter()
            .map(|r| TagInfo {
                name: r.get("name"),
                label: r.get("label"),
                uses: r.get("uses"),
                created_by: r.get("created_by"),
                created_at: r.get("created_at"),
            })
            .collect())
    }

    /// Create a tag with no want attached — a preset, set up ahead of
    /// the ideas that will use it. Idempotent: an existing tag comes
    /// back unchanged rather than erroring, because "make sure this tag
    /// exists" is the caller's actual intent.
    pub async fn create_tag(&self, agent: &str, label: &str) -> Result<TagInfo> {
        let name = normalize_tag(label).ok_or_else(|| {
            plan_invalid(format!(
                "'{label}' has no usable tag in it — a tag needs at least one letter or digit."
            ))
        })?;
        let mut tx = self.pool.begin().await?;
        let created = insert_tag(&mut tx, agent, &name, label.trim().trim_start_matches('#')).await?;
        if created {
            record_event(
                &mut tx,
                "tag_created",
                None,
                None,
                Some(agent),
                json!({ "tag": name }),
            )
            .await?;
        }
        tx.commit().await?;
        self.list_tags()
            .await?
            .into_iter()
            .find(|t| t.name == name)
            .ok_or_else(|| McpmError::not_found("tag", &name))
    }

    /// Read one want by id.
    /// Read one want by id.
    pub async fn get_want(&self, id: &str) -> Result<Want> {
        self.select_wants("", WantFilter::All, &[], &[id.to_string()], None, 1)
            .await?
            .pop()
            .ok_or_else(|| McpmError::not_found("want", id))
    }

    /// The pool: full-text search + status/tag filters, with the
    /// project-wide counts alongside so a composing agent sees the whole
    /// backlog's shape in one call.
    pub async fn list_wants(
        &self,
        query: &str,
        filter: WantFilter,
        tags: &[String],
        limit: i64,
    ) -> Result<WantPool> {
        let wants = self.select_wants(query, filter, tags, &[], None, limit).await?;
        let counts = sqlx::query(
            "SELECT
               COUNT(*) FILTER (WHERE w.state = 'open' AND NOT EXISTS
                 (SELECT 1 FROM want_features wf WHERE wf.want_id = w.id)) AS open,
               COUNT(*) FILTER (WHERE EXISTS
                 (SELECT 1 FROM want_features wf WHERE wf.want_id = w.id)) AS promoted,
               COUNT(*) FILTER (WHERE w.state = 'declined' AND NOT EXISTS
                 (SELECT 1 FROM want_features wf WHERE wf.want_id = w.id)) AS declined
             FROM wants w",
        )
        .fetch_one(&self.pool)
        .await?;
        let open: i64 = counts.get("open");
        let note = if open == 0 {
            "No open wants. Nothing to compose right now.".to_string()
        } else {
            format!(
                "{open} open want(s). Read them for THEMES, not one-to-one: several wants \
                 usually compose into one feature, and one want can inform several. When a \
                 group coheres, call promote_wants with that group and the plan it becomes.",
            )
        };
        Ok(WantPool {
            open,
            promoted: counts.get("promoted"),
            declined: counts.get("declined"),
            wants,
            note,
        })
    }

    /// Every want a feature was composed from (oldest link first).
    pub async fn wants_of_feature(&self, feature_id: &str) -> Result<Vec<Want>> {
        self.select_wants("", WantFilter::All, &[], &[], Some(feature_id), 200)
            .await
    }

    /// Revise a want: sharpen the wording, retag, decline it with a
    /// reason, or reopen a declined one.
    ///
    /// A want a feature already absorbed is frozen: rewriting the idea
    /// after the plan quotes it would make the record lie. Retagging
    /// stays legal, because tags are filing, not content.
    pub async fn update_want(&self, agent: &str, id: &str, edit: WantEdit) -> Result<Want> {
        let current = self.get_want(id).await?;
        let promoted = current.status == "promoted";

        if let Some(body) = &edit.body {
            if body.trim().is_empty() {
                return Err(plan_invalid("A want needs a non-empty body."));
            }
            if promoted && body.trim() != current.body {
                return Err(want_promoted(
                    &current,
                    "Its wording is quoted by a plan that already exists.",
                    "Leave this want as the record of what was asked, and add_want a new one \
                     for the refined idea — then promote that into the feature too.",
                ));
            }
        }
        if let Some(WantState::Declined) = edit.state {
            if promoted {
                return Err(want_promoted(
                    &current,
                    "A feature already absorbed it, so declining it would contradict the plan.",
                    "If the work should not happen, revise or shelve the FEATURE instead.",
                ));
            }
            if edit.reason.as_deref().map(str::trim).unwrap_or("").is_empty() {
                return Err(McpmError::new(
                    ErrorCode::SkipNeedsReason,
                    "Declining a want requires a reason.",
                    json!({ "want_id": id }),
                    "Resend with `reason` — a declined want without a why is just a lost idea, \
                     and the next agent will re-propose it.",
                ));
            }
        }

        let mut tx = self.pool.begin().await?;
        if let Some(body) = &edit.body {
            sqlx::query("UPDATE wants SET body = $2, updated_at = now() WHERE id = $1")
                .bind(id)
                .bind(body.trim())
                .execute(&mut *tx)
                .await?;
        }
        if let Some(tags) = &edit.tags {
            let tags = register_tags(&mut tx, agent, tags).await?;
            sqlx::query("UPDATE wants SET tags = $2, updated_at = now() WHERE id = $1")
                .bind(id)
                .bind(&tags)
                .execute(&mut *tx)
                .await?;
        }
        match edit.state {
            Some(WantState::Declined) => {
                let reason = edit.reason.clone().unwrap_or_default();
                sqlx::query(
                    "UPDATE wants SET state = 'declined', decline_reason = $2, updated_at = now()
                     WHERE id = $1",
                )
                .bind(id)
                .bind(reason.trim())
                .execute(&mut *tx)
                .await?;
                record_event(
                    &mut tx,
                    "want_declined",
                    None,
                    Some(id),
                    Some(agent),
                    json!({ "body": current.body, "reason": reason.trim() }),
                )
                .await?;
            }
            Some(WantState::Open) => {
                sqlx::query(
                    "UPDATE wants SET state = 'open', decline_reason = NULL, updated_at = now()
                     WHERE id = $1",
                )
                .bind(id)
                .execute(&mut *tx)
                .await?;
                if current.status == "declined" {
                    record_event(
                        &mut tx,
                        "want_reopened",
                        None,
                        Some(id),
                        Some(agent),
                        json!({ "body": current.body }),
                    )
                    .await?;
                }
            }
            None => {}
        }
        if edit.body.is_some() || edit.tags.is_some() {
            record_event(
                &mut tx,
                "want_updated",
                None,
                Some(id),
                Some(agent),
                json!({
                    "body": edit.body.as_deref().map(str::trim).unwrap_or(&current.body),
                    "retagged": edit.tags.is_some(),
                }),
            )
            .await?;
        }
        tx.commit().await?;
        self.get_want(id).await
    }

    /// Compose wants into a feature — the one write that turns loose
    /// ideas into planned work.
    ///
    /// Either `plan` (create the feature here) or `feature_id` (attach
    /// to one that exists). The feature, every link, the event, and the
    /// origin memory commit together or not at all: there is no state in
    /// which a feature exists but has forgotten which wants it came
    /// from. A want may inform several features — that is the point of a
    /// pool — but linking it twice to the SAME feature is refused.
    pub async fn promote_wants(&self, agent: &str, req: PromoteWants) -> Result<Promotion> {
        if req.wants.is_empty() {
            return Err(plan_invalid(
                "promote_wants needs at least one want — that is what it composes.",
            ));
        }
        match (&req.plan, &req.feature_id) {
            (Some(_), Some(_)) => {
                return Err(plan_invalid(
                    "Give either `plan` (compose a new feature) or `feature_id` (fold these \
                     wants into an existing one) — not both.",
                ))
            }
            (None, None) => {
                return Err(plan_invalid(
                    "Give `plan` to compose these wants into a new feature, or `feature_id` to \
                     fold them into one that already exists.",
                ))
            }
            _ => {}
        }
        if let Some(plan) = &req.plan {
            validate_plan(plan)?;
        }

        // Resolve every want first: a promotion that would drop one is
        // refused whole, before anything is written.
        let ids: Vec<String> = req.wants.iter().map(|w| w.id.clone()).collect();
        let found = self
            .select_wants("", WantFilter::All, &[], &ids, None, ids.len() as i64)
            .await?;
        let missing: Vec<&String> = ids
            .iter()
            .filter(|id| !found.iter().any(|w| &&w.id == id))
            .collect();
        if !missing.is_empty() {
            return Err(McpmError::new(
                ErrorCode::NotFound,
                format!("Unknown want id(s): {missing:?}. Nothing was written."),
                json!({ "missing": missing }),
                "List the pool with list_wants and promote only ids it returned.",
            ));
        }
        let declined: Vec<&Want> = found.iter().filter(|w| w.status == "declined").collect();
        if !declined.is_empty() {
            return Err(plan_conflict(format!(
                "These wants were declined and cannot be promoted as-is: {}. Reopen each with \
                 update_want(state='open') if the decision has changed — the decline reason is \
                 on the record and may still be right.",
                declined
                    .iter()
                    .map(|w| format!("{} ({})", w.id, w.decline_reason.clone().unwrap_or_default()))
                    .collect::<Vec<_>>()
                    .join("; ")
            )));
        }

        let mut tx = self.pool.begin().await?;
        let feature_id = match (&req.plan, &req.feature_id) {
            (Some(plan), _) => insert_plan(&mut tx, agent, plan).await?,
            (_, Some(existing)) => {
                let known: Option<String> = sqlx::query_scalar("SELECT id FROM features WHERE id = $1")
                    .bind(existing)
                    .fetch_optional(&mut *tx)
                    .await?;
                known.ok_or_else(|| McpmError::not_found("feature", existing))?
            }
            _ => unreachable!("validated above"),
        };
        // Same-feature duplicates are a no-op the caller should know
        // about; links to OTHER features are legitimate reuse.
        let already: Vec<&Want> = found
            .iter()
            .filter(|w| w.features.iter().any(|l| l.feature_id == feature_id))
            .collect();
        if !already.is_empty() {
            return Err(McpmError::new(
                ErrorCode::AlreadyLinked,
                format!(
                    "Already part of that feature: {}. Nothing was written.",
                    already.iter().map(|w| w.id.as_str()).collect::<Vec<_>>().join(", ")
                ),
                json!({ "feature_id": feature_id, "wants": already.iter().map(|w| &w.id).collect::<Vec<_>>() }),
                "Drop those ids and resend with only the wants this feature has not absorbed \
                 yet. To extend the feature's plan, use revise_plan.",
            ));
        }

        let feature_name: String = sqlx::query_scalar("SELECT name FROM features WHERE id = $1")
            .bind(&feature_id)
            .fetch_one(&mut *tx)
            .await?;
        let mut origin = format!("This feature was composed from {} want(s):\n", req.wants.len());
        for want in &req.wants {
            let w = found.iter().find(|w| w.id == want.id).expect("resolved above");
            sqlx::query(
                "INSERT INTO want_features (want_id, feature_id, rationale, linked_by)
                 VALUES ($1, $2, $3, $4)",
            )
            .bind(&want.id)
            .bind(&feature_id)
            .bind(want.rationale.trim())
            .bind(agent)
            .execute(&mut *tx)
            .await?;
            origin.push_str(&format!("- [{}] {}", w.id, w.body));
            if !want.rationale.trim().is_empty() {
                origin.push_str(&format!("\n  → read as: {}", want.rationale.trim()));
            }
            origin.push('\n');
        }

        record_event(
            &mut tx,
            "wants_promoted",
            Some(&feature_id),
            Some(&feature_id),
            Some(agent),
            json!({
                "feature": feature_name,
                "count": req.wants.len(),
                "wants": ids,
                "new_feature": req.plan.is_some(),
            }),
        )
        .await?;
        // The origin note lands as a feature-scope memory, so every
        // worker that later searches `up` from its module reads the raw
        // ideas this work came from — in the words they were asked in.
        insert_memory(
            &mut tx,
            Level::Feature,
            &feature_id,
            MemoryKind::Decision,
            origin.trim_end(),
            &["wants", "origin"],
            agent,
        )
        .await?;
        tx.commit().await?;

        let linked = self
            .select_wants("", WantFilter::All, &[], &ids, None, ids.len() as i64)
            .await?;
        Ok(Promotion {
            feature: self.feature_tree(&feature_id).await?,
            linked,
            note: format!(
                "{} want(s) composed into '{}' ({}). Their origin is committed as a \
                 feature-scope memory — workers will read it via search_memory(direction='up'). \
                 Dispatch with next_work('{}').",
                req.wants.len(),
                feature_name,
                feature_id,
                feature_id
            ),
        })
    }

    /// The one want query: full-text + tag + derived-status filters, and
    /// optional restriction to a set of ids or to one feature's links.
    async fn select_wants(
        &self,
        query: &str,
        filter: WantFilter,
        tags: &[String],
        ids: &[String],
        feature_id: Option<&str>,
        limit: i64,
    ) -> Result<Vec<Want>> {
        let rows = sqlx::query(
            "WITH linked AS (
               SELECT wf.want_id,
                      array_agg(wf.feature_id ORDER BY wf.linked_at) AS feature_ids,
                      array_agg(f.name       ORDER BY wf.linked_at) AS feature_names,
                      array_agg(wf.rationale ORDER BY wf.linked_at) AS rationales
               FROM want_features wf JOIN features f ON f.id = wf.feature_id
               GROUP BY wf.want_id
             ),
             pool AS (
               SELECT w.id, w.body, w.tags, w.author, w.created_at, w.updated_at,
                      w.decline_reason, w.tsv,
                      CASE WHEN l.want_id IS NOT NULL THEN 'promoted'
                           WHEN w.state = 'declined'  THEN 'declined'
                           ELSE 'open' END AS status,
                      COALESCE(l.feature_ids,   '{}') AS feature_ids,
                      COALESCE(l.feature_names, '{}') AS feature_names,
                      COALESCE(l.rationales,    '{}') AS rationales
               FROM wants w LEFT JOIN linked l ON l.want_id = w.id
             )
             SELECT id, body, tags, author, created_at, updated_at, decline_reason, status,
                    feature_ids, feature_names, rationales
             FROM pool p
             WHERE ($1 = '' OR p.tsv @@ websearch_to_tsquery('english', $1))
               AND (cardinality($2::text[]) = 0 OR p.tags && $2)
               AND ($3 = 'all' OR p.status = $3)
               AND (cardinality($4::text[]) = 0 OR p.id = ANY($4))
               AND ($5::text IS NULL OR $5 = ANY(p.feature_ids))
             ORDER BY CASE WHEN $1 = '' THEN 0
                           ELSE ts_rank(p.tsv, websearch_to_tsquery('english', $1)) END DESC,
                      p.created_at DESC
             LIMIT $6",
        )
        .bind(query)
        .bind(tags)
        .bind(filter.as_str())
        .bind(ids)
        .bind(feature_id)
        .bind(limit.clamp(1, 500))
        .fetch_all(&self.pool)
        .await?;
        Ok(rows
            .into_iter()
            .map(|r| {
                let feature_ids: Vec<String> = r.get("feature_ids");
                let feature_names: Vec<String> = r.get("feature_names");
                let rationales: Vec<String> = r.get("rationales");
                Want {
                    id: r.get("id"),
                    body: r.get("body"),
                    tags: r.get("tags"),
                    status: r.get("status"),
                    decline_reason: r.get("decline_reason"),
                    author: r.get("author"),
                    created_at: r.get("created_at"),
                    updated_at: r.get("updated_at"),
                    features: feature_ids
                        .into_iter()
                        .zip(feature_names)
                        .zip(rationales)
                        .map(|((feature_id, feature_name), rationale)| WantLink {
                            feature_id,
                            feature_name,
                            rationale,
                        })
                        .collect(),
                }
            })
            .collect())
    }

    // -----------------------------------------------------------------
    // Audit
    // -----------------------------------------------------------------

    pub async fn get_events(&self, feature_id: Option<&str>, since: i64, limit: i64) -> Result<Vec<Event>> {
        let rows = sqlx::query(
            "SELECT seq, ts, type, feature_id, subject_id, agent, payload FROM events
             WHERE ($1::text IS NULL OR feature_id = $1) AND seq > $2
             ORDER BY seq LIMIT $3",
        )
        .bind(feature_id)
        .bind(since)
        .bind(limit.clamp(1, 500))
        .fetch_all(&self.pool)
        .await?;
        Ok(rows
            .into_iter()
            .map(|r| Event {
                seq: r.get("seq"),
                ts: r.get("ts"),
                kind: r.get("type"),
                feature_id: r.get("feature_id"),
                subject_id: r.get("subject_id"),
                agent: r.get("agent"),
                payload: r.get("payload"),
            })
            .collect())
    }

    // -----------------------------------------------------------------
    // Tree assembly + helpers
    // -----------------------------------------------------------------

    /// Full tree with derived stage statuses and dispatchability.
    pub async fn feature_tree(&self, feature_id: &str) -> Result<FeatureTree> {
        let feature = sqlx::query("SELECT id, name, description, status, summary FROM features WHERE id = $1")
            .bind(feature_id)
            .fetch_optional(&self.pool)
            .await?
            .ok_or_else(|| McpmError::not_found("feature", feature_id))?;

        let stage_rows = sqlx::query(
            "SELECT id, name, position FROM stages WHERE feature_id = $1 ORDER BY position",
        )
        .bind(feature_id)
        .fetch_all(&self.pool)
        .await?;
        let module_rows = sqlx::query(
            "SELECT m.id, m.stage_id, m.name, m.description, m.status, m.claimed_by, m.summary
             FROM modules m JOIN stages s ON s.id = m.stage_id
             WHERE s.feature_id = $1 ORDER BY s.position, m.name",
        )
        .bind(feature_id)
        .fetch_all(&self.pool)
        .await?;
        let task_rows = sqlx::query(
            "SELECT t.id, t.module_id, t.name, t.status, t.origin, t.note
             FROM tasks t JOIN modules m ON m.id = t.module_id
             JOIN stages s ON s.id = m.stage_id
             WHERE s.feature_id = $1 ORDER BY t.position, t.created_at",
        )
        .bind(feature_id)
        .fetch_all(&self.pool)
        .await?;

        // Lowest stage position holding any unfinished module = the
        // unlocked frontier. Everything before is done; after, locked.
        let lowest_open: Option<i32> = stage_rows
            .iter()
            .filter(|s| {
                let sid: String = s.get("id");
                module_rows.iter().any(|m| {
                    m.get::<String, _>("stage_id") == sid && m.get::<String, _>("status") != "done"
                })
            })
            .map(|s| s.get::<i32, _>("position"))
            .min();

        let stages = stage_rows
            .iter()
            .map(|s| {
                let sid: String = s.get("id");
                let position: i32 = s.get("position");
                let status = match lowest_open {
                    None => "done",
                    Some(open) if position < open => "done",
                    Some(open) if position == open => "unlocked",
                    Some(_) => "locked",
                };
                let modules = module_rows
                    .iter()
                    .filter(|m| m.get::<String, _>("stage_id") == sid)
                    .map(|m| {
                        let mid: String = m.get("id");
                        let mstatus: String = m.get("status");
                        let claimed: Option<String> = m.get("claimed_by");
                        ModuleView {
                            dispatchable: mstatus == "todo"
                                && claimed.is_none()
                                && status == "unlocked",
                            id: mid.clone(),
                            name: m.get("name"),
                            description: m.get("description"),
                            status: mstatus,
                            claimed_by: claimed,
                            summary: m.get("summary"),
                            tasks: task_rows
                                .iter()
                                .filter(|t| t.get::<String, _>("module_id") == mid)
                                .map(|t| TaskView {
                                    id: t.get("id"),
                                    name: t.get("name"),
                                    status: t.get("status"),
                                    origin: t.get("origin"),
                                    note: t.get("note"),
                                })
                                .collect(),
                        }
                    })
                    .collect();
                StageView {
                    id: sid,
                    name: s.get("name"),
                    position,
                    status: status.to_string(),
                    modules,
                }
            })
            .collect();

        Ok(FeatureTree {
            id: feature.get("id"),
            name: feature.get("name"),
            description: feature.get("description"),
            status: feature.get("status"),
            summary: feature.get("summary"),
            stages,
        })
    }

    async fn tasks_of(&self, module_id: &str) -> Result<Vec<TaskView>> {
        let rows = sqlx::query(
            "SELECT id, name, status, origin, note FROM tasks
             WHERE module_id = $1 ORDER BY position, created_at",
        )
        .bind(module_id)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows
            .into_iter()
            .map(|t| TaskView {
                id: t.get("id"),
                name: t.get("name"),
                status: t.get("status"),
                origin: t.get("origin"),
                note: t.get("note"),
            })
            .collect())
    }

    async fn subject_name(&self, level: Level, id: &str) -> Result<Option<String>> {
        // Project scope is a singleton with no row of its own in the
        // tree; its "name" is the project's.
        if level == Level::Project {
            let row = sqlx::query("SELECT name FROM project WHERE id = 1")
                .fetch_optional(&self.pool)
                .await?;
            return Ok(Some(
                row.map(|r| r.get::<String, _>("name"))
                    .unwrap_or_else(|| "project".to_string()),
            ));
        }
        let table = match level {
            Level::Project => unreachable!("handled above"),
            Level::Feature => "features",
            Level::Stage => "stages",
            Level::Module => "modules",
            Level::Task => "tasks",
        };
        let row = sqlx::query(&format!("SELECT name FROM {table} WHERE id = $1"))
            .bind(id)
            .fetch_optional(&self.pool)
            .await?;
        Ok(row.map(|r| r.get("name")))
    }

    /// The feature an id belongs to (for event attribution).
    async fn feature_of(&self, level: Level, id: &str) -> Result<Option<String>> {
        // A project-scoped memory belongs to no feature — that is the
        // point of the scope — so its event carries no feature_id.
        if level == Level::Project {
            return Ok(None);
        }
        let sql = match level {
            Level::Project => unreachable!("handled above"),
            Level::Feature => "SELECT id AS feature_id FROM features WHERE id = $1",
            Level::Stage => "SELECT feature_id FROM stages WHERE id = $1",
            Level::Module => {
                "SELECT s.feature_id FROM modules m JOIN stages s ON s.id = m.stage_id WHERE m.id = $1"
            }
            Level::Task => {
                "SELECT s.feature_id FROM tasks t JOIN modules m ON m.id = t.module_id
                 JOIN stages s ON s.id = m.stage_id WHERE t.id = $1"
            }
        };
        let row = sqlx::query(sql).bind(id).fetch_optional(&self.pool).await?;
        Ok(row.map(|r| r.get("feature_id")))
    }

    /// Resolve the anchor + direction into per-level subject id sets.
    async fn subject_sets(&self, scope: &MemoryScope, dir: SearchDirection) -> Result<SubjectSets> {
        let mut sets = SubjectSets::default();
        // The anchor's ancestor chain (feature, stage?, module?, task?).
        // A project anchor has no ancestor chain: it IS the top. `here`
        // and `up` both mean the project's own shelf; `down` means
        // everything, which the caller handles by leaving the sets
        // empty and unscoping the query.
        if scope.level == Level::Project {
            sets.project = true;
            return Ok(sets);
        }
        let (feature, stage, module, task): (Option<String>, Option<String>, Option<String>, Option<String>) =
            match scope.level {
                Level::Project => unreachable!("handled above"),
                Level::Feature => (Some(scope.id.clone()), None, None, None),
                Level::Stage => {
                    let r = sqlx::query("SELECT feature_id FROM stages WHERE id = $1")
                        .bind(&scope.id)
                        .fetch_optional(&self.pool)
                        .await?
                        .ok_or_else(|| McpmError::not_found("stage", &scope.id))?;
                    (Some(r.get("feature_id")), Some(scope.id.clone()), None, None)
                }
                Level::Module => {
                    let r = sqlx::query(
                        "SELECT s.id AS stage_id, s.feature_id
                         FROM modules m JOIN stages s ON s.id = m.stage_id WHERE m.id = $1",
                    )
                    .bind(&scope.id)
                    .fetch_optional(&self.pool)
                    .await?
                    .ok_or_else(|| McpmError::not_found("module", &scope.id))?;
                    (
                        Some(r.get("feature_id")),
                        Some(r.get("stage_id")),
                        Some(scope.id.clone()),
                        None,
                    )
                }
                Level::Task => {
                    let r = sqlx::query(
                        "SELECT m.id AS module_id, s.id AS stage_id, s.feature_id
                         FROM tasks t JOIN modules m ON m.id = t.module_id
                         JOIN stages s ON s.id = m.stage_id WHERE t.id = $1",
                    )
                    .bind(&scope.id)
                    .fetch_optional(&self.pool)
                    .await?
                    .ok_or_else(|| McpmError::not_found("task", &scope.id))?;
                    (
                        Some(r.get("feature_id")),
                        Some(r.get("stage_id")),
                        Some(r.get("module_id")),
                        Some(scope.id.clone()),
                    )
                }
            };

        // `here`: the anchor only. `up`: anchor + ancestors.
        match scope.level {
            Level::Project => unreachable!("handled above"),
            Level::Feature => sets.features.extend(feature.clone()),
            Level::Stage => sets.stages.extend(stage.clone()),
            Level::Module => sets.modules.extend(module.clone()),
            Level::Task => sets.tasks.extend(task.clone()),
        }
        if dir == SearchDirection::Up {
            sets.features.extend(feature.clone());
            sets.stages.extend(stage.clone());
            sets.modules.extend(module.clone());
            sets.tasks.extend(task.clone());
            // The chain now terminates at the project, so a worker
            // reading upward from its module reaches the standing
            // conventions — which is the whole reason project scope
            // exists. Leaving it out would make `up` mean "everything
            // above me except the part that always applies".
            sets.project = true;
            sets.dedup();
        }
        // `down`: anchor + all descendants.
        if dir == SearchDirection::Down {
            match scope.level {
                Level::Project => unreachable!("handled above"),
                Level::Feature => {
                    let rows = sqlx::query(
                        "SELECT s.id AS stage_id, m.id AS module_id, t.id AS task_id
                         FROM stages s
                         LEFT JOIN modules m ON m.stage_id = s.id
                         LEFT JOIN tasks t ON t.module_id = m.id
                         WHERE s.feature_id = $1",
                    )
                    .bind(&scope.id)
                    .fetch_all(&self.pool)
                    .await?;
                    for r in rows {
                        sets.stages.push(r.get("stage_id"));
                        if let Some(m) = r.get::<Option<String>, _>("module_id") {
                            sets.modules.push(m);
                        }
                        if let Some(t) = r.get::<Option<String>, _>("task_id") {
                            sets.tasks.push(t);
                        }
                    }
                }
                Level::Stage => {
                    let rows = sqlx::query(
                        "SELECT m.id AS module_id, t.id AS task_id
                         FROM modules m LEFT JOIN tasks t ON t.module_id = m.id
                         WHERE m.stage_id = $1",
                    )
                    .bind(&scope.id)
                    .fetch_all(&self.pool)
                    .await?;
                    for r in rows {
                        sets.modules.push(r.get("module_id"));
                        if let Some(t) = r.get::<Option<String>, _>("task_id") {
                            sets.tasks.push(t);
                        }
                    }
                }
                Level::Module => {
                    let rows = sqlx::query("SELECT id FROM tasks WHERE module_id = $1")
                        .bind(&scope.id)
                        .fetch_all(&self.pool)
                        .await?;
                    for r in rows {
                        sets.tasks.push(r.get("id"));
                    }
                }
                Level::Task => {}
            }
            sets.dedup();
        }
        Ok(sets)
    }

    // -----------------------------------------------------------------
    // API keys
    // -----------------------------------------------------------------
    //
    // The credential is an identity, not just a gate: `verify_key`
    // returns the agent name and role the key was issued for, and every
    // caller attributes its writes to THAT rather than to anything the
    // agent sent. Which is why verification lives here with the other
    // invariants — the check and the identity it produces must not be
    // separable by a caller.

    /// Mint a key for `agent_name` at `role`. The returned
    /// [`IssuedKey::token`] is the only time the secret exists outside
    /// the bearer's hands: only its hash is written.
    pub async fn issue_key(
        &self,
        issuer: &str,
        label: &str,
        agent_name: &str,
        role: KeyRole,
    ) -> Result<IssuedKey> {
        let label = label.trim();
        let agent_name = agent_name.trim();
        if agent_name.is_empty() {
            return Err(McpmError::new(
                ErrorCode::PlanInvalid,
                "A key must name the agent it speaks for.",
                json!({}),
                "Pass --agent <name>. That name is what the ledger records for every write \
                 this key makes, so make it the one you want to read back.",
            ));
        }
        self.insert_key(issuer, label, agent_name, role, "cli").await
    }

    /// Issue a WORKER key to a manager, so a box that is not this
    /// machine can hold an identity of its own.
    ///
    /// This is the other half of the fleet story, and it is deliberately
    /// not delegation. A delegation token is scoped, expiring, and
    /// paired to one key precisely because it travels in band as plain
    /// text in a prompt; stretching it to cover boxes that already have
    /// their own credentials would make every box in a fleet share one
    /// identity, which is the exact collapse `mint_worker` exists to
    /// undo one level down. A box gets a key, and then delegation goes
    /// back to meaning what it means.
    ///
    /// Three things bound it, because it mints a STANDING credential
    /// where every other tool mints scoped ones:
    ///
    /// - **Worker only.** The role is not an argument. A manager that
    ///   could issue manager keys could escalate a subagent past the
    ///   gate this whole system is; a console key could not present the
    ///   result anyway.
    /// - **One live key per agent name.** Two live keys naming the same
    ///   agent are two boxes the ledger cannot tell apart and that can
    ///   complete each other's modules — the failure this exists to
    ///   fix. Rotation is revoke-then-issue, which is one call more and
    ///   leaves the withdrawal on the record.
    /// - **A cap.** A dispatch loop that issues a key per attempt would
    ///   otherwise fill `api_keys` with live credentials nobody is
    ///   holding, and nothing about that raises an error at the time.
    ///
    /// The operator's `--issue-key` is under none of these: it is a
    /// human at a shell, and it is how the first manager key exists at
    /// all.
    pub async fn issue_worker_key(
        &self,
        issuer: &Actor,
        agent_name: &str,
        label: Option<&str>,
    ) -> Result<IssuedKey> {
        if issuer.is_delegated() {
            return Err(McpmError::new(
                ErrorCode::Forbidden,
                "A delegated identity cannot issue a key.",
                json!({ "agent": issuer.name, "scope": issuer.scope }),
                "You were minted for one module and hold no authority to create \
                 credentials. If the work needs another box, report that to the manager \
                 that minted you.",
            ));
        }
        let agent_name = agent_name.trim();
        if agent_name.is_empty() {
            return Err(plan_invalid(
                "issue_worker_key requires the agent_name the box will be recorded as — \
                 that name is the whole point of giving it a key of its own. Use something \
                 that identifies the box, e.g. the branch slug it runs.",
            ));
        }
        if let Some(existing) = sqlx::query(
            "SELECT id FROM api_keys
             WHERE agent_name = $1 AND revoked_at IS NULL AND role = 'worker'",
        )
        .bind(agent_name)
        .fetch_optional(&self.pool)
        .await?
        {
            let id: String = existing.get("id");
            return Err(McpmError::new(
                ErrorCode::PlanConflict,
                format!("A live worker key already names agent '{agent_name}'."),
                json!({ "agent_name": agent_name, "key_id": id }),
                "Two live keys under one name are two boxes the ledger cannot tell apart, \
                 which is the problem a key per box exists to fix. If that box is still \
                 running, use the key it has. If it lost the key, ask the operator to \
                 revoke this one first. If this is a different box, give it a different \
                 name.",
            ));
        }
        let live: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM api_keys WHERE role = 'worker' AND revoked_at IS NULL",
        )
        .fetch_one(&self.pool)
        .await?;
        if live >= MAX_LIVE_WORKER_KEYS {
            return Err(McpmError::new(
                ErrorCode::Forbidden,
                format!("This deployment already holds {live} live worker keys."),
                json!({ "live_worker_keys": live, "cap": MAX_LIVE_WORKER_KEYS }),
                "A key outlives the box it was issued to unless somebody revokes it, so \
                 this cap is what stops a dispatch loop leaving a pile of live \
                 credentials nobody holds. Reuse the key a box already has, or ask the \
                 operator to revoke the keys of boxes that are gone.",
            ));
        }
        self.insert_key(
            &issuer.name,
            label.unwrap_or("").trim(),
            agent_name,
            KeyRole::Worker,
            "issue_worker_key",
        )
        .await
    }

    /// The insert both issuance paths share, so a key issued by an
    /// agent and one issued at a shell are the same row and the same
    /// ledger entry — differing only in `via`, which is what tells an
    /// audit which door a credential came through.
    async fn insert_key(
        &self,
        issuer: &str,
        label: &str,
        agent_name: &str,
        role: KeyRole,
        via: &str,
    ) -> Result<IssuedKey> {
        let label = if label.is_empty() { agent_name } else { label };
        let (token, id, hash) = crate::keys::mint();

        let mut tx = self.pool.begin().await?;
        sqlx::query(
            "INSERT INTO api_keys (id, hash, label, agent_name, role, created_by)
             VALUES ($1, $2, $3, $4, $5, $6)",
        )
        .bind(&id)
        .bind(&hash)
        .bind(label)
        .bind(agent_name)
        .bind(role.as_str())
        .bind(issuer)
        .execute(&mut *tx)
        .await?;
        // The ledger records that a credential now exists and who for.
        // Never the token, and never the hash — an event feed is read by
        // everything with console access.
        record_event(
            &mut tx,
            "key_issued",
            None,
            None,
            Some(issuer),
            json!({
                "key_id": id,
                "label": label,
                "agent": agent_name,
                "role": role.as_str(),
                "via": via,
            }),
        )
        .await?;
        tx.commit().await?;

        let info = self
            .key_info(&id)
            .await?
            .ok_or_else(|| McpmError::internal("issued key vanished before it could be read"))?;
        Ok(IssuedKey { token, info })
    }

    /// Resolve a presented token to the identity it carries, or reject
    /// it. Every failure — malformed, unknown, revoked — returns the
    /// same [`McpmError::unauthorized`]: a caller probing the endpoint
    /// must not learn which of its guesses was closer.
    pub async fn verify_key(&self, token: &str) -> Result<KeyIdentity> {
        // Shape first: junk never reaches the database, so an
        // unauthenticated flood costs no connections from a 4-slot pool.
        let parsed = crate::keys::parse(token).ok_or_else(McpmError::unauthorized)?;
        let row = sqlx::query(
            "SELECT hash, label, agent_name, role FROM api_keys
             WHERE id = $1 AND revoked_at IS NULL",
        )
        .bind(&parsed.id)
        .fetch_optional(&self.pool)
        .await?
        .ok_or_else(McpmError::unauthorized)?;

        let stored: String = row.get("hash");
        if !crate::keys::hash_eq(&stored, &parsed.secret_hash) {
            return Err(McpmError::unauthorized());
        }
        let role = KeyRole::parse(row.get::<String, _>("role").as_str())
            .ok_or_else(|| McpmError::internal("api_keys.role holds an unknown role"))?;

        // Best-effort liveness stamp. A failure here must not fail an
        // otherwise good request — the key is already verified, and the
        // column is an operator convenience, not an invariant.
        let _ = sqlx::query("UPDATE api_keys SET last_used = now() WHERE id = $1")
            .bind(&parsed.id)
            .execute(&self.pool)
            .await;

        Ok(KeyIdentity {
            key_id: parsed.id,
            label: row.get("label"),
            agent_name: row.get("agent_name"),
            role,
        })
    }

    // -----------------------------------------------------------------
    // Delegated identities
    // -----------------------------------------------------------------

    /// Mint one worker identity, scoped to one module.
    ///
    /// This exists because a key is per MACHINE and a subagent cannot
    /// present a different one — so without it every subagent in a
    /// process tree IS that machine, and attribution, mutual exclusion
    /// between siblings, and the manager/worker split all collapse
    /// inside the tree while still holding between trees.
    ///
    /// Two invariants are enforced here and the rest by the columns and
    /// by `require_claim`:
    ///
    /// - **Delegation is one level deep.** A delegated actor cannot
    ///   mint. The role gate says the same thing (a delegated caller is
    ///   gated as a worker, and `mint_worker` is manager-only), but it
    ///   is repeated here because the gate binds only where a role was
    ///   proven, and an unauthenticated stdio session proves nothing.
    /// - **One live token per module.** Minting again for the same
    ///   module retires the previous token in the same transaction, so
    ///   a manager re-dispatching a module cannot leave a second
    ///   identity able to write to it.
    ///
    /// `req.for_key_id` moves where the token's boundary sits without
    /// weakening its shape — see [`MintRequest::for_key_id`]. It is
    /// checked against `api_keys` HERE rather than left to the foreign
    /// key, because a manager naming a key that is revoked, absent, or
    /// a console key has made a dispatch error it needs to read as one:
    /// the foreign key would either accept it (revoked, console) or
    /// surface as an opaque constraint violation, and both end with a
    /// box that cannot work and a manager that thinks it dispatched.
    pub async fn mint_worker(
        &self,
        minter: &Actor,
        key_id: Option<&str>,
        req: MintRequest<'_>,
    ) -> Result<MintedWorker> {
        let MintRequest { module_id, agent_name, ttl_minutes, for_key_id } = req;
        if minter.is_delegated() {
            return Err(McpmError::new(
                ErrorCode::Forbidden,
                "A delegated identity cannot mint another one.",
                json!({ "agent": minter.name, "scope": minter.scope }),
                "Delegation is one level deep so the tree stays legible. If the work needs \
                 splitting further, report that to the manager that minted you.",
            ));
        }
        let agent_name = agent_name.trim();
        if agent_name.is_empty() {
            return Err(plan_invalid(
                "mint_worker requires the agent_name the subagent will be recorded as — that \
                 name is the whole point of minting.",
            ));
        }
        let ttl = ttl_minutes
            .unwrap_or(DEFAULT_DELEGATION_TTL_MINUTES)
            .clamp(MIN_DELEGATION_TTL_MINUTES, MAX_DELEGATION_TTL_MINUTES);
        let expires_at = chrono::Utc::now() + chrono::Duration::minutes(ttl);

        let mut tx = self.pool.begin().await?;
        // Which credential this token will answer to. Exactly one,
        // always — the default is the minter's own. Checked inside the
        // transaction that writes the row, like every other rule here:
        // a key revoked between a check and an insert would otherwise
        // produce a token that is born dead.
        let bound_key_id = match for_key_id {
            Some(target) => Some(verify_mint_target(&mut tx, target).await?),
            None => key_id.map(str::to_string),
        };
        let row = sqlx::query(
            "SELECT m.id, m.name AS module_name, m.status, s.feature_id
             FROM modules m JOIN stages s ON s.id = m.stage_id
             WHERE m.id = $1 FOR UPDATE OF m",
        )
        .bind(module_id)
        .fetch_optional(&mut *tx)
        .await?
        .ok_or_else(|| McpmError::not_found("module", module_id))?;
        let module_name: String = row.get("module_name");
        let feature_id: String = row.get("feature_id");
        let status: String = row.get("status");
        if status == "done" {
            return Err(McpmError::new(
                ErrorCode::AlreadyDone,
                format!("Module '{module_name}' is already complete."),
                json!({ "module_id": module_id }),
                "There is no work to delegate. Dispatch what next_work returns instead.",
            ));
        }

        let superseded = retire_delegations(&mut tx, module_id).await?;
        let (token, id, hash) = crate::keys::mint_delegation();
        sqlx::query(
            "INSERT INTO delegations
                 (id, hash, key_id, agent_name, module_id, minted_by, expires_at)
             VALUES ($1, $2, $3, $4, $5, $6, $7)",
        )
        .bind(&id)
        .bind(&hash)
        .bind(bound_key_id.as_deref())
        .bind(agent_name)
        .bind(module_id)
        .bind(&minter.name)
        .bind(expires_at)
        .execute(&mut *tx)
        .await?;
        // The ledger records that an identity now exists and who for.
        // Never the token: an event feed is read by everything with
        // console access, and this one is meant to be pasted into a
        // prompt, not stored.
        record_event(
            &mut tx,
            "worker_minted",
            Some(&feature_id),
            Some(module_id),
            Some(&minter.name),
            json!({
                "agent_name": agent_name,
                "module": module_name,
                "expires_at": expires_at,
                "superseded": superseded,
                // WHO was allowed to present this, on the record. The
                // token itself is never here, but "which credential
                // could have used it" is exactly the question an audit
                // asks afterwards, and it cannot be reconstructed from
                // the minter — `for_key_id` may point anywhere.
                "bound_key_id": bound_key_id,
                "delegated_off_machine": for_key_id.is_some(),
            }),
        )
        .await?;
        tx.commit().await?;

        let where_it_works = match (&bound_key_id, for_key_id) {
            (Some(id), Some(_)) => format!(
                " It is bound to key '{id}', NOT to this machine — it works only where \
                 that key is installed, and is inert anywhere else including here."
            ),
            _ => String::new(),
        };
        Ok(MintedWorker {
            instructions: format!(
                "Give this token to the subagent in its prompt and tell it: you are \
                 '{agent_name}'; pass delegation_token on get_context and on every write \
                 (claim_module, complete_task, add_task, complete_module, report_blocker, \
                 release_module). It is scoped to {module_id} alone and stops working when \
                 that module completes or is released, or at {expires_at}.{where_it_works}"
            ),
            delegation_token: token,
            agent_name: agent_name.to_string(),
            module_id: module_id.to_string(),
            expires_at,
            bound_key_id,
        })
    }


    /// Resolve a delegation token to the identity it carries.
    ///
    /// `key_id` is the key the request authenticated with, and the match
    /// is on equality INCLUDING null: a token bound to a key resolves
    /// only alongside that same key, and one minted on a keyless stdio
    /// session resolves only on a keyless session. That pairing is what
    /// lets the token travel in band — off the machine it was bound to,
    /// it is inert.
    ///
    /// The row is fetched by id ALONE and the four failures are then
    /// told apart in order, because they want four different reactions
    /// from the subagent holding the token. The order is the whole
    /// safety argument: the secret is verified FIRST, so nothing below
    /// it — not the expiry, not the key it is bound to — is observable
    /// without already holding the token. A guesser only ever reaches
    /// `delegation_unknown`.
    pub async fn resolve_delegation(
        &self,
        key_id: Option<&str>,
        token: &str,
    ) -> Result<Delegation> {
        // Shape first, so junk never reaches the database.
        let parsed =
            crate::keys::parse_delegation(token).ok_or_else(McpmError::delegation_unknown)?;
        let row = sqlx::query(
            "SELECT hash, agent_name, module_id, expires_at, revoked_at, key_id,
                    expires_at <= now() AS expired
             FROM delegations WHERE id = $1",
        )
        .bind(&parsed.id)
        .fetch_optional(&self.pool)
        .await?
        .ok_or_else(McpmError::delegation_unknown)?;

        // Gate one, and it is a gate: everything after this line is a
        // fact about a token the caller has proven it holds.
        let stored: String = row.get("hash");
        if !crate::keys::hash_eq(&stored, &parsed.secret_hash) {
            return Err(McpmError::delegation_unknown());
        }
        let expires_at: chrono::DateTime<chrono::Utc> = row.get("expires_at");
        if row.get::<Option<chrono::DateTime<chrono::Utc>>, _>("revoked_at").is_some() {
            return Err(McpmError::delegation_retired());
        }
        if row.get::<Option<bool>, _>("expired").unwrap_or(true) {
            return Err(McpmError::delegation_expired(expires_at));
        }
        let bound: Option<String> = row.get("key_id");
        if bound.as_deref() != key_id {
            return Err(McpmError::delegation_wrong_key(bound.as_deref(), key_id));
        }
        Ok(Delegation {
            id: parsed.id,
            agent_name: row.get("agent_name"),
            module_id: row.get("module_id"),
            expires_at,
        })
    }

    /// Every key ever issued, newest first — revoked ones included, so
    /// the operator can see what was withdrawn and when.
    pub async fn list_keys(&self) -> Result<Vec<ApiKeyInfo>> {
        let rows = sqlx::query(
            "SELECT id, label, agent_name, role, created_at, created_by, last_used, revoked_at
             FROM api_keys ORDER BY created_at DESC",
        )
        .fetch_all(&self.pool)
        .await?;
        rows.iter().map(map_key_row).collect()
    }

    /// Withdraw a key. Idempotent: revoking an already-revoked key is a
    /// no-op that still succeeds, because the caller's intent ("this
    /// key must not work") is satisfied either way.
    pub async fn revoke_key(&self, revoker: &str, key_id: &str) -> Result<ApiKeyInfo> {
        let mut tx = self.pool.begin().await?;
        let updated = sqlx::query(
            "UPDATE api_keys SET revoked_at = now() WHERE id = $1 AND revoked_at IS NULL",
        )
        .bind(key_id)
        .execute(&mut *tx)
        .await?;
        if updated.rows_affected() > 0 {
            record_event(
                &mut tx,
                "key_revoked",
                None,
                None,
                Some(revoker),
                json!({ "key_id": key_id }),
            )
            .await?;
        }
        tx.commit().await?;
        self.key_info(key_id)
            .await?
            .ok_or_else(|| McpmError::not_found("api key", key_id))
    }

    /// Whether this deployment has any live key at all. The HTTP
    /// listener refuses to start without one — a server that would
    /// admit nobody is a misconfiguration worth failing loudly on,
    /// rather than a silent wall every agent bounces off.
    pub async fn live_key_count(&self) -> Result<i64> {
        Ok(
            sqlx::query_scalar("SELECT COUNT(*) FROM api_keys WHERE revoked_at IS NULL")
                .fetch_one(&self.pool)
                .await?,
        )
    }

    async fn key_info(&self, key_id: &str) -> Result<Option<ApiKeyInfo>> {
        let row = sqlx::query(
            "SELECT id, label, agent_name, role, created_at, created_by, last_used, revoked_at
             FROM api_keys WHERE id = $1",
        )
        .bind(key_id)
        .fetch_optional(&self.pool)
        .await?;
        row.as_ref().map(map_key_row).transpose()
    }
}

/// Which subjects a scoped search matches, resolved from the anchor +
/// direction. `project` is a flag rather than an id list because the
/// project shelf is a singleton.
#[derive(Default)]
struct SubjectSets {
    project: bool,
    features: Vec<String>,
    stages: Vec<String>,
    modules: Vec<String>,
    tasks: Vec<String>,
}

impl SubjectSets {
    fn dedup(&mut self) {
        for set in [&mut self.features, &mut self.stages, &mut self.modules, &mut self.tasks] {
            set.sort();
            set.dedup();
        }
    }
}

// ---------------------------------------------------------------------
// Free helpers
// ---------------------------------------------------------------------

async fn record_event(
    tx: &mut Tx<'_>,
    kind: &str,
    feature_id: Option<&str>,
    subject_id: Option<&str>,
    agent: Option<&str>,
    payload: serde_json::Value,
) -> std::result::Result<(), McpmError> {
    sqlx::query(
        "INSERT INTO events (type, feature_id, subject_id, agent, payload)
         VALUES ($1, $2, $3, $4, $5)",
    )
    .bind(kind)
    .bind(feature_id)
    .bind(subject_id)
    .bind(agent)
    .bind(payload)
    .execute(&mut **tx)
    .await?;
    Ok(())
}

async fn insert_memory(
    tx: &mut Tx<'_>,
    level: Level,
    subject_id: &str,
    kind: MemoryKind,
    content: &str,
    tags: &[&str],
    author: &str,
) -> std::result::Result<String, McpmError> {
    let id = new_memory_id();
    let tags: Vec<String> = tags.iter().map(|t| t.to_string()).collect();
    sqlx::query(
        "INSERT INTO memories (id, level, subject_id, kind, content, tags, author)
         VALUES ($1, $2, $3, $4, $5, $6, $7)",
    )
    .bind(&id)
    .bind(level.as_str())
    .bind(subject_id)
    .bind(kind.as_str())
    .bind(content)
    .bind(&tags)
    .bind(author)
    .execute(&mut **tx)
    .await?;
    Ok(id)
}

/// Writes to a module's checklist require holding its claim — and, for
/// a delegated identity, require the module to be the one it was minted
/// for.
///
/// Both checks live here because both answer the same question ("may
/// this actor write to this module?") and every write verb already
/// funnels through it with the module row in hand. Splitting them would
/// mean a new verb could pick up one and miss the other.
///
/// The row must carry `module_id`, `module_name` and `claimed_by`.
fn require_claim(
    row: &sqlx::postgres::PgRow,
    actor: &Actor,
    action: &str,
) -> std::result::Result<(), McpmError> {
    let module_id: String = row.get("module_id");
    require_scope(actor, &module_id)?;
    let agent: &str = &actor.name;
    let claimed_by: Option<String> = row.get("claimed_by");
    let module_name: String = row.get("module_name");
    match claimed_by.as_deref() {
        Some(holder) if holder == agent => Ok(()),
        Some(holder) => Err(McpmError::new(
            ErrorCode::NotClaimedByYou,
            format!("Module '{module_name}' is claimed by '{holder}', not you — you cannot {action}."),
            json!({ "claimed_by": holder }),
            "Only the claim holder writes to a module. If you are taking over, have the \
             holder release_module first (or your manager arrange it).",
        )),
        None => Err(McpmError::new(
            ErrorCode::NotClaimedByYou,
            format!("Module '{module_name}' is unclaimed — claim it before you {action}."),
            serde_json::Value::Null,
            "Call claim_module first; the claim is also your briefing.",
        )),
    }
}

/// Check a `for_key_id` before a token is bound to it, and hand the id
/// back so the caller cannot accidentally bind the unchecked one.
///
/// Revoked and console keys are refused rather than accepted-and-
/// useless: a token bound to either resolves forever to
/// `delegation_wrong_key` on the box holding it, which reads as an
/// operator error on the WORKER's side and sends it chasing a
/// mistake its manager made minutes earlier. Takes the transaction
/// rather than the pool, so the key it approves is the key the row is
/// written against.
async fn verify_mint_target(tx: &mut Tx<'_>, target: &str) -> Result<String> {
    let row = sqlx::query("SELECT role, revoked_at FROM api_keys WHERE id = $1 FOR SHARE")
        .bind(target)
        .fetch_optional(&mut **tx)
        .await?
        .ok_or_else(|| {
            McpmError::new(
                ErrorCode::NotFound,
                format!("No API key with id '{target}'."),
                json!({ "for_key_id": target }),
                "for_key_id is a key's PUBLIC half (the `mcpm_<id>_…` middle), not the \
                 whole token and not an agent name. Issue the box a key with \
                 issue_worker_key and bind to the key_id it returns.",
            )
        })?;
    if row.get::<Option<chrono::DateTime<chrono::Utc>>, _>("revoked_at").is_some() {
        return Err(McpmError::new(
            ErrorCode::Forbidden,
            format!("API key '{target}' has been revoked."),
            json!({ "for_key_id": target }),
            "Nothing can present it, so a token bound to it would be born dead. Issue \
             the box a fresh key and bind to that.",
        ));
    }
    let role = KeyRole::parse(row.get::<String, _>("role").as_str())
        .ok_or_else(|| McpmError::internal("api_keys.role holds an unknown role"))?;
    if !role.is_agent() {
        return Err(McpmError::new(
            ErrorCode::Forbidden,
            format!("API key '{target}' is a {} key.", role.as_str()),
            json!({ "for_key_id": target, "role": role.as_str() }),
            "A console key is refused by the MCP surface outright, so it could never \
             present the token. Bind to the agent key the box actually runs with.",
        ));
    }
    Ok(target.to_string())
}

/// Retire every live delegation on a module. Called inside the
/// transaction that ends the module's life as claimed work — completion
/// or release — never from a caller, so there is no path that finishes a
/// module and leaves its token usable.
async fn retire_delegations(tx: &mut Tx<'_>, module_id: &str) -> Result<u64> {
    let done = sqlx::query(
        "UPDATE delegations SET revoked_at = now()
         WHERE module_id = $1 AND revoked_at IS NULL",
    )
    .bind(module_id)
    .execute(&mut **tx)
    .await?;
    Ok(done.rows_affected())
}

/// A delegated actor may touch exactly the module it was minted for.
/// An undelegated one — a key speaking for itself — is unconstrained
/// here and answers to `require_claim` alone.
fn require_scope(actor: &Actor, module_id: &str) -> std::result::Result<(), McpmError> {
    match actor.scope.as_deref() {
        Some(scope) if scope != module_id => Err(McpmError::out_of_scope(module_id, scope)),
        _ => Ok(()),
    }
}

fn plan_invalid(message: impl Into<String>) -> McpmError {
    McpmError::new(
        ErrorCode::PlanInvalid,
        message,
        serde_json::Value::Null,
        "Fix the plan and resend — nothing was written.",
    )
}

fn plan_conflict(message: impl Into<String>) -> McpmError {
    McpmError::new(
        ErrorCode::PlanConflict,
        message,
        serde_json::Value::Null,
        "Plan forward, not backward: completed or claimed work is history. Add new \
         stages/modules/tasks instead of rewriting finished ones.",
    )
}

fn unique_to_plan_invalid(what: &str) -> impl Fn(sqlx::Error) -> McpmError + '_ {
    move |err: sqlx::Error| match &err {
        sqlx::Error::Database(db) if db.is_unique_violation() => plan_invalid(format!(
            "Name collision: {what}. Names must be unique within their parent."
        )),
        _ => McpmError::internal(err),
    }
}

async fn apply_plan_op(
    tx: &mut Tx<'_>,
    agent: &str,
    feature_id: &str,
    op: PlanOp,
) -> std::result::Result<String, McpmError> {
    match op {
        PlanOp::AddStage { name, after } => {
            let position: i32 = match &after {
                None => sqlx::query("SELECT COALESCE(MAX(position), 0) + 1 AS p FROM stages WHERE feature_id = $1")
                    .bind(feature_id)
                    .fetch_one(&mut **tx)
                    .await?
                    .get("p"),
                Some(after_id) => {
                    let row = sqlx::query("SELECT position FROM stages WHERE id = $1 AND feature_id = $2")
                        .bind(after_id)
                        .bind(feature_id)
                        .fetch_optional(&mut **tx)
                        .await?
                        .ok_or_else(|| McpmError::not_found("stage", after_id))?;
                    let after_pos: i32 = row.get("position");
                    sqlx::query("UPDATE stages SET position = position + 1 WHERE feature_id = $1 AND position > $2")
                        .bind(feature_id)
                        .bind(after_pos)
                        .execute(&mut **tx)
                        .await?;
                    after_pos + 1
                }
            };
            let id = new_id(Level::Stage);
            sqlx::query("INSERT INTO stages (id, feature_id, name, position) VALUES ($1, $2, $3, $4)")
                .bind(&id)
                .bind(feature_id)
                .bind(&name)
                .bind(position)
                .execute(&mut **tx)
                .await
                .map_err(unique_to_plan_invalid("duplicate stage name in this feature"))?;
            Ok(format!("add_stage '{name}' ({id}) at position {position}"))
        }
        PlanOp::AddModule { stage_id, name, description, tasks } => {
            let stage = sqlx::query("SELECT position FROM stages WHERE id = $1 AND feature_id = $2")
                .bind(&stage_id)
                .bind(feature_id)
                .fetch_optional(&mut **tx)
                .await?
                .ok_or_else(|| McpmError::not_found("stage", &stage_id))?;
            let position: i32 = stage.get("position");
            // Adding to a completed stage would retroactively re-lock
            // later stages that already started — that rewrites history.
            let stage_done: i64 = sqlx::query(
                "SELECT COUNT(*) AS n FROM modules WHERE stage_id = $1 AND status <> 'done'",
            )
            .bind(&stage_id)
            .fetch_one(&mut **tx)
            .await?
            .get("n");
            if stage_done == 0 {
                let later_started: i64 = sqlx::query(
                    "SELECT COUNT(*) AS n FROM modules m JOIN stages s ON s.id = m.stage_id
                     WHERE s.feature_id = $1 AND s.position > $2 AND m.status <> 'todo'",
                )
                .bind(feature_id)
                .bind(position)
                .fetch_one(&mut **tx)
                .await?
                .get("n");
                if later_started > 0 {
                    return Err(plan_conflict(format!(
                        "Stage {position} is complete and later stages have already started — \
                         adding a module there would retroactively close an open gate."
                    )));
                }
            }
            let id = new_id(Level::Module);
            sqlx::query("INSERT INTO modules (id, stage_id, name, description) VALUES ($1, $2, $3, $4)")
                .bind(&id)
                .bind(&stage_id)
                .bind(&name)
                .bind(&description)
                .execute(&mut **tx)
                .await
                .map_err(unique_to_plan_invalid("duplicate module name in one stage"))?;
            for (ti, task) in tasks.iter().enumerate() {
                sqlx::query(
                    "INSERT INTO tasks (id, module_id, name, origin, position, created_by)
                     VALUES ($1, $2, $3, 'planned', $4, $5)",
                )
                .bind(new_id(Level::Task))
                .bind(&id)
                .bind(task)
                .bind(ti as i32)
                .bind(agent)
                .execute(&mut **tx)
                .await?;
            }
            Ok(format!("add_module '{name}' ({id}) to stage {position}"))
        }
        PlanOp::AddTask { module_id, name, note } => {
            let module = sqlx::query(
                "SELECT m.status FROM modules m JOIN stages s ON s.id = m.stage_id
                 WHERE m.id = $1 AND s.feature_id = $2",
            )
            .bind(&module_id)
            .bind(feature_id)
            .fetch_optional(&mut **tx)
            .await?
            .ok_or_else(|| McpmError::not_found("module", &module_id))?;
            if module.get::<String, _>("status") == "done" {
                return Err(plan_conflict(
                    "The module is complete; its checklist is closed. Add the work as a new \
                     module instead.",
                ));
            }
            let id = new_id(Level::Task);
            sqlx::query(
                "INSERT INTO tasks (id, module_id, name, origin, note, position, created_by)
                 VALUES ($1, $2, $3, 'planned', $4,
                         (SELECT COALESCE(MAX(position), -1) + 1 FROM tasks WHERE module_id = $2), $5)",
            )
            .bind(&id)
            .bind(&module_id)
            .bind(&name)
            .bind(&note)
            .bind(agent)
            .execute(&mut **tx)
            .await?;
            Ok(format!("add_task '{name}' ({id}) to {module_id}"))
        }
        PlanOp::Rename { id, name } => {
            let level = id_level(&id)
                .ok_or_else(|| plan_invalid(format!("'{id}' is not a recognizable tree id.")))?;
            let (table, guard) = match level {
                Level::Project => {
                    return Err(plan_invalid(
                        "The project is not part of a feature's plan — revise_plan cannot \
                         rename it.",
                    ))
                }
                Level::Feature => ("features", "id = $2"),
                Level::Stage => ("stages", "id = $2 AND feature_id = $3"),
                Level::Module => ("modules", "id = $2"),
                Level::Task => ("tasks", "id = $2"),
            };
            let sql = format!("UPDATE {table} SET name = $1 WHERE {guard}");
            let mut q = sqlx::query(&sql).bind(&name).bind(&id);
            if level == Level::Stage {
                q = q.bind(feature_id);
            }
            let n = q
                .execute(&mut **tx)
                .await
                .map_err(unique_to_plan_invalid("that name is already taken here"))?
                .rows_affected();
            if n == 0 {
                return Err(McpmError::not_found(level.as_str(), &id));
            }
            Ok(format!("rename {id} -> '{name}'"))
        }
        PlanOp::Remove { id } => {
            let level = id_level(&id)
                .ok_or_else(|| plan_invalid(format!("'{id}' is not a recognizable tree id.")))?;
            match level {
                Level::Project => Err(plan_invalid(
                    "The project is not part of a feature's plan — revise_plan cannot remove it.",
                )),
                Level::Feature => Err(plan_invalid(
                    "revise_plan cannot remove the feature itself — shelve it instead.",
                )),
                Level::Stage => {
                    let touched: i64 = sqlx::query(
                        "SELECT COUNT(*) AS n FROM modules m
                         WHERE m.stage_id = $1 AND m.status <> 'todo'",
                    )
                    .bind(&id)
                    .fetch_one(&mut **tx)
                    .await?
                    .get("n");
                    if touched > 0 {
                        return Err(plan_conflict(
                            "The stage holds claimed or completed modules — that work is history.",
                        ));
                    }
                    let n = sqlx::query("DELETE FROM stages WHERE id = $1 AND feature_id = $2")
                        .bind(&id)
                        .bind(feature_id)
                        .execute(&mut **tx)
                        .await?
                        .rows_affected();
                    if n == 0 {
                        return Err(McpmError::not_found("stage", &id));
                    }
                    Ok(format!("remove stage {id}"))
                }
                Level::Module => {
                    let row = sqlx::query("SELECT status FROM modules WHERE id = $1")
                        .bind(&id)
                        .fetch_optional(&mut **tx)
                        .await?
                        .ok_or_else(|| McpmError::not_found("module", &id))?;
                    if row.get::<String, _>("status") != "todo" {
                        return Err(plan_conflict(
                            "The module has been claimed, blocked, or completed — that work is \
                             history.",
                        ));
                    }
                    sqlx::query("DELETE FROM modules WHERE id = $1")
                        .bind(&id)
                        .execute(&mut **tx)
                        .await?;
                    Ok(format!("remove module {id}"))
                }
                Level::Task => {
                    let row = sqlx::query("SELECT status FROM tasks WHERE id = $1")
                        .bind(&id)
                        .fetch_optional(&mut **tx)
                        .await?
                        .ok_or_else(|| McpmError::not_found("task", &id))?;
                    if row.get::<String, _>("status") != "open" {
                        return Err(plan_conflict(
                            "The task is already resolved — resolved checklist items are history.",
                        ));
                    }
                    sqlx::query("DELETE FROM tasks WHERE id = $1")
                        .bind(&id)
                        .execute(&mut **tx)
                        .await?;
                    Ok(format!("remove task {id}"))
                }
            }
        }
    }
}

/// Plan shape rules, checked before anything is written.
fn validate_plan(plan: &PlanFeature) -> Result<()> {
    if plan.name.trim().is_empty() {
        return Err(plan_invalid("The feature needs a non-empty name."));
    }
    if plan.stages.is_empty() {
        return Err(plan_invalid("A feature needs at least one stage."));
    }
    for stage in &plan.stages {
        if stage.modules.is_empty() {
            return Err(plan_invalid(format!(
                "Stage '{}' has no modules — every stage needs at least one, because a \
                 stage with nothing to do can never complete.",
                stage.name
            )));
        }
    }
    Ok(())
}

/// Write a whole validated plan inside a caller-owned transaction and
/// record `feature_planned`. Shared by [`Store::plan_feature`] and
/// [`Store::promote_wants`], so composing a feature out of wants is as
/// atomic as planning one directly.
async fn insert_plan(tx: &mut Tx<'_>, agent: &str, plan: &PlanFeature) -> Result<String> {
    let feature_id = new_id(Level::Feature);
    sqlx::query(
        "INSERT INTO features (id, name, description, status, created_by)
         VALUES ($1, $2, $3, 'planning', $4)",
    )
    .bind(&feature_id)
    .bind(&plan.name)
    .bind(&plan.description)
    .bind(agent)
    .execute(&mut **tx)
    .await
    .map_err(unique_to_plan_invalid("a feature with that name already exists"))?;

    let (mut n_modules, mut n_tasks) = (0usize, 0usize);
    for (si, stage) in plan.stages.iter().enumerate() {
        let stage_id = new_id(Level::Stage);
        sqlx::query("INSERT INTO stages (id, feature_id, name, position) VALUES ($1, $2, $3, $4)")
            .bind(&stage_id)
            .bind(&feature_id)
            .bind(&stage.name)
            .bind((si + 1) as i32)
            .execute(&mut **tx)
            .await
            .map_err(unique_to_plan_invalid("duplicate stage name in this feature"))?;
        for module in &stage.modules {
            let module_id = new_id(Level::Module);
            sqlx::query(
                "INSERT INTO modules (id, stage_id, name, description) VALUES ($1, $2, $3, $4)",
            )
            .bind(&module_id)
            .bind(&stage_id)
            .bind(&module.name)
            .bind(&module.description)
            .execute(&mut **tx)
            .await
            .map_err(unique_to_plan_invalid("duplicate module name in one stage"))?;
            n_modules += 1;
            for (ti, task) in module.tasks.iter().enumerate() {
                sqlx::query(
                    "INSERT INTO tasks (id, module_id, name, origin, position, created_by)
                     VALUES ($1, $2, $3, 'planned', $4, $5)",
                )
                .bind(new_id(Level::Task))
                .bind(&module_id)
                .bind(task)
                .bind(ti as i32)
                .bind(agent)
                .execute(&mut **tx)
                .await?;
                n_tasks += 1;
            }
        }
    }

    record_event(
        tx,
        "feature_planned",
        Some(&feature_id),
        Some(&feature_id),
        Some(agent),
        json!({
            "name": plan.name,
            "stages": plan.stages.len(),
            "modules": n_modules,
            "tasks": n_tasks,
        }),
    )
    .await?;
    Ok(feature_id)
}

/// A want a feature already absorbed is frozen: refuse the edit and say
/// which feature owns it.
fn want_promoted(want: &Want, why: &str, hint: &str) -> McpmError {
    let features: Vec<String> = want
        .features
        .iter()
        .map(|l| format!("{} ({})", l.feature_name, l.feature_id))
        .collect();
    McpmError::new(
        ErrorCode::WantPromoted,
        format!("Want {} is already promoted into {}. {why}", want.id, features.join(", ")),
        serde_json::json!({ "want_id": want.id, "features": want.features.iter().map(|l| &l.feature_id).collect::<Vec<_>>() }),
        hint,
    )
}

/// Normalize a tag list and make sure every one of them exists in the
/// registry, returning the slugs the want should carry. Called inside
/// the same transaction as the write that uses them, so a tag can never
/// be referenced by a want the registry has not heard of.
async fn register_tags(tx: &mut Tx<'_>, agent: &str, raw: &[String]) -> Result<Vec<String>> {
    let mut out: Vec<String> = Vec::with_capacity(raw.len());
    for entry in raw {
        let Some(name) = normalize_tag(entry) else {
            continue;
        };
        if out.contains(&name) {
            continue;
        }
        let created = insert_tag(tx, agent, &name, entry.trim().trim_start_matches('#')).await?;
        if created {
            record_event(tx, "tag_created", None, None, Some(agent), json!({ "tag": name })).await?;
        }
        out.push(name);
    }
    Ok(out)
}

/// Insert one tag if the registry lacks it. Returns whether it was new,
/// so only a real creation reaches the ledger.
async fn insert_tag(tx: &mut Tx<'_>, agent: &str, name: &str, label: &str) -> Result<bool> {
    let label = if label.trim().is_empty() { name } else { label.trim() };
    let inserted = sqlx::query(
        "INSERT INTO tags (name, label, created_by) VALUES ($1, $2, $3)
         ON CONFLICT (name) DO NOTHING",
    )
    .bind(name)
    .bind(label)
    .bind(agent)
    .execute(&mut **tx)
    .await?;
    Ok(inserted.rows_affected() > 0)
}

/// The CTEs every memory read shares: the tunable weights, the
/// distinct-agent signal counts, and each entry's position within its
/// own kind.
///
/// One string because the ranking must be identical everywhere a memory
/// is read. Two copies of this that drifted would mean the console and
/// the agents were looking at differently-ordered versions of the same
/// base, which is exactly the bug nobody would notice.
const MEMORY_CTES: &str = "
    w AS (
        SELECT COALESCE(MAX(value) FILTER (WHERE name = 'signal_touch'),   0.15) AS w_touch,
               COALESCE(MAX(value) FILTER (WHERE name = 'signal_confirm'), 1.0)  AS w_confirm,
               COALESCE(MAX(value) FILTER (WHERE name = 'signal_dispute'), -2.0) AS w_dispute,
               COALESCE(MAX(value) FILTER (WHERE name = 'sigmoid_k'),      0.25) AS k,
               COALESCE(MAX(value) FILTER (WHERE name = 'decay_strength'), 0.5)  AS decay,
               COALESCE(MAX(value) FILTER (WHERE name = 'rank_floor'),     0.2)  AS floor,
               COALESCE(MAX(value) FILTER (WHERE name = 'penalty_superseded'), 0.6) AS pen_sup,
               COALESCE(MAX(value) FILTER (WHERE name = 'penalty_disputed'),   0.8) AS pen_dis,
               COALESCE(MAX(value) FILTER (WHERE name = 'penalty_machine'),   0.15) AS pen_mach
        FROM knowledge_weights
    ),
    sig AS (
        -- DISTINCT agent, not row: an agent touching in a loop is one
        -- agent's opinion however many times it fires.
        SELECT memory_id,
               COUNT(DISTINCT agent) FILTER (WHERE kind = 'touch')   AS touches,
               COUNT(DISTINCT agent) FILTER (WHERE kind = 'confirm') AS confirms,
               COUNT(DISTINCT agent) FILTER (WHERE kind = 'dispute') AS disputes
        FROM memory_signals GROUP BY memory_id
    ),
    pos AS (
        -- The decay clock: what fraction of same-kind memories were
        -- written AFTER this one. 0 for the newest, 1 for the oldest.
        --
        -- Corpus position rather than wall-clock, so a dormant project
        -- does not stale its own knowledge and a busy one does. Within
        -- KIND, because most of this table is machine-written outcomes
        -- (every module completion commits one) and counting globally
        -- would bury curated conventions under activity that could never
        -- have superseded them.
        --
        -- Self-normalizing, so it cannot run away as the base grows, and
        -- there is no per-insert constant to tune.
        SELECT id, PERCENT_RANK() OVER (PARTITION BY kind ORDER BY created_at DESC) AS newer
        FROM memories
    ),
    ed AS (
        -- Supersession kinds BY NAME. A standing relation (refines,
        -- depends_on, contradicts, relates_to) describes a memory; it
        -- must never retire one.
        SELECT to_id, COUNT(*) AS superseded_by FROM memory_edges
        WHERE kind IN ('replaces', 'refutes', 'revises', 'consolidates')
        GROUP BY to_id
    ),
    mach AS (
        -- Machine-written records: the completion summaries and blocker
        -- reports the store commits on the crew's behalf. Knowledge, but
        -- history rather than guidance.
        SELECT id FROM memories WHERE tags @> ARRAY['system']
    )";

/// The scored, state-annotated projection of one memory row. Assumes
/// [`MEMORY_CTES`] is in scope and the row is aliased `mem`.
const MEMORY_FIELDS: &str = "
    mem.id, mem.level, mem.subject_id, mem.kind, mem.content, mem.tags,
    mem.author, mem.created_at,
    COALESCE(p.name, f.name, s.name, m.name, t.name, mem.subject_id) AS subject_name,
    COALESCE(sig.touches, 0)  AS touches,
    COALESCE(sig.confirms, 0) AS confirms,
    COALESCE(sig.disputes, 0) AS disputes,
    pos.newer::real AS newer_fraction,
    CASE WHEN COALESCE(ed.superseded_by, 0) > 0 THEN 'superseded'
         WHEN COALESCE(sig.disputes, 0) > COALESCE(sig.confirms, 0) THEN 'disputed'
         ELSE 'current' END AS state,
    tanh(w.k * (w.w_touch   * COALESCE(sig.touches, 0)
              + w.w_confirm * COALESCE(sig.confirms, 0)
              + w.w_dispute * COALESCE(sig.disputes, 0)))::real AS evidence,
    -- standing = evidence - decay*position - state penalty, clamped,
    -- then mapped onto [floor, 1]. Mapped rather than used raw because
    -- a negative multiplier would invert the ordering and a zero one
    -- would delete the memory by arithmetic.
    --
    -- The state penalty is what stops a well-confirmed old belief
    -- outranking the entry that corrected it: evidence accrued while it
    -- was current does not go away when it is superseded, so without
    -- this the history view leads with the thing that is no longer true.
    (w.floor + (1 - w.floor) * ((LEAST(GREATEST(
        tanh(w.k * (w.w_touch   * COALESCE(sig.touches, 0)
                  + w.w_confirm * COALESCE(sig.confirms, 0)
                  + w.w_dispute * COALESCE(sig.disputes, 0)))
        - w.decay * pos.newer
        - CASE WHEN COALESCE(ed.superseded_by, 0) > 0 THEN w.pen_sup
               WHEN COALESCE(sig.disputes, 0) > COALESCE(sig.confirms, 0) THEN w.pen_dis
               ELSE 0 END
        - CASE WHEN mach.id IS NOT NULL THEN w.pen_mach ELSE 0 END,
        -1), 1) + 1) / 2))::real AS multiplier";

/// The joins [`MEMORY_FIELDS`] needs.
const MEMORY_JOINS: &str = "
    FROM memories mem
    CROSS JOIN w
    LEFT JOIN sig ON sig.memory_id = mem.id
    LEFT JOIN pos ON pos.id = mem.id
    LEFT JOIN ed   ON ed.to_id = mem.id
    LEFT JOIN mach ON mach.id = mem.id
    LEFT JOIN project p  ON mem.level = 'project' AND p.id = 1
    LEFT JOIN features f ON mem.level = 'feature' AND f.id = mem.subject_id
    LEFT JOIN stages s   ON mem.level = 'stage'   AND s.id = mem.subject_id
    LEFT JOIN modules m  ON mem.level = 'module'  AND m.id = mem.subject_id
    LEFT JOIN tasks t    ON mem.level = 'task'    AND t.id = mem.subject_id";

/// Row → [`Memory`], including the derived state and decomposed
/// standing. One mapper, so every read path agrees about what a memory
/// is.
fn map_memory(r: &sqlx::postgres::PgRow) -> Memory {
    Memory {
        id: r.get("id"),
        level: r.get("level"),
        subject_id: r.get("subject_id"),
        subject_name: r.get("subject_name"),
        kind: MemoryKind::parse(r.get::<String, _>("kind").as_str()).unwrap_or_default(),
        content: r.get("content"),
        tags: r.get("tags"),
        author: r.get("author"),
        created_at: r.get("created_at"),
        state: match r.get::<String, _>("state").as_str() {
            "superseded" => MemoryState::Superseded,
            "disputed" => MemoryState::Disputed,
            _ => MemoryState::Current,
        },
        standing: Standing {
            touches: r.get("touches"),
            confirms: r.get("confirms"),
            disputes: r.get("disputes"),
            newer_fraction: r.get("newer_fraction"),
            evidence: r.get("evidence"),
            multiplier: r.get("multiplier"),
        },
    }
}

/// Record a batch of touches from an exit door, ignoring ids that do
/// not resolve.
///
/// Lenient where `touch_memory` is strict: a completion must not fail
/// because an agent mistyped one id in a list of five. The work is
/// done, and refusing the completion over a bad reference would lose
/// far more than the touch was worth.
async fn attest_used(
    tx: &mut Tx<'_>,
    agent: &str,
    ids: &[String],
    note: &str,
) -> std::result::Result<(), McpmError> {
    if ids.is_empty() {
        return Ok(());
    }
    let batch = uuid::Uuid::new_v4();
    for id in ids {
        let known: Option<String> = sqlx::query_scalar("SELECT id FROM memories WHERE id = $1")
            .bind(id)
            .fetch_optional(&mut **tx)
            .await?;
        if known.is_some() {
            insert_signal(tx, id, agent, SignalKind::Touch, note, Some(batch)).await?;
        }
    }
    Ok(())
}

/// Append one piece of evidence. Never updates anything.
async fn insert_signal(
    tx: &mut Tx<'_>,
    memory_id: &str,
    agent: &str,
    kind: SignalKind,
    note: &str,
    batch: Option<uuid::Uuid>,
) -> std::result::Result<(), McpmError> {
    sqlx::query(
        "INSERT INTO memory_signals (memory_id, agent, kind, note, batch)
         VALUES ($1, $2, $3, $4, $5)",
    )
    .bind(memory_id)
    .bind(agent)
    .bind(kind.as_str())
    .bind(note)
    .bind(batch)
    .execute(&mut **tx)
    .await?;
    Ok(())
}

/// Split free text into safe tsquery lexemes.
///
/// Everything that is not alphanumeric becomes a separator, so no
/// caller input can reach `to_tsquery` as syntax — an unescaped `&`,
/// `|` or `!` there is a 500, and a `:` turns into a weight operator.
/// This is the only place caller text becomes part of a query
/// expression, which is why the sanitising lives here rather than at
/// each call site.
///
/// Single characters are dropped: they match nearly everything and
/// contribute nothing to ranking.
fn lexemes(text: &str) -> Vec<String> {
    text.split(|c: char| !c.is_alphanumeric())
        .filter(|w| w.chars().count() > 1)
        .map(|w| w.to_lowercase())
        .take(12) // A query longer than this is prose, not a search.
        .collect()
}

/// Row → [`ApiKeyInfo`]. Shared by every read path so the display shape
/// of a key is defined once.
fn map_key_row(r: &sqlx::postgres::PgRow) -> Result<ApiKeyInfo> {
    Ok(ApiKeyInfo {
        id: r.get("id"),
        label: r.get("label"),
        agent_name: r.get("agent_name"),
        role: KeyRole::parse(r.get::<String, _>("role").as_str())
            .ok_or_else(|| McpmError::internal("api_keys.role holds an unknown role"))?,
        created_at: r.get("created_at"),
        created_by: r.get("created_by"),
        last_used: r.get("last_used"),
        revoked_at: r.get("revoked_at"),
    })
}
