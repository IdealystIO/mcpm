//! The roadmap: the graph above the work tree, and the two ship doors
//! it gates.
//!
//! What these guard, in order of how badly a regression would hurt:
//!
//! 1. **A held feature can still be BUILT.** Pre-planning against a
//!    roadmap is the whole reason to have one, and a lock that reached
//!    back into `claim_module` by default would quietly kill it.
//! 2. **A hard edge, and only a hard edge, holds work** — with the same
//!    `PREREQS_OPEN` a module edge gives, so no worker needs a new
//!    reaction.
//! 3. **A loose feature releases by itself.** The alternative is a
//!    second call per sprint that gets forgotten, and a board that
//!    reads as though nothing ever shipped.
//! 4. The ladder: modules prove the feature, features prove the item,
//!    items gate each other.
//!
//! Runs against a real Postgres (DATABASE_URL, default: the
//! devcontainer database on 55432), one scratch database per test.

use mcpm_core::*;
use sqlx::{Connection, Executor, PgConnection};

fn base_url() -> String {
    std::env::var("DATABASE_URL")
        .unwrap_or_else(|_| "postgres://app:app@localhost:55432/app".to_string())
}

async fn with_scratch<F, Fut>(tag: &str, body: F)
where
    F: FnOnce(Store) -> Fut,
    Fut: std::future::Future<Output = ()>,
{
    let base = base_url();
    let db_name = format!("mcpm_{tag}_{}", &uuid::Uuid::new_v4().simple().to_string()[..12]);
    let mut admin = PgConnection::connect(&base)
        .await
        .expect("connect to the devcontainer database (is it up on 55432?)");
    admin
        .execute(format!("CREATE DATABASE {db_name}").as_str())
        .await
        .expect("create scratch database");
    let (head, _) = base.rsplit_once('/').expect("url has a database segment");
    let scratch_url = format!("{head}/{db_name}");
    {
        let store = Store::connect(&scratch_url).await.expect("connect + migrate");
        store.ensure_project("t", "", "").await.expect("project row");
        body(store).await;
    }
    admin
        .execute(format!("DROP DATABASE {db_name} WITH (FORCE)").as_str())
        .await
        .expect("drop scratch database");
}

/// A one-module feature, optionally bound to an item by name.
fn plan(name: &str, item: Option<&str>) -> PlanFeature {
    PlanFeature {
        name: name.into(),
        description: String::new(),
        whitepaper: None,
        roadmap_item: item.map(str::to_string),
        stages: vec![],
        modules: vec![PlanModule {
            name: "build".into(),
            description: String::new(),
            tasks: vec!["do it".into()],
            depends_on: vec![],
            owns: vec![],
        }],
    }
}

/// Two items, `later` waiting on `first`. `hard` decides whether the
/// edge holds work or only the ship door.
async fn two_items(store: &Store, hard: bool) -> (String, String) {
    let road = store
        .plan_roadmap(
            "mgr",
            PlanRoadmap {
                items: vec![
                    PlanRoadmapItem {
                        name: "first".into(),
                        intent: "The thing that must exist.".into(),
                        horizon: "now".into(),
                        ..Default::default()
                    },
                    PlanRoadmapItem {
                        name: "later".into(),
                        intent: "The thing that builds on it.".into(),
                        horizon: "next".into(),
                        depends_on: if hard { vec![] } else { vec!["first".into()] },
                        hard_depends_on: if hard { vec!["first".into()] } else { vec![] },
                        ..Default::default()
                    },
                ],
            },
        )
        .await
        .expect("plan the roadmap");
    let id = |n: &str| road.items.iter().find(|i| i.name == n).expect("item").id.clone();
    (id("first"), id("later"))
}

/// Drive a one-module feature to `done`.
async fn finish(store: &Store, feature_id: &str) {
    let tree = store.feature_tree(feature_id).await.expect("tree");
    let module = &tree.modules[0];
    store.claim_module("w", &module.id).await.expect("claim");
    for t in &module.tasks {
        store
            .complete_task("w", &t.id, TaskOutcome::Done, None)
            .await
            .expect("task");
    }
    store
        .complete_module("w", &module.id, "built", &[], None)
        .await
        .expect("module");
    store
        .complete_feature("mgr", feature_id, "done")
        .await
        .expect("feature");
}

// ---------------------------------------------------------------------

#[tokio::test]
async fn a_held_feature_is_still_fully_buildable() {
    with_scratch("road_build", |store| async move {
        let (_first, later) = two_items(&store, false).await;
        let f = store.plan_feature("mgr", plan("Ahead", Some("later"))).await.expect("plan");

        // The point of planning against a roadmap: everything up to
        // the ship door works while the item is held.
        let tree = store.feature_tree(&f.id).await.expect("tree");
        assert_eq!(tree.roadmap_item.as_ref().map(|e| e.item_id.as_str()), Some(later.as_str()));
        let module = &tree.modules[0];
        store.claim_module("w", &module.id).await.expect("a soft edge must not hold the claim");
        store
            .complete_task("w", &module.tasks[0].id, TaskOutcome::Done, None)
            .await
            .expect("task");
        store.complete_module("w", &module.id, "built", &[], None).await.expect("module");
        let ack = store.complete_feature("mgr", &f.id, "done").await.expect("complete");
        assert!(
            ack.message.contains("HELD"),
            "completing a held feature should say so: {}",
            ack.message
        );

        // And only the ship door is shut.
        let err = store
            .release_feature("mgr", &f.id, "")
            .await
            .expect_err("release must be refused while the prerequisite is unshipped");
        assert_eq!(err.code, ErrorCode::RoadmapLocked);
        assert!(err.message.contains("first"), "the refusal names the item: {}", err.message);

        let tree = store.feature_tree(&f.id).await.expect("tree");
        assert_eq!(tree.status, "done");
        assert!(tree.released_at.is_none(), "done is not released");
        assert_eq!(tree.held_by.len(), 1);
    })
    .await;
}

#[tokio::test]
async fn a_hard_edge_holds_the_work_with_the_ordinary_refusal() {
    with_scratch("road_hard", |store| async move {
        let (first, _later) = two_items(&store, true).await;
        let f = store.plan_feature("mgr", plan("Blocked", Some("later"))).await.expect("plan");
        let tree = store.feature_tree(&f.id).await.expect("tree");
        let module_id = tree.modules[0].id.clone();

        let err = store
            .claim_module("w", &module_id)
            .await
            .expect_err("a hard edge holds the claim");
        // Deliberately the SAME code a module edge gives: a worker
        // needs no new reaction for a roadmap hold.
        assert_eq!(err.code, ErrorCode::PrereqsOpen);
        assert!(err.message.contains("first"), "names the item: {}", err.message);

        // The manager sees it on the ledger, as with any early dispatch.
        let events = store.get_events(Some(&f.id), 0, 50).await.expect("events");
        assert!(
            events.iter().any(|e| e.kind == "premature_claim"),
            "a refused claim is recorded"
        );

        // Shipping the prerequisite opens it.
        store.ship_roadmap_item("mgr", &first, "live").await.expect("ship");
        store.claim_module("w", &module_id).await.expect("claimable once the item shipped");
    })
    .await;
}

#[tokio::test]
async fn a_loose_feature_releases_when_it_completes() {
    with_scratch("road_loose", |store| async move {
        let f = store.plan_feature("mgr", plan("Sprint", None)).await.expect("plan");
        finish(&store, &f.id).await;
        let tree = store.feature_tree(&f.id).await.expect("tree");
        assert!(
            tree.released_at.is_some(),
            "nothing holds a loose feature, so completing it releases it"
        );
        assert!(tree.roadmap_item.is_none());

        // And the second door refuses politely rather than double-releasing.
        let err = store.release_feature("mgr", &f.id, "").await.expect_err("already out");
        assert_eq!(err.code, ErrorCode::AlreadyDone);
    })
    .await;
}

#[tokio::test]
async fn the_ladder_runs_features_then_items() {
    with_scratch("road_ladder", |store| async move {
        let (first, later) = two_items(&store, false).await;
        let a = store.plan_feature("mgr", plan("Part A", Some("first"))).await.expect("plan a");
        let b = store.plan_feature("mgr", plan("Part B", Some("first"))).await.expect("plan b");

        // An item cannot ship while a feature bound to it is unfinished.
        let err = store.ship_roadmap_item("mgr", &first, "").await.expect_err("features open");
        assert_eq!(err.code, ErrorCode::RoadmapLocked);
        assert!(err.message.contains("Part A"), "names the feature: {}", err.message);

        finish(&store, &a.id).await;
        finish(&store, &b.id).await;
        // Both done, neither released: a bound feature needs the second door.
        let err = store.ship_roadmap_item("mgr", &first, "").await.expect_err("not released");
        assert_eq!(err.code, ErrorCode::RoadmapLocked);

        store.release_feature("mgr", &a.id, "prod").await.expect("release a");
        store.release_feature("mgr", &b.id, "prod").await.expect("release b");

        let road = store.roadmap().await.expect("roadmap");
        let item = road.items.iter().find(|i| i.id == first).expect("first");
        assert_eq!(item.state, "ready", "everything released, nobody shipped it yet");
        let held = road.items.iter().find(|i| i.id == later).expect("later");
        assert_eq!(held.state, "held");

        let ack = store.ship_roadmap_item("mgr", &first, "live").await.expect("ship");
        assert!(ack.message.contains("later"), "says what it unblocked: {}", ack.message);

        let road = store.roadmap().await.expect("roadmap");
        assert_eq!(road.items.iter().find(|i| i.id == first).expect("first").state, "shipped");
        assert_eq!(road.items.iter().find(|i| i.id == later).expect("later").state, "future");
    })
    .await;
}

#[tokio::test]
async fn an_item_with_no_features_may_ship_and_unblocks_what_waits() {
    // The escape hatch: an item satisfied outside the work tree. Without
    // it a roadmap that names an external dependency holds its
    // dependents shut for good.
    with_scratch("road_external", |store| async move {
        let (first, later) = two_items(&store, false).await;
        store.ship_roadmap_item("mgr", &first, "vendor shipped it").await.expect("ship");
        let f = store.plan_feature("mgr", plan("Downstream", Some("later"))).await.expect("plan");
        finish(&store, &f.id).await;
        store.release_feature("mgr", &f.id, "prod").await.expect("nothing holds it now");
        store.ship_roadmap_item("mgr", &later, "live").await.expect("ship later");
    })
    .await;
}

#[tokio::test]
async fn the_plan_is_refused_whole() {
    with_scratch("road_validate", |store| async move {
        let err = store
            .plan_roadmap(
                "mgr",
                PlanRoadmap {
                    items: vec![PlanRoadmapItem {
                        name: "a".into(),
                        depends_on: vec!["nope".into()],
                        ..Default::default()
                    }],
                },
            )
            .await
            .expect_err("unknown prerequisite");
        assert_eq!(err.code, ErrorCode::PlanInvalid);
        assert!(store.roadmap().await.expect("roadmap").items.is_empty(), "nothing was written");

        let err = store
            .plan_roadmap(
                "mgr",
                PlanRoadmap {
                    items: vec![
                        PlanRoadmapItem { name: "a".into(), depends_on: vec!["b".into()], ..Default::default() },
                        PlanRoadmapItem { name: "b".into(), depends_on: vec!["a".into()], ..Default::default() },
                    ],
                },
            )
            .await
            .expect_err("cycle");
        assert_eq!(err.code, ErrorCode::PlanInvalid);
        assert!(err.message.contains("cycle"), "{}", err.message);
        assert!(store.roadmap().await.expect("roadmap").items.is_empty(), "nothing was written");

        // A cycle closed THROUGH what is already stored is the one a
        // per-plan check would miss.
        store
            .plan_roadmap(
                "mgr",
                PlanRoadmap {
                    items: vec![
                        PlanRoadmapItem { name: "a".into(), ..Default::default() },
                        PlanRoadmapItem { name: "b".into(), depends_on: vec!["a".into()], ..Default::default() },
                    ],
                },
            )
            .await
            .expect("plan");
        let err = store
            .plan_roadmap(
                "mgr",
                PlanRoadmap {
                    items: vec![PlanRoadmapItem {
                        name: "a".into(),
                        depends_on: vec!["b".into()],
                        ..Default::default()
                    }],
                },
            )
            .await
            .expect_err("cycle through stored edges");
        assert_eq!(err.code, ErrorCode::PlanInvalid);
    })
    .await;
}

#[tokio::test]
async fn re_stating_the_roadmap_updates_in_place() {
    with_scratch("road_restate", |store| async move {
        store
            .plan_roadmap(
                "mgr",
                PlanRoadmap {
                    items: vec![PlanRoadmapItem {
                        name: "a".into(),
                        intent: "first wording".into(),
                        ..Default::default()
                    }],
                },
            )
            .await
            .expect("plan");
        let road = store
            .plan_roadmap(
                "mgr",
                PlanRoadmap {
                    items: vec![
                        PlanRoadmapItem { name: "a".into(), intent: "second wording".into(), ..Default::default() },
                        PlanRoadmapItem { name: "b".into(), ..Default::default() },
                    ],
                },
            )
            .await
            .expect("re-state");
        assert_eq!(road.items.len(), 2, "'a' was updated, not duplicated");
        assert_eq!(road.items.iter().find(|i| i.name == "a").expect("a").intent, "second wording");
    })
    .await;
}

#[tokio::test]
async fn shelving_releases_what_an_item_was_holding() {
    with_scratch("road_shelve", |store| async move {
        let (first, _later) = two_items(&store, false).await;
        let f = store.plan_feature("mgr", plan("Ahead", Some("later"))).await.expect("plan");
        finish(&store, &f.id).await;
        store.release_feature("mgr", &f.id, "").await.expect_err("held");

        // Shelving is how a planner says "out of the picture". An edge
        // to a shelf that still held would be a roadmap nobody can
        // unstick.
        store
            .revise_roadmap("mgr", vec![RoadmapOp::ShelveItem { item_id: first, shelved: true }])
            .await
            .expect("shelve");
        store.release_feature("mgr", &f.id, "prod").await.expect("the shelf holds nothing");
    })
    .await;
}

#[tokio::test]
async fn removing_an_item_will_not_silently_loosen_its_features() {
    with_scratch("road_remove", |store| async move {
        let (first, _later) = two_items(&store, false).await;
        let f = store.plan_feature("mgr", plan("Bound", Some("first"))).await.expect("plan");

        let err = store
            .revise_roadmap("mgr", vec![RoadmapOp::RemoveItem { item_id: first.clone() }])
            .await
            .expect_err("a bound feature blocks removal");
        assert_eq!(err.code, ErrorCode::PlanInvalid);
        assert!(err.message.contains("Bound"), "names the feature: {}", err.message);

        store
            .revise_roadmap(
                "mgr",
                vec![
                    RoadmapOp::BindFeature { feature_id: f.id.clone(), item_id: None },
                    RoadmapOp::RemoveItem { item_id: first },
                ],
            )
            .await
            .expect("unbind, then remove");
        assert!(store.feature_tree(&f.id).await.expect("tree").roadmap_item.is_none());
    })
    .await;
}

#[tokio::test]
async fn the_digest_and_the_briefing_carry_the_direction() {
    with_scratch("road_context", |store| async move {
        let (_first, _later) = two_items(&store, false).await;
        let f = store.plan_feature("mgr", plan("Ahead", Some("first"))).await.expect("plan");

        let ctx = store.get_context("w", "worker").await.expect("context");
        assert_eq!(ctx.roadmap.len(), 2, "every unshelved item reaches every agent");
        let first_line = ctx.roadmap.iter().find(|l| l.name == "first").expect("first");
        assert_eq!(
            first_line.intent, "The thing that must exist.",
            "an unshipped item's intent travels WHOLE — a title alone teaches nothing"
        );

        let tree = store.feature_tree(&f.id).await.expect("tree");
        let briefing = store.claim_module("w", &tree.modules[0].id).await.expect("claim");
        let road = briefing.roadmap.expect("a bound feature briefs its worker on the roadmap");
        assert_eq!(road.item.name, "first");
        assert_eq!(road.unlocks.len(), 1, "the downstream half is the point");
        assert_eq!(road.unlocks[0].name, "later");
        assert!(
            road.guidance.contains("later") && road.guidance.contains("NOT building"),
            "guidance names what waits and warns off building it: {}",
            road.guidance
        );
    })
    .await;
}

#[tokio::test]
async fn a_worker_key_may_not_steer_the_roadmap() {
    // Direction and "it has gone out" are a manager's calls. Reading it
    // is deliberately not on the list: context withheld from the agent
    // writing the code is worth nothing.
    for tool in ["plan_roadmap", "revise_roadmap", "release_feature", "ship_roadmap_item"] {
        assert!(MANAGER_ONLY.contains(&tool), "{tool} must be manager-only");
    }
    assert!(!MANAGER_ONLY.contains(&"read_roadmap"), "every agent reads the roadmap");
}
