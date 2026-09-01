//! The memory graph: immutability, supersession, evidence and standing.
//!
//! The rules under test are the ones a caller can violate by accident
//! and not notice — a memory quietly mutated, a correction that leaves
//! both versions reading as current, an agent grading its own work, or
//! a decayed entry that has become unfindable rather than merely
//! lower-ranked. Each of those is silent in production.

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
    let db = format!("mcpm_{tag}_{}", &uuid::Uuid::new_v4().simple().to_string()[..12]);
    let mut admin = PgConnection::connect(&base)
        .await
        .expect("connect to the devcontainer database (is it up on 55432?)");
    admin.execute(format!("CREATE DATABASE {db}").as_str()).await.expect("create");
    let (head, _) = base.rsplit_once('/').expect("url has a database segment");
    {
        let store = Store::connect(&format!("{head}/{db}")).await.expect("connect + migrate");
        store.ensure_project("scratch", "", "test").await.expect("project");
        body(store).await;
    }
    admin.execute(format!("DROP DATABASE {db} WITH (FORCE)").as_str()).await.expect("drop");
}

async fn commit(store: &Store, who: &str, text: &str, sup: &[Supersede]) -> Memory {
    store
        .commit_memory(who, MemoryScope::project(), MemoryKind::Convention, text, &[], sup)
        .await
        .expect("commit")
}

async fn find(store: &Store, text: &str, include: bool) -> Vec<Memory> {
    store
        .search_memory(&MemoryQuery {
            text: text.into(),
            limit: 50,
            include_superseded: include,
            ..Default::default()
        })
        .await
        .expect("search")
        .hits
        .into_iter()
        .map(|h| h.memory)
        .collect()
}

/// The central rule. A correction is a new memory; the old one leaves
/// default results and stays readable.
#[tokio::test]
async fn a_correction_supersedes_and_never_deletes() {
    with_scratch("mgsup", |store| async move {
        store.get_context("alice", "manager").await.expect("register");
        let v1 = commit(&store, "alice", "Exports use library X.", &[]).await;
        assert_eq!(v1.state, MemoryState::Current);

        let v2 = commit(
            &store,
            "alice",
            "Exports use library Y.",
            &[Supersede {
                memory_id: v1.id.clone(),
                kind: EdgeKind::Replaces,
                rationale: "X changed its licence.".into(),
            }],
        )
        .await;
        assert_eq!(v2.state, MemoryState::Current);

        // Default results carry only the current answer.
        let default = find(&store, "exports library", false).await;
        assert!(default.iter().any(|m| m.id == v2.id));
        assert!(
            !default.iter().any(|m| m.id == v1.id),
            "the superseded version must leave default results"
        );

        // But it is still there, and knows what it is.
        let all = find(&store, "exports library", true).await;
        let old = all.iter().find(|m| m.id == v1.id).expect("still searchable");
        assert_eq!(old.state, MemoryState::Superseded);

        // And the lineage carries the reason, which lives on neither
        // memory.
        let history = store.memory_history(&v2.id, true).await.expect("history");
        assert_eq!(history.supersedes.len(), 1);
        assert_eq!(history.supersedes[0].memory.id, v1.id);
        assert_eq!(history.supersedes[0].via, Some(EdgeKind::Replaces));
        assert!(history.supersedes[0].rationale.contains("licence"));

        // Walking the other way from the old entry finds its replacement.
        let back = store.memory_history(&v1.id, true).await.expect("history");
        assert_eq!(back.superseded_by.len(), 1);
        assert_eq!(back.superseded_by[0].memory.id, v2.id);
    })
    .await;
}

/// A rewording is not a change of belief, and history says so — asking
/// what the project used to think must not return three phrasings of
/// one idea.
#[tokio::test]
async fn history_separates_rewording_from_changing_your_mind() {
    with_scratch("mghist", |store| async move {
        store.get_context("alice", "manager").await.expect("register");
        let v1 = commit(&store, "alice", "Migrations are additive.", &[]).await;
        let v2 = commit(
            &store,
            "alice",
            "Migrations are additive; never rewrite an applied one.",
            &[Supersede {
                memory_id: v1.id.clone(),
                kind: EdgeKind::Revises,
                rationale: "Spelled out the consequence.".into(),
            }],
        )
        .await;
        let v3 = commit(
            &store,
            "alice",
            "Migrations run through refinery; rewriting is fine before first deploy.",
            &[Supersede {
                memory_id: v2.id.clone(),
                kind: EdgeKind::Replaces,
                rationale: "Switched migration tooling.".into(),
            }],
        )
        .await;

        // The default question — what did we used to believe — sees one
        // change, not two.
        let belief = store.memory_history(&v3.id, true).await.expect("history");
        assert_eq!(
            belief.supersedes.len(),
            1,
            "the reword must collapse, got {:?}",
            belief.supersedes.iter().map(|s| &s.memory.content).collect::<Vec<_>>()
        );
        assert_eq!(belief.supersedes[0].memory.id, v2.id);

        // The full record is still available and holds every step.
        let full = store.memory_history(&v3.id, false).await.expect("history");
        assert_eq!(full.supersedes.len(), 2);
        assert_eq!(full.supersedes[1].memory.id, v1.id);
    })
    .await;
}

/// Evidence is attested, counted per distinct agent, and it moves rank.
#[tokio::test]
async fn evidence_accumulates_by_distinct_agent() {
    with_scratch("mgsig", |store| async move {
        for who in ["alice", "bob", "carol"] {
            store.get_context(who, "worker").await.expect("register");
        }
        let m = commit(&store, "alice", "Timestamps are stored UTC.", &[]).await;
        assert_eq!(m.standing.touches, 0);
        assert_eq!(m.standing.confirms, 0);

        // The same agent touching repeatedly is one agent's opinion.
        for _ in 0..5 {
            store
                .touch_memories("bob", &[m.id.clone()], "used it")
                .await
                .expect("touch");
        }
        let after = store.memory_by_id(&m.id).await.expect("read").expect("there");
        assert_eq!(after.standing.touches, 1, "distinct agents, not events");

        store.touch_memories("carol", &[m.id.clone()], "").await.expect("touch");
        let after = store.memory_by_id(&m.id).await.expect("read").expect("there");
        assert_eq!(after.standing.touches, 2);

        // A confirm outweighs a touch, and both raise the multiplier.
        let before = after.standing.multiplier;
        store.confirm_memory("bob", &m.id, "checked the schema").await.expect("confirm");
        let after = store.memory_by_id(&m.id).await.expect("read").expect("there");
        assert_eq!(after.standing.confirms, 1);
        assert!(
            after.standing.multiplier > before,
            "evidence must raise standing: {} -> {}",
            before,
            after.standing.multiplier
        );
    })
    .await;
}

/// Corroboration needs independence, and a dispute needs a reason.
#[tokio::test]
async fn an_agent_cannot_grade_its_own_memory() {
    with_scratch("mgself", |store| async move {
        store.get_context("alice", "manager").await.expect("register");
        store.get_context("bob", "worker").await.expect("register");
        let m = commit(&store, "alice", "The pool caps at four connections.", &[]).await;

        assert!(
            store.confirm_memory("alice", &m.id, "trust me").await.is_err(),
            "an agent must not confirm its own memory"
        );
        assert!(
            store.dispute_memory("alice", &m.id, "actually wrong").await.is_err(),
            "nor dispute it"
        );
        // Touching your own is fine — you may use what you wrote.
        store.touch_memories("alice", &[m.id.clone()], "").await.expect("self-touch is fine");

        // A dispute with no reason cannot be resolved by anyone else.
        assert!(store.dispute_memory("bob", &m.id, "   ").await.is_err());
        store.dispute_memory("bob", &m.id, "It caps at eight.").await.expect("dispute");

        let after = store.memory_by_id(&m.id).await.expect("read").expect("there");
        assert_eq!(after.standing.disputes, 1);
        assert_eq!(after.state, MemoryState::Disputed);
    })
    .await;
}

/// A dispute removes a memory from circulation without removing it from
/// the record — and enough confirmations outweigh a lone dissenter.
#[tokio::test]
async fn a_dispute_withdraws_but_does_not_delete() {
    with_scratch("mgdisp", |store| async move {
        for who in ["alice", "bob", "carol", "dan"] {
            store.get_context(who, "worker").await.expect("register");
        }
        let m = commit(&store, "alice", "Ports are allocated per project.", &[]).await;
        store.dispute_memory("bob", &m.id, "Not since the move.").await.expect("dispute");

        assert!(
            !find(&store, "ports allocated", false).await.iter().any(|x| x.id == m.id),
            "a disputed memory leaves default results"
        );
        assert!(
            find(&store, "ports allocated", true).await.iter().any(|x| x.id == m.id),
            "and stays findable when asked for"
        );

        // Two independent confirmations outweigh the single dispute, and
        // it returns on its own — no separate resolution step needed for
        // the common case.
        store.confirm_memory("carol", &m.id, "checked").await.expect("confirm");
        store.confirm_memory("dan", &m.id, "checked").await.expect("confirm");
        let back = store.memory_by_id(&m.id).await.expect("read").expect("there");
        assert_eq!(back.state, MemoryState::Current);
        assert!(find(&store, "ports allocated", false).await.iter().any(|x| x.id == m.id));
    })
    .await;
}

/// Decay lowers rank; it must never lower it to invisibility. This is
/// the arithmetic form of "never delete".
#[tokio::test]
async fn decay_never_makes_a_memory_unfindable() {
    with_scratch("mgfloor", |store| async move {
        store.get_context("alice", "manager").await.expect("register");
        let oldest = commit(&store, "alice", "The zeroth convention about kestrels.", &[]).await;

        // Bury it under a great many newer entries of the same kind.
        for i in 0..60 {
            commit(&store, "alice", &format!("Convention number {i} about other things."), &[])
                .await;
        }

        let aged = store.memory_by_id(&oldest.id).await.expect("read").expect("there");
        assert!(
            aged.standing.newer_fraction > 0.9,
            "it should be near the back of its kind, got {}",
            aged.standing.newer_fraction
        );
        assert!(
            aged.standing.multiplier > 0.0,
            "the multiplier must never reach zero, got {}",
            aged.standing.multiplier
        );

        // Searching its own words still finds it. Decay changed its
        // rank, not its existence.
        let hits = find(&store, "kestrels", false).await;
        assert!(
            hits.iter().any(|m| m.id == oldest.id),
            "an aged memory must still be findable by its own words"
        );
    })
    .await;
}

/// Retries are not new beliefs, and a supersession that changes nothing
/// is not a step in the lineage.
#[tokio::test]
async fn identical_writes_collapse_rather_than_duplicate() {
    with_scratch("mgidem", |store| async move {
        store.get_context("alice", "manager").await.expect("register");
        let a = commit(&store, "alice", "Bind to loopback by default.", &[]).await;
        let b = commit(&store, "alice", "Bind to loopback by default.", &[]).await;
        assert_eq!(a.id, b.id, "a retry must not become a second memory");

        let total = store
            .search_memory(&MemoryQuery { limit: 50, ..Default::default() })
            .await
            .expect("search")
            .total;
        assert_eq!(total, 1);

        // Superseding something with byte-identical content would add a
        // step to the lineage that changed nothing.
        let err = store
            .commit_memory(
                "alice",
                MemoryScope::project(),
                MemoryKind::Convention,
                "Bind to loopback by default.",
                &[],
                &[Supersede {
                    memory_id: a.id.clone(),
                    kind: EdgeKind::Revises,
                    rationale: String::new(),
                }],
            )
            .await;
        assert!(err.is_err(), "a no-op supersession must be refused");
    })
    .await;
}

/// Scoring reads the weights table, so retuning is a SQL statement and
/// takes effect on the next query — no backfill, and every entry stays
/// comparable because none of them stored a score.
#[tokio::test]
async fn reweighting_re_ranks_without_touching_a_single_memory() {
    with_scratch("mgweights", |store| async move {
        store.get_context("alice", "manager").await.expect("register");
        store.get_context("bob", "worker").await.expect("register");
        let m = commit(&store, "alice", "Weights are read at query time.", &[]).await;
        store.confirm_memory("bob", &m.id, "checked").await.expect("confirm");

        let before = store.memory_by_id(&m.id).await.expect("read").expect("there");

        sqlx::query("UPDATE knowledge_weights SET value = $1 WHERE name = 'signal_confirm'")
            .bind(4.0_f32)
            .execute(store.pool())
            .await
            .expect("retune");

        let after = store.memory_by_id(&m.id).await.expect("read").expect("there");
        assert!(
            after.standing.evidence > before.standing.evidence,
            "a weight change must re-rank existing entries: {} -> {}",
            before.standing.evidence,
            after.standing.evidence
        );
        // The memory itself did not change.
        assert_eq!(before.content, after.content);
        assert_eq!(before.created_at, after.created_at);
    })
    .await;
}

/// In history view the CURRENT entry must lead. Evidence a memory
/// accrued while it was true does not go away when it stops being true,
/// so without a state penalty a well-confirmed old belief outranks the
/// entry that corrected it — and the history view leads with the thing
/// that is no longer the case.
#[tokio::test]
async fn a_superseded_entry_cannot_outrank_its_own_replacement() {
    with_scratch("mgorder", |store| async move {
        store.get_context("alice", "manager").await.expect("register");
        store.get_context("bob", "worker").await.expect("register");

        let v1 = commit(&store, "alice", "Kestrels are stored in localStorage.", &[]).await;
        // v1 earns real standing while it is the current answer.
        store.touch_memories("bob", &[v1.id.clone()], "").await.expect("touch");
        store.confirm_memory("bob", &v1.id, "checked").await.expect("confirm");

        let v2 = commit(
            &store,
            "alice",
            "Kestrels are stored in the credentials vault.",
            &[Supersede {
                memory_id: v1.id.clone(),
                kind: EdgeKind::Replaces,
                rationale: "localStorage was a stopgap.".into(),
            }],
        )
        .await;

        let hits = find(&store, "kestrels stored", true).await;
        let at = |id: &str| hits.iter().position(|m| m.id == id).expect("present");
        assert!(
            at(&v2.id) < at(&v1.id),
            "the current entry must lead even in history view — got {:?}",
            hits.iter().map(|m| (&m.state, &m.content)).collect::<Vec<_>>()
        );

        // The old one is ranked lower, not hidden: still above the floor.
        let old = hits.iter().find(|m| m.id == v1.id).expect("still there");
        assert!(old.standing.multiplier > 0.0);
    })
    .await;
}

/// A refuted memory rides along in the claim briefing.
///
/// Heads-only is right for a plain replacement — the successor says
/// everything needed. It is wrong for a refutation: "we tried X and it
/// did not work" is the thing that stops a fresh agent proposing X, and
/// hiding it invites the crew to rediscover the same dead end.
#[tokio::test]
async fn a_worker_is_briefed_on_dead_ends_but_not_on_plain_replacements() {
    with_scratch("mgbrief", |store| async move {
        store.get_context("planner", "manager").await.expect("register");
        let tree = store
            .plan_feature(
                "planner",
                PlanFeature {
                    name: "Exports".into(),
                    description: String::new(),
                    stages: vec![PlanStage {
                        name: "Build".into(),
                        modules: vec![PlanModule {
                            name: "CSV".into(),
                            description: "d".into(),
                            tasks: vec!["t".into()],
                        }],
                    }],
                },
            )
            .await
            .expect("plan");
        let module_id = tree.stages[0].modules[0].id.clone();

        // A dead end: tried, and wrong.
        let dead = commit(&store, "planner", "Stream exports through the CSV crate.", &[]).await;
        commit(
            &store,
            "planner",
            "Write exports by hand; the CSV crate cannot do streaming.",
            &[Supersede {
                memory_id: dead.id.clone(),
                kind: EdgeKind::Refutes,
                rationale: "It buffers the whole file.".into(),
            }],
        )
        .await;

        // A plain replacement: superseded because the world moved.
        let moved = commit(&store, "planner", "Exports are written to /tmp.", &[]).await;
        commit(
            &store,
            "planner",
            "Exports are written to the object store.",
            &[Supersede {
                memory_id: moved.id.clone(),
                kind: EdgeKind::Replaces,
                rationale: "We got a bucket.".into(),
            }],
        )
        .await;

        store.get_context("worker", "worker").await.expect("register");
        let briefing = store.claim_module("worker", &module_id).await.expect("claim");
        let seen: Vec<&str> = briefing
            .ancestor_memories
            .iter()
            .map(|m| m.content.as_str())
            .collect();

        assert!(
            seen.iter().any(|c| c.contains("CSV crate cannot do streaming")),
            "the current answer must be there: {seen:?}"
        );
        assert!(
            seen.iter().any(|c| c.contains("Stream exports through the CSV crate")),
            "the REFUTED entry must ride along, or the worker retries the dead end: {seen:?}"
        );
        assert!(
            !seen.iter().any(|c| c.contains("written to /tmp")),
            "a plain replacement must NOT: its successor says everything needed: {seen:?}"
        );
    })
    .await;
}

/// Standing relations describe; they must never retire anything.
#[tokio::test]
async fn a_standing_relation_does_not_withdraw_its_target() {
    with_scratch("mgrel", |store| async move {
        store.get_context("alice", "manager").await.expect("register");
        let broad = commit(&store, "alice", "All ids are prefixed by kind.", &[]).await;
        let narrow = commit(&store, "alice", "Want ids use the want_ prefix.", &[]).await;

        store
            .relate_memories("alice", &narrow.id, &broad.id, EdgeKind::Refines, "The want case.")
            .await
            .expect("relate");

        // Both are still current, and both still in default results.
        for id in [&broad.id, &narrow.id] {
            let m = store.memory_by_id(id).await.expect("read").expect("there");
            assert_eq!(m.state, MemoryState::Current, "a relation must not retire anything");
        }
        assert_eq!(
            find(&store, "prefix ids", false).await.len(),
            2,
            "both must remain in default results"
        );

        // The relation shows from both ends, with its direction intact.
        let from_narrow = store.memory_history(&narrow.id, true).await.expect("history");
        assert_eq!(from_narrow.relations.len(), 1);
        assert!(from_narrow.relations[0].outgoing, "narrow refines broad");
        assert_eq!(from_narrow.relations[0].other.id, broad.id);

        let from_broad = store.memory_history(&broad.id, true).await.expect("history");
        assert_eq!(from_broad.relations.len(), 1);
        assert!(!from_broad.relations[0].outgoing, "read the other way round");

        // Supersession cannot be declared between two existing memories:
        // that is what keeps the history acyclic.
        assert!(
            store
                .relate_memories("alice", &narrow.id, &broad.id, EdgeKind::Replaces, "")
                .await
                .is_err(),
            "supersession is commit-time only"
        );
        assert!(store
            .relate_memories("alice", &broad.id, &broad.id, EdgeKind::RelatesTo, "")
            .await
            .is_err());
    })
    .await;
}

/// Co-use suggests; it never asserts.
#[tokio::test]
async fn memories_used_together_are_suggested_not_linked() {
    with_scratch("mgsug", |store| async move {
        store.get_context("alice", "manager").await.expect("register");
        store.get_context("bob", "worker").await.expect("register");
        store.get_context("carol", "worker").await.expect("register");

        let a = commit(&store, "alice", "The gate rejects a premature claim.", &[]).await;
        let b = commit(&store, "alice", "A rejection still commits its event.", &[]).await;
        let c = commit(&store, "alice", "Unrelated: fonts live in /fonts.", &[]).await;

        // Two agents lean on a and b together; nobody links them.
        for who in ["bob", "carol"] {
            store
                .touch_memories(who, &[a.id.clone(), b.id.clone()], "gate work")
                .await
                .expect("touch");
        }
        // c is used, but on its own — co-use is about the same breath.
        store.touch_memories("bob", &[c.id.clone()], "").await.expect("touch");

        let h = store.memory_history(&a.id, true).await.expect("history");
        assert!(h.relations.is_empty(), "nothing was declared, so nothing is asserted");
        assert_eq!(h.suggestions.len(), 1, "exactly the co-used one is suggested");
        assert_eq!(h.suggestions[0].other.id, b.id);
        assert_eq!(h.suggestions[0].co_touches, 2, "counted per distinct agent");

        // Once declared, it stops being a suggestion — a list that keeps
        // proposing what you already answered is one people stop reading.
        store
            .relate_memories("bob", &a.id, &b.id, EdgeKind::RelatesTo, "Same mechanism.")
            .await
            .expect("relate");
        let h = store.memory_history(&a.id, true).await.expect("history");
        assert_eq!(h.relations.len(), 1);
        assert!(h.suggestions.is_empty(), "a declared link is no longer a suggestion");
    })
    .await;
}

/// The exit doors attest, and a bad id there does not cost the work.
#[tokio::test]
async fn finishing_work_records_what_it_leaned_on() {
    with_scratch("mgexit", |store| async move {
        store.get_context("planner", "manager").await.expect("register");
        let tree = store
            .plan_feature(
                "planner",
                PlanFeature {
                    name: "Exports".into(),
                    description: String::new(),
                    stages: vec![PlanStage {
                        name: "Build".into(),
                        modules: vec![PlanModule {
                            name: "CSV".into(),
                            description: "d".into(),
                            tasks: vec![],
                        }],
                    }],
                },
            )
            .await
            .expect("plan");
        let module_id = tree.stages[0].modules[0].id.clone();
        let leaned_on = commit(&store, "planner", "Exports stream rather than buffer.", &[]).await;

        store.get_context("worker", "worker").await.expect("register");
        store.claim_module("worker", &module_id).await.expect("claim");
        store
            .complete_module(
                "worker",
                &module_id,
                "CSV export done.",
                // One real id and one that does not resolve: the work is
                // finished, and a typo must not fail the completion.
                &[leaned_on.id.clone(), "mem_deadbeef".into()],
            )
            .await
            .expect("a bad reference must not cost the completion");

        let after = store.memory_by_id(&leaned_on.id).await.expect("read").expect("there");
        assert_eq!(after.standing.touches, 1, "the exit door attested");
    })
    .await;
}
