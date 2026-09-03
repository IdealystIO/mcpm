//! Minted worker identities: what a delegation token may and may not do.
//!
//! One key is one MACHINE, and a subagent cannot present a different
//! one — so without delegation every subagent in a process tree IS that
//! machine, and three things collapse inside the tree while still
//! holding between trees: attribution, mutual exclusion between
//! siblings, and the manager/worker split. Each test below is one of
//! those coming back, or one of the rules that makes handing a token to
//! a subagent in plain text safe.
//!
//! Runs against a real Postgres: set DATABASE_URL (defaults to the
//! devcontainer database published on host port 55432). Each test
//! creates and drops its own scratch database.

use mcpm_core::*;
use sqlx::{Connection, Executor, PgConnection};

fn base_url() -> String {
    std::env::var("DATABASE_URL")
        .unwrap_or_else(|_| "postgres://app:app@localhost:55432/app".to_string())
}

/// Run `body` against a freshly migrated scratch database. The URL comes
/// back too: a couple of these tests have to age a row that no tool can
/// age, and reaching past the store is honest about that.
async fn with_scratch<F, Fut>(tag: &str, body: F)
where
    F: FnOnce(Store, String) -> Fut,
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
    let scratch_url = format!("{head}/{db_name}");

    {
        let store = Store::connect(&scratch_url).await.expect("connect + migrate");
        body(store, scratch_url.clone()).await;
    }

    admin
        .execute(format!("DROP DATABASE {db_name} WITH (FORCE)").as_str())
        .await
        .expect("drop scratch database");
}

/// One stage, two modules — the shape that makes sibling subagents
/// possible in the first place.
fn plan() -> PlanFeature {
    PlanFeature {
        name: "Delegation".into(),
        description: "Two concurrent modules in one stage.".into(),
        stages: vec![PlanStage {
            name: "Build".into(),
            modules: vec![
                PlanModule {
                    name: "Schema".into(),
                    description: "Tables".into(),
                    tasks: vec!["Write migration".into()],
                },
                PlanModule {
                    name: "API".into(),
                    description: "Endpoints".into(),
                    tasks: vec!["Write handler".into()],
                },
            ],
        }],
    }
}

/// Plan the feature and return `(feature_id, [module ids])`.
async fn seed(store: &Store) -> (String, Vec<String>) {
    let tree = store.plan_feature("laptop", plan()).await.expect("plan");
    let modules = tree.stages[0]
        .modules
        .iter()
        .map(|m| m.id.clone())
        .collect();
    (tree.id, modules)
}

async fn manager_key(store: &Store) -> IssuedKey {
    store
        .issue_key("operator", "Laptop", "laptop", KeyRole::Manager)
        .await
        .expect("issue")
}

// ---------------------------------------------------------------------

/// Attribution: the loss that stings most day to day. Every write a
/// delegated subagent makes must read back as the subagent, not as the
/// machine whose key carried it — including the module summary, which
/// is the thing the next stage's workers actually read.
#[tokio::test]
async fn a_minted_identity_signs_its_own_work() {
    with_scratch("dlgsign", |store, _url| async move {
        let (feature_id, modules) = seed(&store).await;
        let key = manager_key(&store).await;

        let minted = store
            .mint_worker(
                &Actor::new("laptop"),
                Some(&key.info.id),
                &modules[0],
                "agent.mod.schema",
                None,
            )
            .await
            .expect("mint");
        assert_eq!(minted.agent_name, "agent.mod.schema");
        assert_eq!(minted.module_id, modules[0]);

        let who = store
            .resolve_delegation(Some(&key.info.id), &minted.delegation_token)
            .await
            .expect("resolve");
        let actor = Actor::delegated(&who.agent_name, &who.module_id);

        let briefing = store.claim_module(&actor, &modules[0]).await.expect("claim");
        store
            .complete_task(&actor, &briefing.module.tasks[0].id, TaskOutcome::Done, None)
            .await
            .expect("task");
        store
            .complete_module(&actor, &modules[0], "Tables landed.", &[])
            .await
            .expect("complete");

        let events = store.get_events(Some(&feature_id), 0, 200).await.expect("events");
        let signed: Vec<&str> = events
            .iter()
            .filter(|e| ["module_claimed", "task_done", "module_done"].contains(&e.kind.as_str()))
            .map(|e| e.agent.as_deref().unwrap_or(""))
            .collect();
        assert_eq!(
            signed,
            ["agent.mod.schema", "agent.mod.schema", "agent.mod.schema"],
            "every write of a delegated worker is recorded as the worker, not as 'laptop'"
        );

        // The minting itself stays the manager's act, and the token is
        // never in the ledger — an event feed is read by everything with
        // console access.
        let mint_event = events
            .iter()
            .find(|e| e.kind == "worker_minted")
            .expect("worker_minted recorded");
        assert_eq!(mint_event.agent.as_deref(), Some("laptop"));
        let payload = serde_json::to_string(&mint_event.payload).unwrap();
        assert!(
            !payload.contains(&minted.delegation_token),
            "the token must never reach the event ledger"
        );
    })
    .await;
}

/// The pairing rule, which is what makes in-band delegation safe: a
/// token is only honoured alongside the key that minted it. Off that
/// machine it is inert, so pasting it into a prompt costs nothing.
#[tokio::test]
async fn a_token_is_inert_without_the_key_that_minted_it() {
    with_scratch("dlgpair", |store, _url| async move {
        let (_feature_id, modules) = seed(&store).await;
        let laptop = manager_key(&store).await;
        let other = store
            .issue_key("operator", "Branch box", "box-01", KeyRole::Worker)
            .await
            .expect("issue");

        let minted = store
            .mint_worker(
                &Actor::new("laptop"),
                Some(&laptop.info.id),
                &modules[0],
                "agent.mod.schema",
                None,
            )
            .await
            .expect("mint");

        store
            .resolve_delegation(Some(&laptop.info.id), &minted.delegation_token)
            .await
            .expect("the minting key resolves it");

        for wrong in [Some(other.info.id.as_str()), None] {
            let err = store
                .resolve_delegation(wrong, &minted.delegation_token)
                .await
                .expect_err("another key — or none — must not resolve it");
            assert_eq!(err.code, ErrorCode::Unauthorized);
        }

        // A key is not a delegation token and vice versa: both are
        // refused on shape, before any lookup.
        let err = store
            .resolve_delegation(Some(&laptop.info.id), &laptop.token)
            .await
            .expect_err("an API key is not a delegation token");
        assert_eq!(err.code, ErrorCode::Unauthorized);
    })
    .await;
}

/// Scope: the token names ONE module. A subagent that gets confused —
/// or is told to be helpful — cannot wander into a sibling's work, even
/// though the key behind it holds every claim on the board.
#[tokio::test]
async fn a_delegation_is_confined_to_its_module() {
    with_scratch("dlgscope", |store, _url| async move {
        let (_feature_id, modules) = seed(&store).await;
        let key = manager_key(&store).await;
        let minted = store
            .mint_worker(
                &Actor::new("laptop"),
                Some(&key.info.id),
                &modules[0],
                "agent.mod.schema",
                None,
            )
            .await
            .expect("mint");
        let actor = Actor::delegated(&minted.agent_name, &minted.module_id);

        let err = store
            .claim_module(&actor, &modules[1])
            .await
            .expect_err("a scoped identity cannot claim a sibling module");
        assert_eq!(err.code, ErrorCode::Forbidden);

        // And not by any other door either: the scope check sits with
        // the claim check, so every write verb inherits it.
        let err = store
            .add_task(&actor, &modules[1], "sneak in", None)
            .await
            .expect_err("nor write to one");
        assert_eq!(err.code, ErrorCode::Forbidden);
    })
    .await;
}

/// Mutual exclusion, restored. Before delegation two subagents on one
/// key were ONE identity, so each could complete the other's module.
#[tokio::test]
async fn siblings_cannot_complete_each_others_modules() {
    with_scratch("dlgsibs", |store, _url| async move {
        let (_feature_id, modules) = seed(&store).await;
        let key = manager_key(&store).await;

        let mut actors = Vec::new();
        for (module_id, name) in [(&modules[0], "agent.mod.schema"), (&modules[1], "agent.mod.api")] {
            let minted = store
                .mint_worker(&Actor::new("laptop"), Some(&key.info.id), module_id, name, None)
                .await
                .expect("mint");
            actors.push(Actor::delegated(&minted.agent_name, &minted.module_id));
        }

        // Both claims are held at once — the fan-out the whole design is
        // for, and the part that was never broken.
        store.claim_module(&actors[0], &modules[0]).await.expect("claim 0");
        store.claim_module(&actors[1], &modules[1]).await.expect("claim 1");

        // Reaching for a sibling's module is refused on scope, which is
        // the strictly stronger guard: it does not even depend on who
        // holds the claim.
        let err = store
            .complete_module(&actors[0], &modules[1], "not mine", &[])
            .await
            .expect_err("a sibling's module is not completable");
        assert_eq!(err.code, ErrorCode::Forbidden);

        // The claim guard still binds two identities inside one scope:
        // an undelegated actor on the same key cannot take over either.
        let err = store
            .complete_module("laptop", &modules[0], "the machine speaking", &[])
            .await
            .expect_err("the key's own identity does not hold this claim");
        assert_eq!(err.code, ErrorCode::NotClaimedByYou);
    })
    .await;
}

/// Delegation is one level deep. A delegated identity cannot mint
/// another, so the tree stays legible — and this is enforced in the
/// store rather than only by the role gate, because the gate binds only
/// where a role was proven and a local stdio session proves nothing.
#[tokio::test]
async fn a_delegated_identity_cannot_mint() {
    with_scratch("dlgdepth", |store, _url| async move {
        let (_feature_id, modules) = seed(&store).await;
        let key = manager_key(&store).await;
        let minted = store
            .mint_worker(
                &Actor::new("laptop"),
                Some(&key.info.id),
                &modules[0],
                "agent.mod.schema",
                None,
            )
            .await
            .expect("mint");
        let actor = Actor::delegated(&minted.agent_name, &minted.module_id);

        let err = store
            .mint_worker(&actor, Some(&key.info.id), &modules[0], "agent.sub.sub", None)
            .await
            .expect_err("a delegated identity cannot re-mint");
        assert_eq!(err.code, ErrorCode::Forbidden);

        // The role gate says the same thing on the transports that have
        // one, so the two cannot disagree.
        assert!(!KeyRole::Worker.may_call("mint_worker"));
        assert!(KeyRole::Manager.may_call("mint_worker"));
    })
    .await;
}

/// Nothing outlives the work it was minted for. Both exits retire the
/// token, in the same transaction that ends the module.
#[tokio::test]
async fn a_token_dies_with_its_module() {
    with_scratch("dlgdeath", |store, _url| async move {
        let (_feature_id, modules) = seed(&store).await;
        let key = manager_key(&store).await;

        // Exit one: completion.
        let done = store
            .mint_worker(
                &Actor::new("laptop"),
                Some(&key.info.id),
                &modules[0],
                "agent.mod.schema",
                None,
            )
            .await
            .expect("mint");
        let actor = Actor::delegated(&done.agent_name, &done.module_id);
        let briefing = store.claim_module(&actor, &modules[0]).await.expect("claim");
        store
            .complete_task(&actor, &briefing.module.tasks[0].id, TaskOutcome::Done, None)
            .await
            .expect("task");
        store
            .complete_module(&actor, &modules[0], "Tables landed.", &[])
            .await
            .expect("complete");
        let err = store
            .resolve_delegation(Some(&key.info.id), &done.delegation_token)
            .await
            .expect_err("completing the module retires its token");
        assert_eq!(err.code, ErrorCode::Unauthorized);

        // Exit two: release.
        let released = store
            .mint_worker(
                &Actor::new("laptop"),
                Some(&key.info.id),
                &modules[1],
                "agent.mod.api",
                None,
            )
            .await
            .expect("mint");
        let actor = Actor::delegated(&released.agent_name, &released.module_id);
        store.claim_module(&actor, &modules[1]).await.expect("claim");
        store
            .release_module(&actor, &modules[1], "out of depth")
            .await
            .expect("release");
        let err = store
            .resolve_delegation(Some(&key.info.id), &released.delegation_token)
            .await
            .expect_err("releasing the module retires its token too");
        assert_eq!(err.code, ErrorCode::Unauthorized);
    })
    .await;
}

/// One live token per module. A manager re-dispatching a module must
/// not leave a second identity able to write to it — the replaced
/// subagent may still be running.
#[tokio::test]
async fn minting_again_retires_the_previous_token() {
    with_scratch("dlgremint", |store, _url| async move {
        let (_feature_id, modules) = seed(&store).await;
        let key = manager_key(&store).await;

        let first = store
            .mint_worker(
                &Actor::new("laptop"),
                Some(&key.info.id),
                &modules[0],
                "agent.mod.schema",
                None,
            )
            .await
            .expect("mint");
        let second = store
            .mint_worker(
                &Actor::new("laptop"),
                Some(&key.info.id),
                &modules[0],
                "agent.mod.schema.retry",
                None,
            )
            .await
            .expect("re-mint");

        let err = store
            .resolve_delegation(Some(&key.info.id), &first.delegation_token)
            .await
            .expect_err("the superseded token stops resolving");
        assert_eq!(err.code, ErrorCode::Unauthorized);
        store
            .resolve_delegation(Some(&key.info.id), &second.delegation_token)
            .await
            .expect("the fresh one resolves");
    })
    .await;
}

/// A forgotten token is not a standing grant. The TTL is enforced in
/// the resolving query, so an expired row stops resolving without any
/// sweep having to run.
#[tokio::test]
async fn an_expired_token_stops_resolving() {
    with_scratch("dlgttl", |store, url| async move {
        let (_feature_id, modules) = seed(&store).await;
        let key = manager_key(&store).await;
        let minted = store
            .mint_worker(
                &Actor::new("laptop"),
                Some(&key.info.id),
                &modules[0],
                "agent.mod.schema",
                Some(5),
            )
            .await
            .expect("mint");
        store
            .resolve_delegation(Some(&key.info.id), &minted.delegation_token)
            .await
            .expect("live while inside its TTL");

        // No tool can age a row, and waiting out a TTL in a test is
        // worse than reaching past the store for one UPDATE.
        let mut conn = PgConnection::connect(&url).await.expect("connect");
        conn.execute("UPDATE delegations SET expires_at = now() - interval '1 minute'")
            .await
            .expect("age the token");

        let err = store
            .resolve_delegation(Some(&key.info.id), &minted.delegation_token)
            .await
            .expect_err("an expired token resolves to nothing");
        assert_eq!(err.code, ErrorCode::Unauthorized);
    })
    .await;
}

/// A TTL a manager asks for is clamped rather than refused: a bad
/// number mid-dispatch should not cost the dispatch.
#[tokio::test]
async fn an_absurd_ttl_is_clamped_not_refused() {
    with_scratch("dlgclamp", |store, _url| async move {
        let (_feature_id, modules) = seed(&store).await;
        let key = manager_key(&store).await;
        let week = store
            .mint_worker(
                &Actor::new("laptop"),
                Some(&key.info.id),
                &modules[0],
                "agent.mod.schema",
                Some(60 * 24 * 7),
            )
            .await
            .expect("mint");
        let ceiling = chrono::Utc::now() + chrono::Duration::hours(25);
        assert!(week.expires_at < ceiling, "a week is clamped to a day");

        let instant = store
            .mint_worker(
                &Actor::new("laptop"),
                Some(&key.info.id),
                &modules[1],
                "agent.mod.api",
                Some(0),
            )
            .await
            .expect("mint");
        assert!(
            instant.expires_at > chrono::Utc::now(),
            "and a zero is clamped up to something usable"
        );
    })
    .await;
}
