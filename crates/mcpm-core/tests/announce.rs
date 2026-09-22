//! Announcements: an agent says what it is doing, and the roster, the
//! module and the feature each read the newest one back from the
//! ledger — plus the roster's rule for who is still in the picture.
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

fn plan(name: &str) -> PlanFeature {
    PlanFeature {
        name: name.into(),
        description: String::new(),
        whitepaper: None,
        roadmap_item: None,
        modules: vec![
            PlanModule {
                name: "schema".into(),
                description: String::new(),
                tasks: vec!["migration".into()],
                depends_on: vec![],
                owns: vec![],
            },
            PlanModule {
                name: "ui".into(),
                description: String::new(),
                tasks: vec!["screen".into()],
                depends_on: vec!["schema".into()],
                owns: vec![],
            },
        ],
        ..Default::default()
    }
}

#[tokio::test]
async fn an_announcement_is_read_back_on_the_module_the_feature_and_the_roster() {
    with_scratch("announce", |store| async move {
        store.get_context("planner", "manager").await.unwrap();
        store.get_context("box.a", "worker").await.unwrap();
        let tree = store.plan_feature("planner", plan("Announce")).await.unwrap();
        let schema = tree.modules.iter().find(|m| m.name == "schema").unwrap().id.clone();
        store.claim_module("box.a", &schema).await.unwrap();

        let ack = store
            .announce("box.a", &schema, "  e2e failed on smoke:48, fixing  ")
            .await
            .unwrap();
        assert!(ack.ok);
        assert!(ack.message.contains("schema"), "names the subject: {}", ack.message);

        // The module carries it.
        let tree = store.feature_tree(&tree.id).await.unwrap();
        let m = tree.modules.iter().find(|m| m.id == schema).unwrap();
        let word = m.last_word.as_ref().expect("module reads its last word");
        assert_eq!(word.text, "e2e failed on smoke:48, fixing", "trimmed");
        assert_eq!(word.by, "box.a");
        assert_eq!(word.subject, "schema");
        let ui = tree.modules.iter().find(|m| m.name == "ui").unwrap();
        assert!(ui.last_word.is_none(), "another module hears nothing");

        // The feature carries the newest one in it, whichever module.
        store.announce("box.a", &schema, "green again, on to the screen").await.unwrap();
        let roll = store.rollups().await.unwrap();
        let f = roll.iter().find(|f| f.id == tree.id).unwrap();
        assert_eq!(f.last_word.as_ref().unwrap().text, "green again, on to the screen");

        // A manager announces on the feature itself.
        store.announce("planner", &tree.id, "merging development in").await.unwrap();
        let roll = store.rollups().await.unwrap();
        let f = roll.iter().find(|f| f.id == tree.id).unwrap();
        assert_eq!(f.last_word.as_ref().unwrap().text, "merging development in");
        assert_eq!(f.last_word.as_ref().unwrap().subject_id, tree.id);
        // ...and it does not overwrite the module's own.
        let tree2 = store.feature_tree(&tree.id).await.unwrap();
        let m = tree2.modules.iter().find(|m| m.id == schema).unwrap();
        assert_eq!(m.last_word.as_ref().unwrap().text, "green again, on to the screen");

        // The roster shows each agent's newest word.
        let roster = store.agents_overview().await.unwrap();
        let a = roster.iter().find(|a| a.name == "box.a").unwrap();
        assert_eq!(a.last_word.as_ref().unwrap().text, "green again, on to the screen");
        let p = roster.iter().find(|a| a.name == "planner").unwrap();
        assert_eq!(p.last_word.as_ref().unwrap().text, "merging development in");

        // It is an event on the feature, so the manager's poll sees it.
        let status = store.feature_status(&tree.id, 0).await.unwrap();
        let kinds: Vec<&str> = status.events.iter().map(|e| e.kind.as_str()).collect();
        assert_eq!(kinds.iter().filter(|k| **k == "announcement").count(), 3);
        let last = status.events.iter().rev().find(|e| e.kind == "announcement").unwrap();
        assert_eq!(last.payload["text"], "merging development in");
        assert_eq!(last.payload["level"], "feature");
    })
    .await;
}

#[tokio::test]
async fn an_announcement_must_say_something_short_on_something_worked_on() {
    with_scratch("announce_refuse", |store| async move {
        store.get_context("planner", "manager").await.unwrap();
        let tree = store.plan_feature("planner", plan("Refuse")).await.unwrap();
        let schema = tree.modules.iter().find(|m| m.name == "schema").unwrap().id.clone();

        let err = store.announce("planner", &schema, "   ").await.unwrap_err();
        assert_eq!(err.code, ErrorCode::PlanInvalid);

        let long = "x".repeat(MAX_ANNOUNCEMENT_CHARS + 1);
        let err = store.announce("planner", &schema, &long).await.unwrap_err();
        assert_eq!(err.code, ErrorCode::PlanInvalid);
        assert!(err.message.contains("one line"), "{}", err.message);

        let want = store.add_want("planner", "a wish", &[]).await.unwrap();
        let err = store.announce("planner", &want.id, "working on it").await.unwrap_err();
        assert_eq!(err.code, ErrorCode::PlanInvalid);

        let err = store.announce("planner", "mod_nope", "hello").await.unwrap_err();
        assert_eq!(err.code, ErrorCode::NotFound);

        // Nothing landed.
        let status = store.feature_status(&tree.id, 0).await.unwrap();
        assert!(status.events.iter().all(|e| e.kind != "announcement"));
    })
    .await;
}

#[tokio::test]
async fn a_delegated_identity_announces_only_on_its_own_module() {
    with_scratch("announce_scope", |store| async move {
        store.get_context("laptop", "manager").await.unwrap();
        let tree = store.plan_feature("laptop", plan("Scope")).await.unwrap();
        let schema = tree.modules.iter().find(|m| m.name == "schema").unwrap().id.clone();
        let sub = Actor { name: "sub.schema".into(), scope: Some(schema.clone()), human: false };

        store.announce(sub.clone(), &schema, "on it").await.unwrap();
        let err = store.announce(sub.clone(), &tree.id, "on it").await.unwrap_err();
        assert_eq!(err.code, ErrorCode::Forbidden);

        let tree = store.feature_tree(&tree.id).await.unwrap();
        let m = tree.modules.iter().find(|m| m.id == schema).unwrap();
        assert_eq!(m.last_word.as_ref().unwrap().by, "sub.schema");
    })
    .await;
}

#[tokio::test]
async fn the_roster_lists_who_is_still_in_the_picture() {
    with_scratch("roster", |store| async move {
        store.get_context("planner", "manager").await.unwrap();
        store.get_context("box.claims", "worker").await.unwrap();
        store.get_context("box.quiet", "worker").await.unwrap();
        store.get_context("box.checked", "worker").await.unwrap();
        store.get_context("box.gone", "worker").await.unwrap();
        store.get_context("box.spoke", "worker").await.unwrap();
        let tree = store.plan_feature("planner", plan("Roster")).await.unwrap();
        let schema = tree.modules.iter().find(|m| m.name == "schema").unwrap().id.clone();
        store.claim_module("box.claims", &schema).await.unwrap();

        // Age every worker past the recency window, then give each its
        // one reason to stay — or none.
        sqlx::query("UPDATE agents SET last_seen = now() - interval '3 days' WHERE role = 'worker'")
            .execute(store.pool())
            .await
            .unwrap();
        sqlx::query(
            "UPDATE agents SET health_url = 'https://checked.example', health_state = NULL
             WHERE name = 'box.checked'",
        )
        .execute(store.pool())
        .await
        .unwrap();
        sqlx::query(
            "UPDATE agents SET health_url = 'https://gone.example', health_state = 'gone'
             WHERE name = 'box.gone'",
        )
        .execute(store.pool())
        .await
        .unwrap();
        store.announce("box.spoke", &tree.id, "still here").await.unwrap();

        let roster = store.agents_overview().await.unwrap();
        let names: Vec<&str> = roster.iter().map(|a| a.name.as_str()).collect();
        assert!(names.contains(&"planner"), "seen today: {names:?}");
        assert!(names.contains(&"box.claims"), "holds a claim: {names:?}");
        assert!(names.contains(&"box.checked"), "registered, not yet probed: {names:?}");
        assert!(names.contains(&"box.spoke"), "announcing counts as being seen: {names:?}");
        assert!(!names.contains(&"box.quiet"), "silent for days, nothing held: {names:?}");
        assert!(!names.contains(&"box.gone"), "its hostname answers 404: {names:?}");
        assert_eq!(names[0], "box.claims", "live claims first: {names:?}");
    })
    .await;
}
