//! End-to-end replay of the architecture doc's worked scenario (§4):
//! a feature with three stages — Data modelling → API → Clients (two
//! modules) — including the premature-claim rejection at the gate, the
//! double-path record (error to the worker AND event for the manager),
//! stage unlocks, discovered tasks, blockers, memory search directions,
//! and the completion guards.
//!
//! Runs against a real Postgres: set DATABASE_URL (defaults to the
//! devcontainer database published on host port 55432). Each run
//! creates and drops its own scratch database, so parallel runs and
//! the live project data never collide.

use mcpm_core::*;
use sqlx::{Connection, Executor, PgConnection};

fn base_url() -> String {
    std::env::var("DATABASE_URL")
        .unwrap_or_else(|_| "postgres://app:app@localhost:55432/app".to_string())
}

/// Create a scratch database, run the scenario, drop it.
async fn with_scratch_store<F, Fut>(test: F)
where
    F: FnOnce(Store) -> Fut,
    Fut: std::future::Future<Output = ()>,
{
    let base = base_url();
    let db_name = format!("mcpm_test_{}", &uuid::Uuid::new_v4().simple().to_string()[..12]);
    let mut admin = PgConnection::connect(&base)
        .await
        .expect("connect to the devcontainer database (is it up on 55432?)");
    admin
        .execute(format!("CREATE DATABASE {db_name}").as_str())
        .await
        .expect("create scratch database");
    let scratch_url = {
        let (head, _tail) = base.rsplit_once('/').expect("url has a database segment");
        format!("{head}/{db_name}")
    };

    let store = Store::connect(&scratch_url).await.expect("connect + migrate");
    test(store).await;

    admin
        .execute(format!("DROP DATABASE {db_name} WITH (FORCE)").as_str())
        .await
        .expect("drop scratch database");
}

fn plan() -> PlanFeature {
    PlanFeature {
        name: "Field reports".into(),
        description: "The worked example from the architecture doc.".into(),
        stages: vec![
            PlanStage {
                name: "Data modelling".into(),
                modules: vec![PlanModule {
                    name: "Schema & store".into(),
                    description: "Tables + migrations".into(),
                    tasks: vec!["Design tables".into(), "Write migration".into()],
                }],
            },
            PlanStage {
                name: "API".into(),
                modules: vec![PlanModule {
                    name: "Report endpoints".into(),
                    description: String::new(),
                    tasks: vec!["CRUD endpoints".into(), "Contract tests".into()],
                }],
            },
            PlanStage {
                name: "Clients".into(),
                modules: vec![
                    PlanModule {
                        name: "App".into(),
                        description: String::new(),
                        tasks: vec!["Report screen".into()],
                    },
                    PlanModule {
                        name: "MCP connector".into(),
                        description: String::new(),
                        tasks: vec!["Expose report tool".into()],
                    },
                ],
            },
        ],
    }
}

#[tokio::test]
async fn the_worked_scenario() {
    with_scratch_store(|store| async move {
        store
            .ensure_project("control-center", "/tmp/x", "test project")
            .await
            .unwrap();

        // ---- Manager plans -----------------------------------------
        let manager = "agent.feature.reports";
        let ctx = store.get_context(manager, "manager").await.unwrap();
        assert!(ctx.features.is_empty());
        let tree = store.plan_feature(manager, plan()).await.unwrap();
        let fid = tree.id.clone();
        assert_eq!(tree.stages.len(), 3);
        assert_eq!(tree.stages[0].status, "unlocked");
        assert_eq!(tree.stages[1].status, "locked");
        assert_eq!(tree.stages[2].status, "locked");

        // Empty stages are rejected atomically.
        let bad = store
            .plan_feature(
                manager,
                PlanFeature {
                    name: "Bad".into(),
                    description: String::new(),
                    stages: vec![PlanStage { name: "Empty".into(), modules: vec![] }],
                },
            )
            .await
            .unwrap_err();
        assert_eq!(bad.code, ErrorCode::PlanInvalid);

        // ---- next_work: exactly stage 1's module --------------------
        let work = store.next_work(&fid).await.unwrap();
        assert_eq!(work.dispatchable.len(), 1);
        assert_eq!(work.dispatchable[0].module_name, "Schema & store");
        let m_schema = work.dispatchable[0].module_id.clone();
        let stage3_modules: Vec<String> = tree.stages[2]
            .modules
            .iter()
            .map(|m| m.id.clone())
            .collect();

        // ---- The premature claim (the manager did a bad job) --------
        let eager = "agent.mod.connector";
        store.get_context(eager, "worker").await.unwrap();
        let err = store.claim_module(eager, &stage3_modules[1]).await.unwrap_err();
        assert_eq!(err.code, ErrorCode::StageLocked);
        assert!(err.hint.contains("manager"), "hint tells the worker to report back");
        // The rejection took the second path too: it is on the ledger
        // even though the claim failed.
        let events = store.get_events(Some(&fid), 0, 100).await.unwrap();
        assert!(
            events.iter().any(|e| e.kind == "premature_claim"),
            "premature_claim must be recorded for the manager's next poll"
        );

        // ---- Stage 1 works ------------------------------------------
        let w1 = "agent.mod.schema";
        store.get_context(w1, "worker").await.unwrap();
        let briefing = store.claim_module(w1, &m_schema).await.unwrap();
        assert_eq!(briefing.module.tasks.len(), 2);
        assert!(briefing.upstream_summaries.is_empty());

        // A stranger cannot write to a claimed module.
        let stranger_err = store
            .complete_task(eager, &briefing.module.tasks[0].id, TaskOutcome::Done, None)
            .await
            .unwrap_err();
        assert_eq!(stranger_err.code, ErrorCode::NotClaimedByYou);

        // Completing with open tasks is refused, with the list.
        let open_err = store.complete_module(w1, &m_schema, "too early", &[]).await.unwrap_err();
        assert_eq!(open_err.code, ErrorCode::TasksOpen);

        // Skips need reasons.
        let skip_err = store
            .complete_task(w1, &briefing.module.tasks[0].id, TaskOutcome::Skipped, None)
            .await
            .unwrap_err();
        assert_eq!(skip_err.code, ErrorCode::SkipNeedsReason);

        // Work the checklist; discover one extra task on the way.
        store
            .complete_task(w1, &briefing.module.tasks[0].id, TaskOutcome::Done, None)
            .await
            .unwrap();
        store
            .add_task(w1, &m_schema, "Backfill fixture data", Some("found while migrating"))
            .await
            .unwrap();
        store
            .complete_task(w1, &briefing.module.tasks[1].id, TaskOutcome::Done, None)
            .await
            .unwrap();
        let tree_now = store.feature_tree(&fid).await.unwrap();
        let discovered = tree_now.stages[0].modules[0]
            .tasks
            .iter()
            .find(|t| t.origin == "discovered")
            .expect("discovered task recorded");
        store
            .complete_task(w1, &discovered.id, TaskOutcome::Done, None)
            .await
            .unwrap();

        // Record a decision for downstream workers.
        store
            .commit_memory(
                w1,
                MemoryScope { level: Level::Module, id: m_schema.clone() },
                MemoryKind::Decision,
                "Money amounts are integer minor units; currency codes are ISO 4217.",
                &["decision".into()],
                &[],
            )
            .await
            .unwrap();

        let ack = store
            .complete_module(w1, &m_schema, "Schema landed: reports + report_lines tables.", &[])
            .await
            .unwrap();
        assert!(ack.message.contains("unlocked stage 'API'"), "ack: {}", ack.message);
        let events = store.get_events(Some(&fid), 0, 100).await.unwrap();
        assert!(events.iter().any(|e| e.kind == "stage_unlocked"));

        // ---- Stage 2 ------------------------------------------------
        let work = store.next_work(&fid).await.unwrap();
        assert_eq!(work.dispatchable.len(), 1);
        assert_eq!(work.dispatchable[0].stage_position, 2);
        let m_api = work.dispatchable[0].module_id.clone();

        let w2 = "agent.mod.api";
        store.get_context(w2, "worker").await.unwrap();
        let briefing = store.claim_module(w2, &m_api).await.unwrap();
        // The claim carries the upstream summary + the recorded decision.
        assert_eq!(briefing.upstream_summaries.len(), 1);
        assert!(briefing.upstream_summaries[0].summary.contains("Schema landed"));

        // Worker reads conventions upstream: `up` from its module finds
        // nothing module-scoped of stage 1 (different module), but the
        // feature-level search finds the schema decision via `down`.
        let found = store
            .search_memory(&MemoryQuery {
                text: "currency".into(),
                scope: Some(MemoryScope { level: Level::Feature, id: fid.clone() }),
                direction: SearchDirection::Down,
                limit: 10,
                ..Default::default()
            })
            .await
            .unwrap();
        assert_eq!(found.hits.len(), 1);
        assert_eq!(found.total, 1);
        assert!(found.hits[0].memory.content.contains("ISO 4217"));
        assert_eq!(found.hits[0].memory.kind, MemoryKind::Decision);

        // Blocker path: report, manager sees it, then resume + finish.
        store
            .report_blocker(w2, &m_api, "Rate provider credentials missing from env.", &[])
            .await
            .unwrap();
        let status = store.feature_status(&fid, 0).await.unwrap();
        assert!(status.events.iter().any(|e| e.kind == "blocker_reported"));
        // Same worker resumes its own blocked module via claim_module.
        store.claim_module(w2, &m_api).await.unwrap();
        for task in store.feature_tree(&fid).await.unwrap().stages[1].modules[0].tasks.iter() {
            store
                .complete_task(w2, &task.id, TaskOutcome::Done, None)
                .await
                .unwrap();
        }
        store
            .complete_module(w2, &m_api, "Endpoints live: GET/POST /reports with contract tests.", &[])
            .await
            .unwrap();

        // ---- Feature can't close early ------------------------------
        let close_err = store
            .complete_feature(manager, &fid, "premature")
            .await
            .unwrap_err();
        assert_eq!(close_err.code, ErrorCode::StagesIncomplete);

        // ---- Stage 3: two modules, two concurrent workers -----------
        let work = store.next_work(&fid).await.unwrap();
        assert_eq!(work.dispatchable.len(), 2, "both Clients modules dispatch together");

        let w3 = "agent.mod.app";
        store.get_context(w3, "worker").await.unwrap();
        for module_id in &stage3_modules {
            // Both claimed by different workers; the connector worker
            // finally gets its module — legally this time.
            let worker = if module_id == &stage3_modules[0] { w3 } else { eager };
            let briefing = store.claim_module(worker, module_id).await.unwrap();
            // Two claims can't collide:
            let other = if worker == w3 { eager } else { w3 };
            let collide = store.claim_module(other, module_id).await.unwrap_err();
            assert_eq!(collide.code, ErrorCode::AlreadyClaimed);
            for task in &briefing.module.tasks {
                store
                    .complete_task(worker, &task.id, TaskOutcome::Done, None)
                    .await
                    .unwrap();
            }
            store
                .complete_module(worker, module_id, "Done.", &[])
                .await
                .unwrap();
        }

        // ---- Close --------------------------------------------------
        store
            .complete_feature(manager, &fid, "Field reports shipped end to end.")
            .await
            .unwrap();
        let rollups = store.rollups().await.unwrap();
        assert_eq!(rollups[0].status, "done");
        assert_eq!(rollups[0].stages_done, 3);
        assert_eq!(rollups[0].modules_done, 4);

        // Completion summaries are memories: a whole-project search
        // finds the feature summary without anyone writing docs.
        let summaries = store
            .search_memory(&MemoryQuery {
                text: "shipped".into(),
                limit: 10,
                ..Default::default()
            })
            .await
            .unwrap();
        assert!(summaries
            .hits
            .iter()
            .any(|h| h.memory.level == "feature" && h.memory.kind == MemoryKind::Outcome));

        // Tag-filtered search: the system-tagged module summaries.
        let sys = store
            .search_memory(&MemoryQuery {
                tags: vec!["summary".into()],
                limit: 50,
                ..Default::default()
            })
            .await
            .unwrap();
        assert!(
            sys.hits.len() >= 4,
            "each module + feature completion committed a summary"
        );

        // ---- get_context suggests resumes ---------------------------
        let ctx = store.get_context(manager, "manager").await.unwrap();
        assert_eq!(ctx.features.len(), 1);
        assert!(ctx.your_claims.is_empty());
    })
    .await;
}

#[tokio::test]
async fn revise_plan_guards_history() {
    with_scratch_store(|store| async move {
        store.ensure_project("t", "", "").await.unwrap();
        let manager = "mgr";
        store.get_context(manager, "manager").await.unwrap();
        let tree = store.plan_feature(manager, plan()).await.unwrap();
        let fid = tree.id.clone();
        let m1 = tree.stages[0].modules[0].id.clone();

        // Claim stage 1's module, then try to remove it: refused.
        store.get_context("w", "worker").await.unwrap();
        store.claim_module("w", &m1).await.unwrap();
        let err = store
            .revise_plan(manager, &fid, vec![PlanOp::Remove { id: m1.clone() }])
            .await
            .unwrap_err();
        assert_eq!(err.code, ErrorCode::PlanConflict);

        // Atomicity: a batch whose second op fails writes nothing.
        let before = store.feature_tree(&fid).await.unwrap();
        let err = store
            .revise_plan(
                manager,
                &fid,
                vec![
                    PlanOp::AddStage { name: "Polish".into(), after: None },
                    PlanOp::Remove { id: m1.clone() },
                ],
            )
            .await
            .unwrap_err();
        assert_eq!(err.code, ErrorCode::PlanConflict);
        let after = store.feature_tree(&fid).await.unwrap();
        assert_eq!(
            before.stages.len(),
            after.stages.len(),
            "failed batch must not leave the added stage behind"
        );

        // A good revision: insert a stage after stage 1, rename a module.
        let s1 = tree.stages[0].id.clone();
        let revised = store
            .revise_plan(
                manager,
                &fid,
                vec![
                    PlanOp::AddStage { name: "Hardening".into(), after: Some(s1) },
                    PlanOp::AddModule {
                        stage_id: store.feature_tree(&fid).await.unwrap().stages[0].id.clone(),
                        name: "placeholder".into(),
                        description: String::new(),
                        tasks: vec![],
                    },
                ],
            )
            .await;
        // AddModule went to stage 1 which is in progress (not complete) — allowed.
        let revised = revised.unwrap();
        assert_eq!(revised.stages.len(), 4);
        assert_eq!(revised.stages[1].name, "Hardening");
        assert_eq!(revised.stages[1].position, 2);
        assert_eq!(revised.stages[2].position, 3);
    })
    .await;
}

/// The want pool end to end: capture loose ideas, compose a GROUP of
/// them into one feature, and check every invariant that keeps the
/// pool honest — derived status, the frozen-once-promoted rule,
/// declines needing a reason, atomic promotion, and reuse of one want
/// across two features.
#[tokio::test]
async fn wants_compose_into_features() {
    with_scratch_store(|store| async move {
        let human = "agent.inbox";

        // --- Capture: three loose ideas, no structure demanded --------
        let w1 = store
            .add_want(human, "exports are painful, I always end up in a spreadsheet", &["ux".into()])
            .await
            .expect("capture");
        let w2 = store
            .add_want(human, "would be nice to get a weekly digest by email", &[])
            .await
            .expect("capture");
        let w3 = store
            .add_want(human, "someone asked for CSV again", &["ux".into()])
            .await
            .expect("capture");
        assert_eq!(w1.status, "open");
        assert_eq!(w1.author, human);

        let pool = store.list_wants("", WantFilter::Open, &[], 50).await.unwrap();
        assert_eq!(pool.open, 3);
        assert_eq!(pool.promoted, 0);
        assert_eq!(pool.wants.len(), 3);

        // Full-text search over the raw wording, and tag filtering.
        let hits = store.list_wants("spreadsheet", WantFilter::Open, &[], 50).await.unwrap();
        assert_eq!(hits.wants.len(), 1);
        assert_eq!(hits.wants[0].id, w1.id);
        let tagged = store
            .list_wants("", WantFilter::Open, &["ux".to_string()], 50)
            .await
            .unwrap();
        assert_eq!(tagged.wants.len(), 2, "two wants carry the ux tag");

        // --- A promotion that would drop a want is refused whole ------
        let bogus = store
            .promote_wants(
                "agent.composer",
                PromoteWants {
                    wants: vec![
                        WantRef { id: w1.id.clone(), rationale: String::new() },
                        WantRef { id: "want_nope".into(), rationale: String::new() },
                    ],
                    feature_id: None,
                    plan: Some(export_plan()),
                },
            )
            .await
            .expect_err("unknown want id");
        assert_eq!(bogus.code, ErrorCode::NotFound);
        assert!(
            store.list_wants("", WantFilter::Promoted, &[], 50).await.unwrap().wants.is_empty(),
            "nothing was written by the failed promotion"
        );

        // --- Compose: two wants become ONE feature --------------------
        let promotion = store
            .promote_wants(
                "agent.composer",
                PromoteWants {
                    wants: vec![
                        WantRef {
                            id: w1.id.clone(),
                            rationale: "the spreadsheet detour is the export module".into(),
                        },
                        WantRef {
                            id: w3.id.clone(),
                            rationale: "CSV is the first export format".into(),
                        },
                    ],
                    feature_id: None,
                    plan: Some(export_plan()),
                },
            )
            .await
            .expect("compose");
        let feature_id = promotion.feature.id.clone();
        assert_eq!(promotion.linked.len(), 2);
        assert!(promotion.linked.iter().all(|w| w.status == "promoted"));

        // Status is derived from the link, so the pool re-reads clean.
        let pool = store.list_wants("", WantFilter::Open, &[], 50).await.unwrap();
        assert_eq!(pool.open, 1, "only the digest want is still loose");
        assert_eq!(pool.promoted, 2);
        assert_eq!(pool.wants[0].id, w2.id);

        // The composition is on the record, both ways round.
        let sources = store.wants_of_feature(&feature_id).await.unwrap();
        assert_eq!(sources.len(), 2);
        let back = store.get_want(&w1.id).await.unwrap();
        assert_eq!(back.features.len(), 1);
        assert_eq!(back.features[0].feature_id, feature_id);
        assert_eq!(back.features[0].rationale, "the spreadsheet detour is the export module");

        // The raw ideas reach the workers: promotion commits a
        // feature-scope memory that search_memory(up) will surface.
        let origin = store
            .search_memory(&MemoryQuery {
                text: "spreadsheet".into(),
                tags: vec!["origin".to_string()],
                limit: 10,
                ..Default::default()
            })
            .await
            .unwrap();
        assert_eq!(origin.hits.len(), 1, "origin memory committed with the promotion");
        assert!(origin.hits[0].memory.content.contains("CSV is the first export format"));

        // --- A promoted want is frozen --------------------------------
        let frozen = store
            .update_want(
                human,
                &w1.id,
                WantEdit { body: Some("actually I meant PDF".into()), ..Default::default() },
            )
            .await
            .expect_err("promoted wants keep their wording");
        assert_eq!(frozen.code, ErrorCode::WantPromoted);
        let cannot_decline = store
            .update_want(
                human,
                &w1.id,
                WantEdit {
                    state: Some(WantState::Declined),
                    reason: Some("changed my mind".into()),
                    ..Default::default()
                },
            )
            .await
            .expect_err("a planned want cannot be declined");
        assert_eq!(cannot_decline.code, ErrorCode::WantPromoted);
        // Retagging stays legal: tags are filing, not content.
        let retagged = store
            .update_want(human, &w1.id, WantEdit { tags: Some(vec!["ux".into(), "export".into()]), ..Default::default() })
            .await
            .expect("retagging a promoted want");
        assert_eq!(retagged.tags.len(), 2);

        // Re-linking to the SAME feature is refused; a second feature is
        // legitimate reuse of one idea.
        let dupe = store
            .promote_wants(
                "agent.composer",
                PromoteWants {
                    wants: vec![WantRef { id: w1.id.clone(), rationale: String::new() }],
                    feature_id: Some(feature_id.clone()),
                    plan: None,
                },
            )
            .await
            .expect_err("already part of that feature");
        assert_eq!(dupe.code, ErrorCode::AlreadyLinked);

        let second = store
            .plan_feature(
                "agent.composer",
                PlanFeature {
                    name: "Reporting surface".into(),
                    description: String::new(),
                    stages: vec![PlanStage {
                        name: "Build".into(),
                        modules: vec![PlanModule {
                            name: "Digest job".into(),
                            description: String::new(),
                            tasks: vec!["Schedule it".into()],
                        }],
                    }],
                },
            )
            .await
            .expect("second feature");
        store
            .promote_wants(
                "agent.composer",
                PromoteWants {
                    wants: vec![WantRef {
                        id: w1.id.clone(),
                        rationale: "the digest reuses the export renderer".into(),
                    }],
                    feature_id: Some(second.id.clone()),
                    plan: None,
                },
            )
            .await
            .expect("one want can inform several features");
        let shared = store.get_want(&w1.id).await.unwrap();
        assert_eq!(shared.features.len(), 2);

        // --- Declining needs a reason, and blocks promotion -----------
        let no_reason = store
            .update_want(human, &w2.id, WantEdit { state: Some(WantState::Declined), ..Default::default() })
            .await
            .expect_err("a decline without a why is a lost idea");
        assert_eq!(no_reason.code, ErrorCode::SkipNeedsReason);

        let declined = store
            .update_want(
                human,
                &w2.id,
                WantEdit {
                    state: Some(WantState::Declined),
                    reason: Some("no mail infrastructure this quarter".into()),
                    ..Default::default()
                },
            )
            .await
            .expect("decline with a reason");
        assert_eq!(declined.status, "declined");
        assert_eq!(declined.decline_reason.as_deref(), Some("no mail infrastructure this quarter"));

        let refused = store
            .promote_wants(
                "agent.composer",
                PromoteWants {
                    wants: vec![WantRef { id: w2.id.clone(), rationale: String::new() }],
                    feature_id: Some(second.id.clone()),
                    plan: None,
                },
            )
            .await
            .expect_err("declined wants stay out until reopened");
        assert_eq!(refused.code, ErrorCode::PlanConflict);
        assert!(refused.message.contains("no mail infrastructure"), "the reason travels with the refusal");

        // Reopening clears the decline and makes it composable again.
        let reopened = store
            .update_want(human, &w2.id, WantEdit { state: Some(WantState::Open), ..Default::default() })
            .await
            .expect("reopen");
        assert_eq!(reopened.status, "open");
        assert!(reopened.decline_reason.is_none());
        store
            .promote_wants(
                "agent.composer",
                PromoteWants {
                    wants: vec![WantRef { id: w2.id, rationale: "the digest itself".into() }],
                    feature_id: Some(second.id.clone()),
                    plan: None,
                },
            )
            .await
            .expect("a reopened want promotes");

        // --- The ledger tells the whole story -------------------------
        let events = store.get_events(None, 0, 200).await.unwrap();
        let kinds: Vec<&str> = events.iter().map(|e| e.kind.as_str()).collect();
        assert_eq!(kinds.iter().filter(|k| **k == "want_added").count(), 3);
        assert_eq!(kinds.iter().filter(|k| **k == "wants_promoted").count(), 3);
        assert!(kinds.contains(&"want_declined"));
        assert!(kinds.contains(&"want_reopened"));
        assert!(kinds.contains(&"want_updated"));

        // Promotion events are feature-scoped, so a manager polling its
        // own feature sees where the work came from.
        let feature_events = store.get_events(Some(&feature_id), 0, 200).await.unwrap();
        assert!(feature_events.iter().any(|e| e.kind == "wants_promoted"));
        assert!(feature_events.iter().all(|e| e.kind != "want_added"), "captures stay project-level");

        // A malformed promotion is caught before anything is written.
        let neither = store
            .promote_wants(
                "agent.composer",
                PromoteWants { wants: vec![WantRef { id: w3.id, rationale: String::new() }], feature_id: None, plan: None },
            )
            .await
            .expect_err("needs a target");
        assert_eq!(neither.code, ErrorCode::PlanInvalid);
    })
    .await;
}

fn export_plan() -> PlanFeature {
    PlanFeature {
        name: "Export & share".into(),
        description: "Composed from the export wants.".into(),
        stages: vec![
            PlanStage {
                name: "Renderer".into(),
                modules: vec![PlanModule {
                    name: "CSV writer".into(),
                    description: String::new(),
                    tasks: vec!["Column mapping".into()],
                }],
            },
            PlanStage {
                name: "Surface".into(),
                modules: vec![PlanModule {
                    name: "Download endpoint".into(),
                    description: String::new(),
                    tasks: vec!["Stream the file".into()],
                }],
            },
        ],
    }
}

/// The tag registry and bulk capture: what the console's composer posts
/// on every keystroke-ending Enter, and what an agent's `add_wants`
/// does — same path, same guarantees.
#[tokio::test]
async fn tags_are_registered_as_wants_are_captured() {
    with_scratch_store(|store| async move {
        // A preset: a tag set up before any idea uses it.
        let preset = store.create_tag("console", "Field Reports").await.expect("preset");
        assert_eq!(preset.name, "field-reports", "normalized to a slug");
        assert_eq!(preset.label, "Field Reports", "display text kept as typed");
        assert_eq!(preset.uses, 0);
        // Idempotent — "make sure this exists" is the caller's intent.
        let again = store.create_tag("console", "#field-reports").await.expect("again");
        assert_eq!(again.name, preset.name);
        assert_eq!(store.list_tags().await.unwrap().len(), 1);

        // Bulk capture, the shape a brain-dump has.
        let captured = store
            .add_wants(
                "console",
                vec![
                    WantDraft {
                        body: "exports are painful".into(),
                        tags: vec!["ux".into(), "field-reports".into()],
                    },
                    WantDraft {
                        body: "weekly digest by email".into(),
                        tags: vec!["Ops".into()],
                    },
                    WantDraft { body: "dark mode contrast".into(), tags: vec![] },
                ],
            )
            .await
            .expect("capture");
        assert_eq!(captured.len(), 3);
        assert_eq!(captured[0].body, "exports are painful", "order is capture order");
        assert_eq!(captured[2].body, "dark mode contrast");

        // Tags used for the first time are registered on the way through,
        // normalized, and counted.
        let tags = store.list_tags().await.unwrap();
        let by_name = |n: &str| tags.iter().find(|t| t.name == n).cloned_uses();
        assert_eq!(by_name("field-reports"), 1);
        assert_eq!(by_name("ux"), 1);
        assert_eq!(by_name("ops"), 1, "'Ops' normalized to 'ops'");
        assert_eq!(tags.len(), 3);

        // Most-used first, so autocomplete offers the live vocabulary.
        let more = vec![
            WantDraft { body: "another export gripe".into(), tags: vec!["ux".into()] },
            WantDraft { body: "and another".into(), tags: vec!["ux".into()] },
        ];
        store.add_wants("console", more).await.expect("more");
        assert_eq!(store.list_tags().await.unwrap()[0].name, "ux");

        // The batch is atomic: one empty body rejects the whole list.
        let before = store.list_wants("", WantFilter::All, &[], 100).await.unwrap();
        let rejected = store
            .add_wants(
                "console",
                vec![
                    WantDraft { body: "this one is fine".into(), tags: vec![] },
                    WantDraft { body: "   ".into(), tags: vec![] },
                ],
            )
            .await
            .expect_err("empty body");
        assert_eq!(rejected.code, ErrorCode::PlanInvalid);
        let after = store.list_wants("", WantFilter::All, &[], 100).await.unwrap();
        assert_eq!(after.wants.len(), before.wants.len(), "nothing was written");

        // Filtering by a normalized tag finds what was captured under it.
        let ux = store
            .list_wants("", WantFilter::Open, &["ux".to_string()], 50)
            .await
            .unwrap();
        assert_eq!(ux.wants.len(), 3);

        // Every capture is on the ledger, and a first-use tag says so.
        let events = store.get_events(None, 0, 200).await.unwrap();
        assert_eq!(
            events.iter().filter(|e| e.kind == "want_added").count(),
            5
        );
        assert_eq!(
            events.iter().filter(|e| e.kind == "tag_created").count(),
            3,
            "the preset plus the two first-use tags — never twice for the same tag"
        );
    })
    .await;
}

/// Small helper so the assertions above read as counts.
trait UsesExt {
    fn cloned_uses(self) -> i64;
}
impl UsesExt for Option<&TagInfo> {
    fn cloned_uses(self) -> i64 {
        self.map(|t| t.uses).unwrap_or(-1)
    }
}
