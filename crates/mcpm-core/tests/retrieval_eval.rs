//! A retrieval eval: fixed questions, and the entry that should win.
//!
//! The scoring function has six tunable constants and no natural units.
//! Without a fixture, "is 0.15 the right touch weight" is taste with no
//! way to be wrong, and the weights ossify at whatever their first
//! author guessed. With one, a proposed change is a measurement.
//!
//! Two things this is NOT. It is not a test that the ranking is good —
//! it is a test that specific, argued cases come out right, which is
//! the most a fixture this size can claim. And it is not a reason to
//! chase 100%: a case that only passes with a contorted weight is
//! telling you the case is wrong, or that lexical retrieval cannot
//! reach it. `report_scores` prints the margin so a regression shows up
//! as a shrinking gap before it shows up as a failure.

use mcpm_core::*;
use sqlx::{Connection, Executor, PgConnection};

fn base_url() -> String {
    std::env::var("DATABASE_URL")
        .unwrap_or_else(|_| "postgres://app:app@localhost:55432/app".to_string())
}

/// One entry in the fixture corpus.
struct Entry {
    kind: MemoryKind,
    tags: &'static [&'static str],
    text: &'static str,
}

/// A corpus with the shape a real project's has: a few curated
/// conventions, several machine-written completion summaries, and some
/// near-misses that share vocabulary with the questions without
/// answering them.
const CORPUS: &[Entry] = &[
    Entry { kind: MemoryKind::Convention, tags: &["database"],
        text: "Migrations are additive. sqlx checksums every applied migration, so rewriting one in place makes an existing database refuse to start." },
    Entry { kind: MemoryKind::Convention, tags: &["database"],
        text: "All timestamps are stored and compared in UTC; format at the edge, never in the store." },
    Entry { kind: MemoryKind::Gotcha, tags: &["database"],
        text: "The event listener must open its own connection. Taking one from the four-connection query pool starves ordinary reads." },
    Entry { kind: MemoryKind::Decision, tags: &["database"],
        text: "We chose sqlx over diesel for compile-time checked queries without a DSL." },
    Entry { kind: MemoryKind::Gotcha, tags: &["ui"],
        text: "Unregistered scene payloads panic at realize rather than failing the build, and the table SDK only emits its payload on wasm." },
    Entry { kind: MemoryKind::Convention, tags: &["ui"],
        text: "Prefer idea-ui components and theme tokens over bespoke UI." },
    // Machine-written records. These are the ones that compete unfairly:
    // they mention everything, because they summarise everything.
    Entry { kind: MemoryKind::Outcome, tags: &["system", "summary"],
        text: "Store landed: Postgres schema, migrations, gate and claim transactions, the event ledger, UTC timestamps and full-text memories." },
    Entry { kind: MemoryKind::Outcome, tags: &["system", "summary"],
        text: "Console landed: board, tree, feed and graph views rendering the store over a polled snapshot." },
    Entry { kind: MemoryKind::Outcome, tags: &["system", "summary"],
        text: "MCP surface landed: tools, prompts and resources over stdio, sharing the database with the console." },
];

/// A question and the entry that should win it, with the reason the
/// case exists at all.
struct Case {
    question: &'static str,
    /// A distinctive fragment of the entry that must rank first.
    expect: &'static str,
    why: &'static str,
}

const CASES: &[Case] = &[
    Case {
        question: "how should I change the database schema",
        expect: "Migrations are additive",
        why: "a curated convention must beat a completion summary that merely mentions 'schema' and 'migrations'",
    },
    Case {
        question: "can I rewrite an old migration",
        expect: "Migrations are additive",
        why: "the answer uses none of the caller's words except 'migration' — synonyms and any-term matching carry it",
    },
    Case {
        question: "db connection pool",
        expect: "The event listener must open its own connection",
        why: "'db' must reach 'connection pool' through the synonym table",
    },
    Case {
        question: "diesle",
        expect: "We chose sqlx over diesel",
        why: "a transposition no stemmer recovers from; trigrams must carry it",
    },
    Case {
        question: "why did my component crash at runtime instead of failing to build",
        expect: "Unregistered scene payloads panic at realize",
        why: "a long natural question where only a few terms land",
    },
    Case {
        question: "what timezone are times stored in",
        expect: "All timestamps are stored and compared in UTC",
        why: "'timezone' and 'times' must reach 'timestamps' by stemming and trigram",
    },
];

/// The counter-case for `penalty_machine`.
///
/// Down-weighting machine records is only defensible if asking FOR them
/// still works. It does, and structurally rather than by luck: the
/// penalty applies equally to every `system`-tagged entry, so it is
/// order-preserving within that set and cannot change which record wins
/// a records question however large it grows.
///
/// This began as a ranked case — "what landed in the store module"
/// should return "Store landed" — and failed at every weight including
/// zero, because two completion summaries both mention landing and the
/// store and picking between them was arbitrary. The fixture was right
/// and the case was wrong; asserting reachability is the claim that can
/// actually be defended.
#[tokio::test]
async fn asking_for_machine_records_still_reaches_them() {
    with_corpus(|store| async move {
        let hits = store
            .search_memory(&MemoryQuery {
                text: "landed".into(),
                kinds: vec![MemoryKind::Outcome],
                limit: 10,
                ..Default::default()
            })
            .await
            .expect("search")
            .hits;
        assert_eq!(hits.len(), 3, "every completion summary must still be reachable");
        assert!(hits.iter().all(|h| h.memory.kind == MemoryKind::Outcome));
        assert!(
            hits.iter().all(|h| h.relevance > 0.0),
            "the penalty must never drive a record to zero"
        );
    })
    .await;
}

/// Seed the fixture corpus into a scratch database and run `body`.
async fn with_corpus<F, Fut>(body: F)
where
    F: FnOnce(Store) -> Fut,
    Fut: std::future::Future<Output = ()>,
{
    let base = base_url();
    let db = format!("mcpm_eval_{}", &uuid::Uuid::new_v4().simple().to_string()[..12]);
    let mut admin = PgConnection::connect(&base)
        .await
        .expect("connect to the devcontainer database (is it up on 55432?)");
    admin.execute(format!("CREATE DATABASE {db}").as_str()).await.expect("create");
    let (head, _) = base.rsplit_once('/').expect("url has a database segment");
    {
        let store = Store::connect(&format!("{head}/{db}")).await.expect("connect + migrate");
        store.ensure_project("eval", "", "retrieval fixture").await.expect("project");
        store.get_context("fixture", "manager").await.expect("register");
        for e in CORPUS {
            let tags: Vec<String> = e.tags.iter().map(|t| t.to_string()).collect();
            store
                .commit_memory("fixture", MemoryScope::project(), e.kind, e.text, &tags, &[])
                .await
                .expect("seed");
        }
        body(store).await;
    }
    admin.execute(format!("DROP DATABASE {db} WITH (FORCE)").as_str()).await.expect("drop");
}

#[tokio::test]
async fn the_ranking_answers_the_questions_it_is_meant_to() {
    let failures = std::sync::Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
    let sink = failures.clone();
    with_corpus(move |store| async move {
        let mut failures: Vec<String> = Vec::new();
        println!("\n  retrieval eval — {} cases over {} entries\n", CASES.len(), CORPUS.len());
        for case in CASES {
            let hits = store
                .search_memory(&MemoryQuery {
                    text: case.question.into(),
                    limit: 5,
                    ..Default::default()
                })
                .await
                .expect("search")
                .hits;

            let top = hits.first();
            let won = top.map(|h| h.memory.content.contains(case.expect)).unwrap_or(false);
            // The margin over the runner-up is the early-warning signal:
            // a change that halves it has moved the ranking even while
            // every case still passes.
            let margin = match (hits.first(), hits.get(1)) {
                (Some(a), Some(b)) => a.relevance - b.relevance,
                (Some(_), None) => f32::INFINITY,
                _ => 0.0,
            };
            println!(
                "  [{}] {:<58} margin {:>6}",
                if won { "ok" } else { "FAIL" },
                case.question,
                if margin.is_finite() { format!("{margin:.3}") } else { "sole".into() }
            );
            if !won {
                failures.push(format!(
                    "\n  question: {}\n  expected: {}\n  because:  {}\n  got:      {}",
                    case.question,
                    case.expect,
                    case.why,
                    hits.iter()
                        .take(3)
                        .map(|h| format!("\n              {:.3}  {}", h.relevance,
                            &h.memory.content[..h.memory.content.len().min(64)]))
                        .collect::<String>()
                ));
            }
        }
        println!();
        *sink.lock().unwrap() = failures;
    })
    .await;

    let failures = failures.lock().unwrap();
    assert!(
        failures.is_empty(),
        "{} of {} retrieval cases regressed:{}",
        failures.len(),
        CASES.len(),
        failures.join("")
    );
}
