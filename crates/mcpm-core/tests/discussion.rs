//! Discussion on features, wants and modules — and the half of the
//! gate it adds: an open question holds the work until the person or
//! agent it names answers.
//!
//! Runs against a real Postgres (DATABASE_URL, default: the
//! devcontainer database on 55432) with an in-memory object store.

use std::sync::Arc;

use mcpm_core::*;
use sqlx::{Connection, Executor, PgConnection};

fn base_url() -> String {
    std::env::var("DATABASE_URL")
        .unwrap_or_else(|_| "postgres://app:app@localhost:55432/app".to_string())
}

async fn with_scratch<F, Fut>(tag: &str, body: F)
where
    F: FnOnce(Store, Arc<MemoryFiles>) -> Fut,
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
        let files = MemoryFiles::new();
        let store = Store::connect(&scratch_url)
            .await
            .expect("connect + migrate")
            .with_files(files.clone());
        store.ensure_project("t", "", "").await.expect("project row");
        body(store, files).await;
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
        modules: vec![
            PlanModule {
                name: "A".into(),
                description: String::new(),
                tasks: vec!["a".into()],
                depends_on: vec![],
                owns: vec![],
            },
            PlanModule {
                name: "B".into(),
                description: String::new(),
                tasks: vec!["b".into()],
                depends_on: vec![],
                owns: vec![],
            },
        ],
        stages: vec![],
    }
}

fn file(name: &str, bytes: &[u8]) -> InlineFile {
    InlineFile {
        name: name.into(),
        description: String::new(),
        content_type: "text/plain".into(),
        bytes: bytes.to_vec(),
    }
}

#[tokio::test]
async fn a_question_holds_a_module_until_its_owner_answers() {
    with_scratch("disc", |store, files| async move {
        let tree = store.plan_feature("manager", plan("Exports")).await.expect("plan");
        let a = tree.modules.iter().find(|m| m.name == "A").unwrap().id.clone();
        let b = tree.modules.iter().find(|m| m.name == "B").unwrap().id.clone();

        // A note blocks nothing.
        let note = store
            .add_comment("human", &a, "Remember the old report format.", vec![file("old.txt", b"cols")])
            .await
            .expect("note");
        assert_eq!(note.kind, CommentKind::Note);
        assert_eq!(note.attachments.len(), 1);
        assert_eq!(note.attachments[0].comment.as_ref().map(|c| c.author.as_str()), Some("human"));
        assert!(store.feature_tree(&tree.id).await.unwrap().modules.iter().all(|m| m.dispatchable));
        // A comment's file is the module's file, and so the feature's.
        let listed = store.attachments_of(&tree.id).await.expect("files");
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].via_module.as_ref().map(|m| m.name.as_str()), Some("A"));

        // A question on module A, owed by the manager, holds A only.
        let q = store
            .ask_question("human", &a, "Which currency for totals?", Some("manager"), vec![])
            .await
            .expect("ask");
        assert_eq!(q.kind, CommentKind::Question);
        assert!(!q.resolved);
        let t = store.feature_tree(&tree.id).await.unwrap();
        let ma = t.modules.iter().find(|m| m.id == a).unwrap();
        let mb = t.modules.iter().find(|m| m.id == b).unwrap();
        assert!(!ma.dispatchable && ma.open_questions.len() == 1);
        assert!(mb.dispatchable);
        let err = store.claim_module("worker", &a).await.expect_err("held");
        assert_eq!(err.code, ErrorCode::PendingResolution);
        let work = store.next_work(&tree.id).await.unwrap();
        assert_eq!(work.dispatchable.len(), 1);
        assert_eq!(work.pending.len(), 1);

        // The wrong agent cannot answer; the manager sees it waiting.
        let err = store
            .answer_question("worker", &q.id, "USD", vec![])
            .await
            .expect_err("not the owner");
        assert_eq!(err.code, ErrorCode::Forbidden);
        let ctx = store.get_context("manager", "manager").await.unwrap();
        assert_eq!(ctx.awaiting_you.len(), 1);
        assert!(ctx.suggested_next.contains("answer_question"));

        // The answer resolves it and the gate opens.
        let ans = store
            .answer_question("manager", &q.id, "Report currency, converted at day rate.", vec![])
            .await
            .expect("answer");
        assert_eq!(ans.kind, CommentKind::Answer);
        assert_eq!(ans.answers.as_deref(), Some(q.id.as_str()));
        let q2 = store.comment(&q.id).await.unwrap();
        assert!(q2.resolved);
        assert_eq!(q2.resolved_by.as_deref(), Some("manager"));
        let briefing = store.claim_module("worker", &a).await.expect("claim now");
        assert_eq!(briefing.discussion.len(), 3, "note, question, answer");
        assert!(store.get_context("manager", "manager").await.unwrap().awaiting_you.is_empty());

        // A feature-level question holds everything, and a human may
        // answer whoever it names.
        let fq = store
            .ask_question("manager", &tree.id, "Ship as v1 or behind a flag?", Some("nicho"), vec![])
            .await
            .expect("ask feature");
        assert!(store.feature_tree(&tree.id).await.unwrap().modules.iter().all(|m| !m.dispatchable));
        let err = store.claim_module("worker2", &b).await.expect_err("feature held");
        assert_eq!(err.code, ErrorCode::PendingResolution);
        store
            .answer_question(Actor::human("console"), &fq.id, "Behind a flag.", vec![])
            .await
            .expect("human answers");
        assert!(store.feature_tree(&tree.id).await.unwrap().modules.iter().any(|m| m.dispatchable));

        // A worker's blocker is a question owed by the planner; the
        // planner's answer clears `blocked` and keeps the claim.
        store
            .report_blocker("worker", &a, "The rates API needs a key I do not have.", &[])
            .await
            .expect("blocker");
        let t = store.feature_tree(&tree.id).await.unwrap();
        let ma = t.modules.iter().find(|m| m.id == a).unwrap();
        assert_eq!(ma.status, "blocked");
        let blocker = ma.open_questions.first().expect("question");
        assert_eq!(blocker.assigned_to.as_deref(), Some("manager"));
        assert_eq!(blocker.author, "worker");
        store
            .answer_question("manager", &blocker.id, "Key is in the vault under rates/.", vec![])
            .await
            .expect("answer blocker");
        let t = store.feature_tree(&tree.id).await.unwrap();
        let ma = t.modules.iter().find(|m| m.id == a).unwrap();
        assert_eq!(ma.status, "in_progress");
        assert_eq!(ma.claimed_by.as_deref(), Some("worker"));

        // Withdrawing an open question releases too; an answer stays.
        let wq = store.ask_question("worker", &a, "Never mind?", None, vec![]).await.unwrap();
        assert_eq!(store.feature_tree(&tree.id).await.unwrap().modules.iter().find(|m| m.id == a).unwrap().status, "blocked");
        store.delete_comment("worker", &wq.id).await.expect("withdraw");
        assert_eq!(store.feature_tree(&tree.id).await.unwrap().modules.iter().find(|m| m.id == a).unwrap().status, "in_progress");
        assert_eq!(store.delete_comment("manager", &ans.id).await.expect_err("history").code, ErrorCode::PlanConflict);
        assert_eq!(store.delete_comment("worker", &note.id).await.expect_err("not author").code, ErrorCode::Forbidden);

        // Deleting the note takes its file.
        assert_eq!(files.len(), 1);
        store.delete_comment("human", &note.id).await.expect("delete note");
        assert!(files.is_empty());

        // The thread reads oldest first and pages by "since".
        let thread = store.comments_of(&a, None, 100).await.unwrap();
        assert!(thread.windows(2).all(|w| w[0].created_at <= w[1].created_at));
        let later = store.comments_of(&a, Some(&q.id), 100).await.unwrap();
        assert!(later.iter().all(|c| c.created_at > q.created_at));
    })
    .await;
}

#[tokio::test]
async fn a_want_with_an_open_question_is_not_promotable() {
    with_scratch("disc_want", |store, files| async move {
        let want = store.add_want("human", "exports in the old format", &[]).await.unwrap();
        let q = store
            .ask_question("manager", &want.id, "Which old format — 2019 or 2022?", Some("human"), vec![])
            .await
            .unwrap();
        let err = store
            .promote_wants(
                "manager",
                PromoteWants {
                    wants: vec![WantRef { id: want.id.clone(), rationale: String::new() }],
                    feature_id: None,
                    plan: Some(plan("Exports")),
                },
            )
            .await
            .expect_err("unsettled");
        assert_eq!(err.code, ErrorCode::PendingResolution);
        store
            .answer_question("human", &q.id, "2022.", vec![file("sample-2022.csv", b"a,b")])
            .await
            .unwrap();
        let promotion = store
            .promote_wants(
                "manager",
                PromoteWants {
                    wants: vec![WantRef { id: want.id.clone(), rationale: String::new() }],
                    feature_id: None,
                    plan: Some(plan("Exports")),
                },
            )
            .await
            .expect("promotable now");
        // The answer's file reaches the feature through the want.
        let listed = store.attachments_of(&promotion.feature.id).await.unwrap();
        assert_eq!(listed.len(), 1);
        assert!(listed[0].via_want.is_some());
        assert!(listed[0].comment.is_some());

        // Deleting the plan leaves the want's discussion; deleting the
        // want takes it, files and all.
        store.delete_feature("manager", &promotion.feature.id).await.unwrap();
        assert_eq!(store.comments_of(&want.id, None, 10).await.unwrap().len(), 2);
        store.delete_want("human", &want.id).await.unwrap();
        assert!(files.is_empty());
        assert!(store.open_questions().await.unwrap().is_empty());
    })
    .await;
}

#[tokio::test]
async fn a_delegated_identity_talks_only_on_its_module() {
    with_scratch("disc_deleg", |store, _files| async move {
        let tree = store.plan_feature("manager", plan("Exports")).await.unwrap();
        let a = tree.modules[0].id.clone();
        let sub = Actor::delegated("sub", &a);
        store.add_comment(&sub, &a, "Progress: half way.", vec![]).await.expect("own module");
        let err = store.add_comment(&sub, &tree.id, "Hi", vec![]).await.expect_err("feature");
        assert_eq!(err.code, ErrorCode::Forbidden);
        // Removing the module through a plan revision takes its
        // discussion with it.
        store
            .revise_plan("manager", &tree.id, vec![PlanOp::Remove { id: a.clone() }])
            .await
            .expect("remove");
        assert_eq!(store.comments_of(&a, None, 10).await.unwrap().len(), 0);
    })
    .await;
}
