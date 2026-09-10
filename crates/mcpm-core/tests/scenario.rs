//! End-to-end replay of the architecture doc's worked scenario (§4) on
//! the module GRAPH: Schema & store → Report endpoints → {App, MCP
//! connector} — including the premature-claim rejection at the gate,
//! the double-path record (error to the worker AND event for the
//! manager), the per-module unlock, discovered tasks, blockers, memory
//! search directions, documents, and the completion guards.
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

fn module(name: &str, tasks: &[&str], deps: &[&str]) -> PlanModule {
    PlanModule {
        name: name.into(),
        description: String::new(),
        tasks: tasks.iter().map(|t| t.to_string()).collect(),
        depends_on: deps.iter().map(|d| d.to_string()).collect(),
        owns: vec![],
    }
}

fn plan() -> PlanFeature {
    PlanFeature {
        name: "Field reports".into(),
        description: "The worked example from the architecture doc.".into(),
        modules: vec![
            module("Schema & store", &["Design tables", "Write migration"], &[]),
            module("Report endpoints", &["CRUD endpoints", "Contract tests"], &["Schema & store"]),
            module("App", &["Report screen"], &["Report endpoints"]),
            module("MCP connector", &["Expose report tool"], &["Report endpoints"]),
        ],
        stages: vec![],
        whitepaper: Some("# Field reports\n\nReports are filed per shift and read by the office.".into()),
    }
}

fn id_of(tree: &FeatureTree, name: &str) -> String {
    tree.modules
        .iter()
        .find(|m| m.name == name)
        .map(|m| m.id.clone())
        .unwrap_or_else(|| panic!("module '{name}' in tree"))
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
        assert_eq!(tree.modules.len(), 4);
        // Topological order with depths: root first, the two leaves last.
        let names: Vec<&str> = tree.modules.iter().map(|m| m.name.as_str()).collect();
        assert_eq!(names, vec!["Schema & store", "Report endpoints", "App", "MCP connector"]);
        let depths: Vec<i32> = tree.modules.iter().map(|m| m.depth).collect();
        assert_eq!(depths, vec![1, 2, 3, 3]);
        assert!(tree.modules[0].dispatchable);
        assert!(!tree.modules[1].dispatchable);
        assert_eq!(tree.modules[1].waiting_on, vec![tree.modules[0].id.clone()]);
        assert!(tree.whitepaper.is_some(), "the plan's prose landed as revision 1");
        assert_eq!(tree.whitepaper.as_ref().unwrap().revision, 1);

        // A plan with no modules is rejected atomically; so is a cycle.
        let bad = store
            .plan_feature(
                manager,
                PlanFeature { name: "Bad".into(), ..Default::default() },
            )
            .await
            .unwrap_err();
        assert_eq!(bad.code, ErrorCode::PlanInvalid);
        let cyclic = store
            .plan_feature(
                manager,
                PlanFeature {
                    name: "Cyclic".into(),
                    modules: vec![module("a", &[], &["b"]), module("b", &[], &["a"])],
                    ..Default::default()
                },
            )
            .await
            .unwrap_err();
        assert_eq!(cyclic.code, ErrorCode::PlanInvalid);
        assert!(cyclic.message.contains("cycle"), "{}", cyclic.message);
        // And an unordered pair of writers on one path.
        let clash = store
            .plan_feature(
                manager,
                PlanFeature {
                    name: "Clash".into(),
                    modules: vec![
                        PlanModule { owns: vec!["src/store.rs".into()], ..module("a", &[], &[]) },
                        PlanModule { owns: vec!["src".into()], ..module("b", &[], &[]) },
                    ],
                    ..Default::default()
                },
            )
            .await
            .unwrap_err();
        assert_eq!(clash.code, ErrorCode::PlanInvalid);
        assert!(clash.message.contains("both own"), "{}", clash.message);

        // ---- next_work: exactly the root ----------------------------
        let work = store.next_work(&fid).await.unwrap();
        assert_eq!(work.dispatchable.len(), 1);
        assert_eq!(work.dispatchable[0].module_name, "Schema & store");
        let m_schema = work.dispatchable[0].module_id.clone();
        let m_api = id_of(&tree, "Report endpoints");
        let m_app = id_of(&tree, "App");
        let m_mcp = id_of(&tree, "MCP connector");

        // ---- The premature claim (the manager did a bad job) --------
        let eager = "agent.mod.connector";
        store.get_context(eager, "worker").await.unwrap();
        let err = store.claim_module(eager, &m_mcp).await.unwrap_err();
        assert_eq!(err.code, ErrorCode::PrereqsOpen);
        assert!(err.hint.contains("manager"), "hint tells the worker to report back");
        assert!(err.message.contains("Report endpoints"), "names the open prerequisite");
        // The rejection took the second path too: it is on the ledger
        // even though the claim failed.
        let events = store.get_events(Some(&fid), 0, 100).await.unwrap();
        assert!(
            events.iter().any(|e| e.kind == "premature_claim"),
            "premature_claim must be recorded for the manager's next poll"
        );

        // ---- The root works ------------------------------------------
        let w1 = "agent.mod.schema";
        store.get_context(w1, "worker").await.unwrap();
        let briefing = store.claim_module(w1, &m_schema).await.unwrap();
        assert_eq!(briefing.module.tasks.len(), 2);
        assert!(briefing.upstream_summaries.is_empty());
        assert!(briefing.whitepaper.is_some(), "the briefing carries the plan as prose");

        // A stranger cannot write to a claimed module.
        let stranger_err = store
            .complete_task(eager, &briefing.module.tasks[0].id, TaskOutcome::Done, None)
            .await
            .unwrap_err();
        assert_eq!(stranger_err.code, ErrorCode::NotClaimedByYou);

        // Completing with open tasks is refused, with the list.
        let open_err = store
            .complete_module(w1, &m_schema, "too early", &[], None)
            .await
            .unwrap_err();
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
        let discovered = tree_now
            .modules
            .iter()
            .find(|m| m.id == m_schema)
            .unwrap()
            .tasks
            .iter()
            .find(|t| t.origin == "discovered")
            .expect("discovered task recorded");
        store
            .complete_task(w1, &discovered.id, TaskOutcome::Done, None)
            .await
            .unwrap();

        // The worker documents what it built as it goes.
        let doc = store
            .write_document(
                w1,
                DocumentKind::Handoff,
                &m_schema,
                "Schema",
                "## reports\n\nOne row per filed report; `report_lines` hangs off it.",
            )
            .await
            .unwrap();
        assert_eq!(doc.revision, 1);
        // A stranger may not.
        let doc_err = store
            .write_document(eager, DocumentKind::Handoff, &m_schema, "", "mine now")
            .await
            .unwrap_err();
        assert_eq!(doc_err.code, ErrorCode::NotClaimedByYou);

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

        // Completion can carry a handoff revision of its own.
        let ack = store
            .complete_module(
                w1,
                &m_schema,
                "Schema landed: reports + report_lines tables.",
                &[],
                Some("## reports\n\nOne row per filed report. Use `db::reports::insert`."),
            )
            .await
            .unwrap();
        assert!(ack.message.contains("released 'Report endpoints'"), "ack: {}", ack.message);
        let events = store.get_events(Some(&fid), 0, 100).await.unwrap();
        let unlocked: Vec<&Event> = events.iter().filter(|e| e.kind == "module_unlocked").collect();
        assert_eq!(unlocked.len(), 1);
        assert_eq!(unlocked[0].subject_id.as_deref(), Some(m_api.as_str()));
        let handoff = store
            .read_document(DocumentKind::Handoff, &m_schema)
            .await
            .unwrap()
            .expect("current handoff");
        assert_eq!(handoff.revision, 2, "the completion appended a revision");
        assert!(handoff.body.contains("db::reports::insert"));

        // ---- The endpoints ------------------------------------------
        let work = store.next_work(&fid).await.unwrap();
        assert_eq!(work.dispatchable.len(), 1);
        assert_eq!(work.dispatchable[0].module_id, m_api);
        assert_eq!(work.dispatchable[0].depends_on, vec![m_schema.clone()]);

        let w2 = "agent.mod.api";
        store.get_context(w2, "worker").await.unwrap();
        let briefing = store.claim_module(w2, &m_api).await.unwrap();
        // The claim carries the upstream summary, marked as a
        // prerequisite, with the handoff beside it.
        assert_eq!(briefing.upstream_summaries.len(), 1);
        assert!(briefing.upstream_summaries[0].prerequisite);
        assert!(briefing.upstream_summaries[0].summary.contains("Schema landed"));
        assert!(briefing.upstream_summaries[0]
            .handoff
            .as_deref()
            .unwrap_or_default()
            .contains("db::reports::insert"));
        assert_eq!(briefing.module.depth, 2);

        // Worker reads conventions upstream: `up` from its module finds
        // nothing module-scoped of the schema module (different module),
        // but the feature-level search finds the decision via `down`.
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
        for task in store
            .feature_tree(&fid)
            .await
            .unwrap()
            .modules
            .iter()
            .find(|m| m.id == m_api)
            .unwrap()
            .tasks
            .iter()
        {
            store
                .complete_task(w2, &task.id, TaskOutcome::Done, None)
                .await
                .unwrap();
        }
        let ack = store
            .complete_module(
                w2,
                &m_api,
                "Endpoints live: GET/POST /reports with contract tests.",
                &[],
                None,
            )
            .await
            .unwrap();
        assert!(ack.message.contains("'App'") && ack.message.contains("'MCP connector'"), "{}", ack.message);

        // ---- Feature can't close early ------------------------------
        let close_err = store
            .complete_feature(manager, &fid, "premature")
            .await
            .unwrap_err();
        assert_eq!(close_err.code, ErrorCode::ModulesIncomplete);

        // ---- The leaves: two modules, two concurrent workers --------
        let work = store.next_work(&fid).await.unwrap();
        assert_eq!(work.dispatchable.len(), 2, "both leaves dispatch together");
        assert!(work.note.contains("none of them depends on another"));

        let w3 = "agent.mod.app";
        store.get_context(w3, "worker").await.unwrap();
        for module_id in [&m_app, &m_mcp] {
            // Both claimed by different workers; the connector worker
            // finally gets its module — legally this time.
            let worker = if module_id == &m_app { w3 } else { eager };
            let briefing = store.claim_module(worker, module_id).await.unwrap();
            // Prerequisites first (schema, then endpoints); a completed
            // sibling — App, once the connector claims after it — rides
            // behind them, marked as not a prerequisite.
            let chain: Vec<(&str, bool)> = briefing
                .upstream_summaries
                .iter()
                .map(|u| (u.module_name.as_str(), u.prerequisite))
                .collect();
            assert_eq!(&chain[..2], &[("Schema & store", true), ("Report endpoints", true)]);
            assert!(chain[2..].iter().all(|(_, p)| !p), "{chain:?}");
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
                .complete_module(worker, module_id, "Done.", &[], None)
                .await
                .unwrap();
        }
        // A completed sibling reaches a module that did not depend on it,
        // after the prerequisites.
        let last = store.feature_tree(&fid).await.unwrap();
        assert!(last.modules.iter().all(|m| m.status == "done"));

        // ---- Close --------------------------------------------------
        store
            .complete_feature(manager, &fid, "Field reports shipped end to end.")
            .await
            .unwrap();
        let rollups = store.rollups().await.unwrap();
        assert_eq!(rollups[0].status, "done");
        assert_eq!(rollups[0].modules_done, 4);
        assert_eq!(rollups[0].modules_ready, 0);

        // Every current document of the feature: one whitepaper, one
        // handoff (the schema's, at its second revision).
        let docs = store.feature_documents(&fid).await.unwrap();
        assert_eq!(docs.len(), 2, "{docs:?}");
        assert!(docs.iter().any(|d| d.kind == DocumentKind::Whitepaper && d.revision == 1));
        assert!(docs.iter().any(|d| d.kind == DocumentKind::Handoff && d.revision == 2));

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

/// The deprecated stage ladder still plans, and lowers to exactly the
/// edges the gate used to imply: each module waits on every module of
/// the preceding stage.
#[tokio::test]
async fn a_stage_ladder_lowers_to_edges() {
    with_scratch_store(|store| async move {
        store.get_context("mgr", "manager").await.unwrap();
        let tree = store
            .plan_feature(
                "mgr",
                PlanFeature {
                    name: "Ladder".into(),
                    stages: vec![
                        PlanStage {
                            name: "One".into(),
                            modules: vec![module("a", &[], &[]), module("b", &[], &[])],
                        },
                        PlanStage {
                            name: "Two".into(),
                            modules: vec![module("c", &[], &[])],
                        },
                    ],
                    ..Default::default()
                },
            )
            .await
            .unwrap();
        let c = tree.modules.iter().find(|m| m.name == "c").unwrap();
        let mut deps = c.depends_on.clone();
        deps.sort();
        let mut ab = vec![id_of(&tree, "a"), id_of(&tree, "b")];
        ab.sort();
        assert_eq!(deps, ab);
        assert_eq!(c.depth, 2);
        let both = store
            .plan_feature(
                "mgr",
                PlanFeature {
                    name: "Both".into(),
                    modules: vec![module("x", &[], &[])],
                    stages: vec![PlanStage { name: "S".into(), modules: vec![module("y", &[], &[])] }],
                    ..Default::default()
                },
            )
            .await
            .unwrap_err();
        assert_eq!(both.code, ErrorCode::PlanInvalid);
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
        let m_schema = id_of(&tree, "Schema & store");
        let m_api = id_of(&tree, "Report endpoints");
        let m_app = id_of(&tree, "App");

        // Claim the root, then try to remove it: refused.
        store.get_context("w", "worker").await.unwrap();
        store.claim_module("w", &m_schema).await.unwrap();
        let err = store
            .revise_plan(manager, &fid, vec![PlanOp::Remove { id: m_schema.clone() }])
            .await
            .unwrap_err();
        assert_eq!(err.code, ErrorCode::PlanConflict);

        // A prerequisite cannot be added to a module that has started.
        let started = store
            .revise_plan(
                manager,
                &fid,
                vec![PlanOp::AddDependency { module_id: m_schema.clone(), depends_on: m_app.clone() }],
            )
            .await
            .unwrap_err();
        assert_eq!(started.code, ErrorCode::PlanConflict);

        // A cycle is refused: App already waits (transitively) on the
        // endpoints, so the endpoints cannot wait on App.
        let cyc = store
            .revise_plan(
                manager,
                &fid,
                vec![PlanOp::AddDependency { module_id: m_api.clone(), depends_on: m_app.clone() }],
            )
            .await
            .unwrap_err();
        assert_eq!(cyc.code, ErrorCode::PlanInvalid);
        assert!(cyc.message.contains("cycle"), "{}", cyc.message);

        // Atomicity: a batch whose second op fails writes nothing.
        let before = store.feature_tree(&fid).await.unwrap();
        let err = store
            .revise_plan(
                manager,
                &fid,
                vec![
                    PlanOp::AddModule {
                        name: "Polish".into(),
                        description: String::new(),
                        tasks: vec![],
                        depends_on: vec![m_app.clone()],
                        owns: vec![],
                    },
                    PlanOp::Remove { id: m_schema.clone() },
                ],
            )
            .await
            .unwrap_err();
        assert_eq!(err.code, ErrorCode::PlanConflict);
        let after = store.feature_tree(&fid).await.unwrap();
        assert_eq!(
            before.modules.len(),
            after.modules.len(),
            "failed batch must not leave the added module behind"
        );

        // A good revision: a hardening module behind both leaves, an
        // owned path on the endpoints, and a rename.
        let revised = store
            .revise_plan(
                manager,
                &fid,
                vec![
                    PlanOp::AddModule {
                        name: "Hardening".into(),
                        description: "Rate limits".into(),
                        tasks: vec!["Limit writes".into()],
                        depends_on: vec![m_app.clone(), id_of(&tree, "MCP connector")],
                        owns: vec!["crates/api/src/limits.rs".into()],
                    },
                    PlanOp::UpdateModule {
                        id: m_api.clone(),
                        description: Some("The report endpoints.".into()),
                        owns: Some(vec!["crates/api/src/reports.rs".into()]),
                    },
                    PlanOp::Rename { id: m_app.clone(), name: "Mobile app".into() },
                ],
            )
            .await
            .unwrap();
        assert_eq!(revised.modules.len(), 5);
        let hardening = revised.modules.iter().find(|m| m.name == "Hardening").unwrap();
        assert_eq!(hardening.depth, 4);
        assert_eq!(hardening.depends_on.len(), 2);
        assert!(revised.modules.iter().any(|m| m.name == "Mobile app"));
        let api = revised.modules.iter().find(|m| m.id == m_api).unwrap();
        assert_eq!(api.description, "The report endpoints.");

        // An ownership clash introduced by a revision is refused where
        // it lands. Hardening sharing the endpoints' file is fine — it
        // is transitively downstream of them. The two leaves are
        // siblings with no edge between them, so giving both the same
        // path is two unordered writers, and the whole batch is refused.
        let clash = store
            .revise_plan(
                manager,
                &fid,
                vec![
                    PlanOp::UpdateModule {
                        id: m_app.clone(),
                        description: None,
                        owns: Some(vec!["crates/app/src/reports.rs".into()]),
                    },
                    PlanOp::UpdateModule {
                        id: id_of(&tree, "MCP connector"),
                        description: None,
                        owns: Some(vec!["crates/app".into()]),
                    },
                ],
            )
            .await
            .unwrap_err();
        assert_eq!(clash.code, ErrorCode::PlanInvalid, "{}", clash.message);
        assert!(clash.message.contains("both own"), "{}", clash.message);
        let unchanged = store.feature_tree(&fid).await.unwrap();
        assert!(
            unchanged.modules.iter().find(|m| m.id == m_app).unwrap().owns.is_empty(),
            "the refused batch wrote nothing"
        );
        // An edge can go as long as the writers it ordered stay ordered
        // some other way.
        let loosened = store
            .revise_plan(
                manager,
                &fid,
                vec![
                    PlanOp::AddDependency {
                        module_id: hardening.id.clone(),
                        depends_on: m_api.clone(),
                    },
                    PlanOp::UpdateModule {
                        id: hardening.id.clone(),
                        description: None,
                        owns: Some(vec!["crates/api/src/reports.rs".into()]),
                    },
                    PlanOp::RemoveDependency {
                        module_id: hardening.id.clone(),
                        depends_on: m_app.clone(),
                    },
                ],
            )
            .await;
        // Hardening -> endpoints keeps them ordered even after the App
        // edge goes, so this whole batch is legal.
        assert!(loosened.is_ok(), "{:?}", loosened.err());
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
                    modules: vec![module("Digest job", &["Schedule it"], &[])],
                    ..Default::default()
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
        modules: vec![
            module("CSV writer", &["Column mapping"], &[]),
            module("Download endpoint", &["Stream the file"], &["CSV writer"]),
        ],
        ..Default::default()
    }
}
