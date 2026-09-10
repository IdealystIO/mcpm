//! The knowledge base: project scope, kinds, and the three retrieval
//! routes.
//!
//! The search here is the one part of this system whose failure is
//! silent and total — a query that returns nothing looks exactly like a
//! project that knows nothing, and an agent has no way to tell the
//! difference. So each route gets a test that would fail if that route
//! stopped contributing, written as the question a caller would
//! actually ask rather than as the lexemes it happens to produce.

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
    let db_name = format!(
        "mcpm_{tag}_{}",
        &uuid::Uuid::new_v4().simple().to_string()[..12]
    );
    let mut admin = PgConnection::connect(&base)
        .await
        .expect("connect to the devcontainer database (is it up on 55432?)");
    admin
        .execute(format!("CREATE DATABASE {db_name}").as_str())
        .await
        .expect("create scratch database");
    let (head, _) = base.rsplit_once('/').expect("url has a database segment");
    {
        let store = Store::connect(&format!("{head}/{db_name}"))
            .await
            .expect("connect + migrate");
        store
            .ensure_project("scratch", "", "test project")
            .await
            .expect("project row");
        body(store).await;
    }
    admin
        .execute(format!("DROP DATABASE {db_name} WITH (FORCE)").as_str())
        .await
        .expect("drop scratch database");
}

/// Convenience: search by text alone.
async fn ask(store: &Store, text: &str) -> Vec<String> {
    store
        .search_memory(&MemoryQuery { text: text.into(), limit: 20, ..Default::default() })
        .await
        .expect("search")
        .hits
        .into_iter()
        .map(|h| h.memory.content)
        .collect()
}

/// Project scope is the whole point of the change: knowledge that is
/// not about any one feature needs somewhere to live that isn't a
/// feature.
#[tokio::test]
async fn knowledge_can_be_filed_above_the_work_tree() {
    with_scratch("kbproject", |store| async move {
        store.get_context("planner", "manager").await.expect("register");

        let mem = store
            .commit_memory(
                "planner",
                // No id given — project scope is a singleton and the
                // store fills it in.
                MemoryScope { level: Level::Project, id: String::new() },
                MemoryKind::Convention,
                "Migrations are additive; never rewrite an applied one.",
                &["database".into()],
                &[],
            )
            .await
            .expect("commit at project scope");

        assert_eq!(mem.level, "project");
        assert_eq!(mem.subject_id, PROJECT_SUBJECT);
        assert_eq!(mem.kind, MemoryKind::Convention);
        // The subject reads as the project, not as a bare id.
        assert_eq!(mem.subject_name, "scratch");

        let found = ask(&store, "migrations").await;
        assert_eq!(found.len(), 1, "project knowledge must be searchable: {found:?}");
    })
    .await;
}

/// A worker reading `up` from its module must reach the standing rules.
/// Before project scope existed there was nowhere to put them, so this
/// walk ended at the feature and house rules were invisible to anyone
/// not working the feature they happened to be filed under.
#[tokio::test]
async fn reading_upward_from_a_module_reaches_the_project_shelf() {
    with_scratch("kbup", |store| async move {
        store.get_context("planner", "manager").await.expect("register");
        let tree = store
            .plan_feature(
                "planner",
                PlanFeature {
                    name: "Exports".into(),
                    modules: vec![PlanModule {
                        name: "CSV".into(),
                        description: "d".into(),
                        tasks: vec!["t".into()],
                        ..Default::default()
                    }],
                    ..Default::default()
                },
            )
            .await
            .expect("plan");
        let module_id = tree.modules[0].id.clone();

        store
            .commit_memory(
                "planner",
                MemoryScope::project(),
                MemoryKind::Convention,
                "All timestamps are stored UTC.",
                &[],
                &[],
            )
            .await
            .expect("project convention");
        store
            .commit_memory(
                "planner",
                MemoryScope { level: Level::Feature, id: tree.id.clone() },
                MemoryKind::Decision,
                "Exports stream rather than buffer.",
                &[],
                &[],
            )
            .await
            .expect("feature decision");

        let up = store
            .search_memory(&MemoryQuery {
                scope: Some(MemoryScope { level: Level::Module, id: module_id.clone() }),
                direction: SearchDirection::Up,
                limit: 50,
                ..Default::default()
            })
            .await
            .expect("search up");
        let contents: Vec<&str> = up.hits.iter().map(|h| h.memory.content.as_str()).collect();
        assert!(
            contents.iter().any(|c| c.contains("UTC")),
            "reading up must reach project scope, got {contents:?}"
        );
        assert!(
            contents.iter().any(|c| c.contains("stream")),
            "and must still reach the feature, got {contents:?}"
        );

        // The claim briefing rides the same walk, so a worker is handed
        // the house rules without asking for them.
        store.get_context("worker", "worker").await.expect("register");
        let briefing = store.claim_module("worker", &module_id).await.expect("claim");
        assert!(
            briefing
                .ancestor_memories
                .iter()
                .any(|m| m.content.contains("UTC")),
            "the claim briefing must carry project conventions"
        );
    })
    .await;
}

/// The three retrieval routes, each tested by the way it alone can
/// succeed.
#[tokio::test]
async fn loose_language_finds_what_exact_lexemes_would_miss() {
    with_scratch("kbsearch", |store| async move {
        store.get_context("planner", "manager").await.expect("register");
        for (kind, content) in [
            (MemoryKind::Convention, "Schema changes must be additive and reversible."),
            (MemoryKind::Gotcha, "The postgres connection pool caps at four."),
            (MemoryKind::Decision, "We chose sqlx over diesel for compile-time checking."),
        ] {
            store
                .commit_memory("planner", MemoryScope::project(), kind, content, &[], &[])
                .await
                .expect("commit");
        }

        // 1. Synonyms: "db" is in no note; the table maps it to postgres.
        let hits = ask(&store, "db pool").await;
        assert!(
            hits.iter().any(|c| c.contains("connection pool")),
            "synonym expansion must reach 'postgres' from 'db', got {hits:?}"
        );

        // 2. Any-term: a four-word question where only some words land.
        // Under strict AND this returned nothing at all, which made
        // asking in sentences strictly worse than asking in keywords.
        let hits = ask(&store, "how do we handle schema changes").await;
        assert!(
            hits.iter().any(|c| c.contains("additive")),
            "a partial match must still surface, got {hits:?}"
        );

        // 3. Trigrams: a typo no stemmer recovers from.
        let hits = ask(&store, "diesle").await;
        assert!(
            hits.iter().any(|c| c.contains("diesel")),
            "fuzzy matching must survive a transposition, got {hits:?}"
        );

        // Ranking: the note matching every term outranks partial ones.
        let ranked = store
            .search_memory(&MemoryQuery {
                text: "sqlx diesel".into(),
                limit: 20,
                ..Default::default()
            })
            .await
            .expect("search");
        assert!(ranked.hits[0].memory.content.contains("sqlx over diesel"));
    })
    .await;
}

/// Every facet removes rows; none adds any. A filter that widened would
/// break the caller's ability to reason about a query by reading it.
#[tokio::test]
async fn every_facet_narrows_and_they_compose() {
    with_scratch("kbfacet", |store| async move {
        store.get_context("alice", "manager").await.expect("register");
        store.get_context("bob", "worker").await.expect("register");

        store
            .commit_memory(
                "alice",
                MemoryScope::project(),
                MemoryKind::Convention,
                "Prefer idea-ui components over bespoke UI.",
                &["ui".into(), "style".into()],
                &[],
            )
            .await
            .expect("commit");
        store
            .commit_memory(
                "bob",
                MemoryScope::project(),
                MemoryKind::Gotcha,
                "Unregistered scene payloads panic at realize, not at build.",
                &["ui".into()],
                &[],
            )
            .await
            .expect("commit");

        let all = store
            .search_memory(&MemoryQuery { limit: 50, ..Default::default() })
            .await
            .expect("all");
        assert_eq!(all.total, 2);

        let by_kind = store
            .search_memory(&MemoryQuery {
                kinds: vec![MemoryKind::Gotcha],
                limit: 50,
                ..Default::default()
            })
            .await
            .expect("by kind");
        assert_eq!(by_kind.total, 1);
        assert_eq!(by_kind.hits[0].memory.author, "bob");

        let by_author = store
            .search_memory(&MemoryQuery {
                author: Some("alice".into()),
                limit: 50,
                ..Default::default()
            })
            .await
            .expect("by author");
        assert_eq!(by_author.total, 1);

        // Tags are AND, not OR: both must be present.
        let both_tags = store
            .search_memory(&MemoryQuery {
                tags: vec!["ui".into(), "style".into()],
                limit: 50,
                ..Default::default()
            })
            .await
            .expect("by tags");
        assert_eq!(both_tags.total, 1, "every tag must be present, not any");

        // Composed: a kind that exists and an author who did not write
        // it is empty, not either one alone.
        let composed = store
            .search_memory(&MemoryQuery {
                kinds: vec![MemoryKind::Gotcha],
                author: Some("alice".into()),
                limit: 50,
                ..Default::default()
            })
            .await
            .expect("composed");
        assert_eq!(composed.total, 0, "facets compose by narrowing");
    })
    .await;
}

/// The page and its total come from one predicate, and the total counts
/// matches rather than the page — otherwise a pager says "1–20 of 20"
/// forever and the reader never learns there is a second page.
#[tokio::test]
async fn a_page_reports_the_whole_match_not_its_own_length() {
    with_scratch("kbpage", |store| async move {
        store.get_context("planner", "manager").await.expect("register");
        for i in 0..25 {
            store
                .commit_memory(
                    "planner",
                    MemoryScope::project(),
                    MemoryKind::Note,
                    &format!("Entry number {i} about deployment."),
                    &[],
                &[],
            )
                .await
                .expect("commit");
        }

        let first = store
            .search_memory(&MemoryQuery { limit: 10, ..Default::default() })
            .await
            .expect("page 1");
        assert_eq!(first.hits.len(), 10);
        assert_eq!(first.total, 25);

        let last = store
            .search_memory(&MemoryQuery { limit: 10, offset: 20, ..Default::default() })
            .await
            .expect("page 3");
        assert_eq!(last.hits.len(), 5);
        assert_eq!(last.total, 25, "the total describes the match, not the page");
    })
    .await;
}

/// Caller text becomes part of a tsquery expression, so the operators
/// that expression uses must not survive the trip. An unescaped `&` or
/// `!` there is a 500, and a `:` silently becomes a weight selector.
#[tokio::test]
async fn query_syntax_in_the_search_text_cannot_reach_the_parser() {
    with_scratch("kbinject", |store| async move {
        store.get_context("planner", "manager").await.expect("register");
        store
            .commit_memory(
                "planner",
                MemoryScope::project(),
                MemoryKind::Note,
                "Deployment uses a bearer token.",
                &[],
                &[],
            )
            .await
            .expect("commit");

        for hostile in [
            "deployment & token",
            "deployment | token",
            "!deployment",
            "deployment:*:*",
            "(((",
            "'; DROP TABLE memories; --",
            "\\\\",
            "   ",
        ] {
            let out = store
                .search_memory(&MemoryQuery {
                    text: hostile.into(),
                    limit: 10,
                    ..Default::default()
                })
                .await;
            assert!(out.is_ok(), "query {hostile:?} must not error: {:?}", out.err());
        }

        // And the table is still there.
        let after = store
            .search_memory(&MemoryQuery { limit: 10, ..Default::default() })
            .await
            .expect("search");
        assert_eq!(after.total, 1);
    })
    .await;
}
