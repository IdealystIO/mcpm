//! Files attached to features and wants: where the bytes go, who may
//! attach, what a feature inherits from the wants it was composed
//! from, and what deletion takes with it.
//!
//! The object store is `MemoryFiles`, so these run with only Postgres
//! up: set DATABASE_URL (defaults to the devcontainer database on host
//! port 55432). Each test creates and drops its own scratch database.

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
        modules: vec![PlanModule {
            name: "Only".into(),
            description: String::new(),
            tasks: vec!["do it".into()],
            depends_on: vec![],
            owns: vec![],
        }],
        stages: vec![],
    }
}

#[tokio::test]
async fn attach_describe_remove_round_trip() {
    with_scratch("att", |store, files| async move {
        let tree = store.plan_feature("planner", plan("Exports")).await.expect("plan");
        let bytes = b"col_a,col_b\n1,2\n".to_vec();

        let att = store
            .attach_file(
                "planner",
                &tree.id,
                "sample.csv",
                "A representative export row set.",
                "text/csv",
                bytes.clone(),
            )
            .await
            .expect("attach");
        assert_eq!(att.level, "feature");
        assert_eq!(att.subject_id, tree.id);
        assert_eq!(att.subject_name, "Exports");
        assert_eq!(att.name, "sample.csv");
        assert_eq!(att.size_bytes, bytes.len() as i64);
        assert_eq!(att.content_type, "text/csv");
        assert!(att.via_want.is_none());
        assert_eq!(files.len(), 1, "the object landed");

        // The bytes come back whole, and the record with them.
        let (view, got) = store.attachment_bytes(&att.id).await.expect("bytes");
        assert_eq!(view.id, att.id);
        assert_eq!(got, bytes);
        // The memory provider mints no links; a caller then serves
        // bytes itself.
        assert!(store.attachment_link(&att.id).await.expect("link").is_none());

        // The tree carries it, so `feature_status` readers see it.
        let tree = store.feature_tree(&tree.id).await.expect("tree");
        assert_eq!(tree.attachments.len(), 1);
        assert_eq!(tree.attachments[0].description, "A representative export row set.");

        // A worker's briefing carries it too.
        let module = &tree.modules[0].id;
        let briefing = store.claim_module("worker", module).await.expect("claim");
        assert_eq!(briefing.attachments.len(), 1);
        assert_eq!(briefing.attachments[0].name, "sample.csv");

        // Describing rewrites only the description.
        let described = store
            .describe_attachment("planner", &att.id, "Columns a and b; b is the total.")
            .await
            .expect("describe");
        assert_eq!(described.description, "Columns a and b; b is the total.");
        assert_eq!(described.sha256, att.sha256);

        // Removing takes the object down after the row.
        store.remove_attachment("planner", &att.id).await.expect("remove");
        assert!(files.is_empty(), "the object went with the row");
        assert_eq!(store.attachment(&att.id).await.expect_err("gone").code, ErrorCode::NotFound);
        assert!(store.attachments_of(&tree.id).await.expect("list").is_empty());

        // Every step left a trace.
        let kinds: Vec<String> = store
            .get_events(Some(&tree.id), 0, 100)
            .await
            .expect("events")
            .into_iter()
            .map(|e| e.kind)
            .collect();
        for kind in ["attachment_added", "attachment_described", "attachment_removed"] {
            assert!(kinds.contains(&kind.to_string()), "missing {kind} in {kinds:?}");
        }
    })
    .await;
}

#[tokio::test]
async fn a_feature_inherits_its_wants_files_without_copying() {
    with_scratch("att_want", |store, files| async move {
        let want = store
            .add_want("human", "the export should look like the old report", &[])
            .await
            .expect("want");
        let att = store
            .attach_file(
                "human",
                &want.id,
                "old-report.pdf",
                "What the old report looked like.",
                "application/pdf",
                vec![1, 2, 3],
            )
            .await
            .expect("attach to want");
        assert_eq!(att.level, "want");
        assert_eq!(att.subject_name, "the export should look like the old report");
        // A want's own listing does not mark an origin.
        let own = store.attachments_of(&want.id).await.expect("list");
        assert_eq!(own.len(), 1);
        assert!(own[0].via_want.is_none());

        let promotion = store
            .promote_wants(
                "composer",
                PromoteWants {
                    wants: vec![WantRef { id: want.id.clone(), rationale: "the look".into() }],
                    feature_id: None,
                    plan: Some(plan("Exports")),
                },
            )
            .await
            .expect("promote");
        let feature_id = promotion.feature.id.clone();

        // The feature's own file lists first; the want's follows,
        // marked with where it came from.
        store
            .attach_file("planner", &feature_id, "spec.md", "The spec.", "text/markdown", b"# spec".to_vec())
            .await
            .expect("attach to feature");
        let listed = store.attachments_of(&feature_id).await.expect("list");
        assert_eq!(listed.len(), 2);
        assert_eq!(listed[0].name, "spec.md");
        assert!(listed[0].via_want.is_none());
        assert_eq!(listed[1].name, "old-report.pdf");
        assert_eq!(listed[1].via_want.as_ref().map(|w| w.id.as_str()), Some(want.id.as_str()));
        assert_eq!(files.len(), 2, "nothing was copied");

        // Deleting the plan takes only the feature's own file; the
        // want (loose again) keeps its.
        store.delete_feature("planner", &feature_id).await.expect("delete plan");
        assert_eq!(files.len(), 1);
        assert_eq!(store.attachments_of(&want.id).await.expect("list").len(), 1);

        // And deleting the want takes the last one.
        store.delete_want("human", &want.id).await.expect("delete want");
        assert!(files.is_empty());
    })
    .await;
}

#[tokio::test]
async fn refusals_leave_nothing_behind() {
    with_scratch("att_refuse", |store, files| async move {
        let tree = store.plan_feature("planner", plan("Exports")).await.expect("plan");

        // An empty file, a nameless file, a subject that is not a
        // feature, want or module, a subject that does not exist.
        for (subject, name, bytes) in [
            (tree.id.as_str(), "x.txt", Vec::new()),
            (tree.id.as_str(), "  ", b"hi".to_vec()),
            (tree.modules[0].tasks[0].id.as_str(), "x.txt", b"hi".to_vec()),
            ("feat_nope", "x.txt", b"hi".to_vec()),
        ] {
            store
                .attach_file("planner", subject, name, "", "text/plain", bytes)
                .await
                .expect_err("refused");
        }
        assert!(files.is_empty(), "no refusal stored an object");

        // A delegated identity reads attachments in its briefing but
        // cannot change them.
        let delegated = Actor::delegated("sub", &tree.modules[0].id);
        let err = store
            .attach_file(&delegated, &tree.id, "x.txt", "", "text/plain", b"hi".to_vec())
            .await
            .expect_err("delegated");
        assert_eq!(err.code, ErrorCode::Forbidden);

        // A file name is a display name: a path collapses to its last
        // segment, and a query-breaking character is replaced.
        let att = store
            .attach_file("planner", &tree.id, "/tmp/some dir/report v2?.pdf", "", "application/pdf", b"%PDF".to_vec())
            .await
            .expect("attach");
        assert_eq!(att.name, "report v2_.pdf");
    })
    .await;
}
