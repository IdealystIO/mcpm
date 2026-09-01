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

use serde_json::json;
use std::sync::Arc;

use sqlx::postgres::PgPoolOptions;
use tokio::sync::{broadcast, OnceCell};
use sqlx::{PgPool, Postgres, Row, Transaction};

use crate::error::{McpmError, ErrorCode};
use crate::ids::{id_level, new_id, new_memory_id, new_want_id, normalize_tag, Level};
use crate::keys::{ApiKeyInfo, IssuedKey, KeyIdentity, KeyRole};
use crate::types::*;

/// The Postgres NOTIFY channel the `events_notify` trigger announces
/// on. Named once here; the trigger names it in
/// `0005_notify_channel_rename.sql` (0004 installed it under the old
/// name and is frozen — sqlx checksums applied migrations).
const EVENT_CHANNEL: &str = "mcpm_events";

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
            .await?;
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
    pub async fn claim_module(&self, agent: &str, module_id: &str) -> Result<Briefing> {
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
        let ancestor_memories = self
            .search_memory(
                "",
                Some(MemoryScope {
                    level: Level::Module,
                    id: module_id.to_string(),
                }),
                SearchDirection::Up,
                &[],
                25,
            )
            .await?;
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
        })
    }

    /// Check off (or skip, with a reason) one task.
    pub async fn complete_task(
        &self,
        agent: &str,
        task_id: &str,
        outcome: TaskOutcome,
        note: Option<&str>,
    ) -> Result<Ack> {
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
        require_claim(&row, agent, "check off its tasks")?;
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
    pub async fn add_task(&self, agent: &str, module_id: &str, name: &str, note: Option<&str>) -> Result<Ack> {
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
        require_claim(&row, agent, "extend its checklist")?;
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
    pub async fn complete_module(&self, agent: &str, module_id: &str, summary: &str) -> Result<Ack> {
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
        require_claim(&row, agent, "complete it")?;
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
        insert_memory(&mut tx, Level::Module, module_id, summary, &["system", "summary"], agent).await?;
        record_event(
            &mut tx,
            "module_done",
            Some(&feature_id),
            Some(module_id),
            Some(agent),
            json!({ "module": module_name, "stage": stage_name }),
        )
        .await?;

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
    pub async fn report_blocker(&self, agent: &str, module_id: &str, description: &str) -> Result<Ack> {
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
        require_claim(&row, agent, "report a blocker on it")?;
        let module_name: String = row.get("module_name");
        let feature_id: String = row.get("feature_id");
        sqlx::query("UPDATE modules SET status = 'blocked' WHERE id = $1")
            .bind(module_id)
            .execute(&mut *tx)
            .await?;
        insert_memory(&mut tx, Level::Module, module_id, description, &["system", "blocker"], agent).await?;
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
    pub async fn release_module(&self, agent: &str, module_id: &str, reason: &str) -> Result<Ack> {
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
        require_claim(&row, agent, "release it")?;
        let module_name: String = row.get("module_name");
        let feature_id: String = row.get("feature_id");
        sqlx::query("UPDATE modules SET status = 'todo', claimed_by = NULL WHERE id = $1")
            .bind(module_id)
            .execute(&mut *tx)
            .await?;
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
        content: &str,
        tags: &[String],
    ) -> Result<Memory> {
        let subject_name = self
            .subject_name(scope.level, &scope.id)
            .await?
            .ok_or_else(|| McpmError::not_found(scope.level.as_str(), &scope.id))?;
        let mut tx = self.pool.begin().await?;
        let tag_refs: Vec<&str> = tags.iter().map(String::as_str).collect();
        let id = insert_memory(&mut tx, scope.level, &scope.id, content, &tag_refs, agent).await?;
        let feature_id = self.feature_of(scope.level, &scope.id).await?;
        record_event(
            &mut tx,
            "memory_committed",
            feature_id.as_deref(),
            Some(&scope.id),
            Some(agent),
            json!({ "level": scope.level.as_str(), "memory_id": id }),
        )
        .await?;
        tx.commit().await?;
        let row = sqlx::query("SELECT created_at FROM memories WHERE id = $1")
            .bind(&id)
            .fetch_one(&self.pool)
            .await?;
        Ok(Memory {
            id,
            level: scope.level.as_str().to_string(),
            subject_id: scope.id,
            subject_name,
            content: content.to_string(),
            tags: tags.to_vec(),
            author: agent.to_string(),
            created_at: row.get("created_at"),
        })
    }

    /// Full-text search over memories, scoped by an anchor + direction.
    /// An empty query returns the scope's memories newest-first.
    pub async fn search_memory(
        &self,
        query: &str,
        scope: Option<MemoryScope>,
        direction: SearchDirection,
        tags: &[String],
        limit: i64,
    ) -> Result<Vec<Memory>> {
        let (unscoped, sets) = match (&scope, direction) {
            (None, _) | (_, SearchDirection::All) => (true, SubjectSets::default()),
            (Some(scope), dir) => (false, self.subject_sets(scope, dir).await?),
        };
        let limit = limit.clamp(1, 100);
        let rows = sqlx::query(
            "SELECT mem.id, mem.level, mem.subject_id, mem.content, mem.tags, mem.author,
                    mem.created_at,
                    COALESCE(f.name, s.name, m.name, t.name, mem.subject_id) AS subject_name
             FROM memories mem
             LEFT JOIN features f ON mem.level = 'feature' AND f.id = mem.subject_id
             LEFT JOIN stages s   ON mem.level = 'stage'   AND s.id = mem.subject_id
             LEFT JOIN modules m  ON mem.level = 'module'  AND m.id = mem.subject_id
             LEFT JOIN tasks t    ON mem.level = 'task'    AND t.id = mem.subject_id
             WHERE ($1 OR (mem.level = 'feature' AND mem.subject_id = ANY($2))
                       OR (mem.level = 'stage'   AND mem.subject_id = ANY($3))
                       OR (mem.level = 'module'  AND mem.subject_id = ANY($4))
                       OR (mem.level = 'task'    AND mem.subject_id = ANY($5)))
               AND ($6 = '' OR mem.tsv @@ websearch_to_tsquery('english', $6))
               AND (cardinality($7::text[]) = 0 OR mem.tags && $7)
             ORDER BY CASE WHEN $6 = '' THEN 0
                           ELSE ts_rank(mem.tsv, websearch_to_tsquery('english', $6)) END DESC,
                      mem.created_at DESC
             LIMIT $8",
        )
        .bind(unscoped)
        .bind(&sets.features)
        .bind(&sets.stages)
        .bind(&sets.modules)
        .bind(&sets.tasks)
        .bind(query)
        .bind(tags)
        .bind(limit)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows
            .into_iter()
            .map(|r| Memory {
                id: r.get("id"),
                level: r.get("level"),
                subject_id: r.get("subject_id"),
                subject_name: r.get("subject_name"),
                content: r.get("content"),
                tags: r.get("tags"),
                author: r.get("author"),
                created_at: r.get("created_at"),
            })
            .collect())
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
        let table = match level {
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
        let sql = match level {
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
        let (feature, stage, module, task): (Option<String>, Option<String>, Option<String>, Option<String>) =
            match scope.level {
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
            sets.dedup();
        }
        // `down`: anchor + all descendants.
        if dir == SearchDirection::Down {
            match scope.level {
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
            json!({ "key_id": id, "label": label, "agent": agent_name, "role": role.as_str() }),
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

#[derive(Default)]
struct SubjectSets {
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
    content: &str,
    tags: &[&str],
    author: &str,
) -> std::result::Result<String, McpmError> {
    let id = new_memory_id();
    let tags: Vec<String> = tags.iter().map(|t| t.to_string()).collect();
    sqlx::query(
        "INSERT INTO memories (id, level, subject_id, content, tags, author)
         VALUES ($1, $2, $3, $4, $5, $6)",
    )
    .bind(&id)
    .bind(level.as_str())
    .bind(subject_id)
    .bind(content)
    .bind(&tags)
    .bind(author)
    .execute(&mut **tx)
    .await?;
    Ok(id)
}

/// Writes to a module's checklist require holding its claim.
fn require_claim(row: &sqlx::postgres::PgRow, agent: &str, action: &str) -> std::result::Result<(), McpmError> {
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
