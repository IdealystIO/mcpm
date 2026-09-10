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

/// Two independent modules — the shape that makes sibling subagents
/// possible in the first place.
fn plan() -> PlanFeature {
    PlanFeature {
        name: "Delegation".into(),
        description: "Two concurrent modules with no edge between them.".into(),
        modules: vec![
            PlanModule {
                name: "Schema".into(),
                description: "Tables".into(),
                tasks: vec!["Write migration".into()],
                ..Default::default()
            },
            PlanModule {
                name: "API".into(),
                description: "Endpoints".into(),
                tasks: vec!["Write handler".into()],
                ..Default::default()
            },
        ],
        ..Default::default()
    }
}

/// Plan the feature and return `(feature_id, [module ids])`, in plan
/// order (Schema, API) whatever order the tree lists them in.
async fn seed(store: &Store) -> (String, Vec<String>) {
    let tree = store.plan_feature("laptop", plan()).await.expect("plan");
    let by_name = |name: &str| {
        tree.modules
            .iter()
            .find(|m| m.name == name)
            .map(|m| m.id.clone())
            .expect("planned module")
    };
    let modules = vec![by_name("Schema"), by_name("API")];
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
                MintRequest {
                    module_id: &modules[0],
                    agent_name: "agent.mod.schema",
                    ..Default::default()
                },
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
            .complete_module(&actor, &modules[0], "Tables landed.", &[], None)
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
                MintRequest {
                    module_id: &modules[0],
                    agent_name: "agent.mod.schema",
                    ..Default::default()
                },
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
                MintRequest {
                    module_id: &modules[0],
                    agent_name: "agent.mod.schema",
                    ..Default::default()
                },
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
                .mint_worker(
                    &Actor::new("laptop"),
                    Some(&key.info.id),
                    MintRequest { module_id, agent_name: name, ..Default::default() },
                )
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
            .complete_module(&actors[0], &modules[1], "not mine", &[], None)
            .await
            .expect_err("a sibling's module is not completable");
        assert_eq!(err.code, ErrorCode::Forbidden);

        // The claim guard still binds two identities inside one scope:
        // an undelegated actor on the same key cannot take over either.
        let err = store
            .complete_module("laptop", &modules[0], "the machine speaking", &[], None)
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
                MintRequest {
                    module_id: &modules[0],
                    agent_name: "agent.mod.schema",
                    ..Default::default()
                },
            )
            .await
            .expect("mint");
        let actor = Actor::delegated(&minted.agent_name, &minted.module_id);

        let err = store
            .mint_worker(
                &actor,
                Some(&key.info.id),
                MintRequest {
                    module_id: &modules[0],
                    agent_name: "agent.sub.sub",
                    ..Default::default()
                },
            )
            .await
            .expect_err("a delegated identity cannot re-mint");
        assert_eq!(err.code, ErrorCode::Forbidden);

        // The one-level-deep property is the STORE's, not the role
        // gate's — mint_worker came off MANAGER_ONLY on 2026-09-04 so a
        // box could fan its stage out, and this test is what proves
        // that did not buy depth along with it.
        assert!(KeyRole::Worker.may_call("mint_worker"));
        assert!(KeyRole::Manager.may_call("mint_worker"));
    })
    .await;
}

/// A worker key may mint, but only inside a feature it already holds a
/// claim in. That is the whole of what replaced mint_worker's place on
/// MANAGER_ONLY: a box fans its own stage out, and still cannot reach
/// work it was never dispatched to.
#[tokio::test]
async fn a_worker_mints_only_inside_a_feature_it_holds() {
    with_scratch("wkrmint", |store, _url| async move {
        let (_feature_id, modules) = seed(&store).await;
        let key = store
            .issue_key("operator", "Branch box", "boxy", KeyRole::Worker)
            .await
            .expect("issue worker key");

        // Holding nothing, it may not mint — not even for a real module.
        let err = store
            .mint_worker(
                &Actor::new("boxy"),
                Some(&key.info.id),
                MintRequest {
                    module_id: &modules[0],
                    agent_name: "agent.mod.one",
                    ..Default::default()
                },
            )
            .await
            .expect_err("a claimless worker cannot mint");
        assert_eq!(err.code, ErrorCode::Forbidden);

        // With a claim in the feature it may mint for a SIBLING module
        // it does not itself hold — which is the point of the change.
        store.claim_module("boxy", &modules[0]).await.expect("claim");
        let minted = store
            .mint_worker(
                &Actor::new("boxy"),
                Some(&key.info.id),
                MintRequest {
                    module_id: &modules[1],
                    agent_name: "agent.mod.two",
                    ..Default::default()
                },
            )
            .await
            .expect("a worker holding a claim mints for its sibling");
        assert_eq!(minted.module_id, modules[1]);

        // And the token it minted is still a WORKER confined to one
        // module: the authority it passes on is no wider than its own.
        let actor = Actor::delegated(&minted.agent_name, &minted.module_id);
        let err = store
            .mint_worker(
                &actor,
                Some(&key.info.id),
                MintRequest {
                    module_id: &modules[0],
                    agent_name: "agent.sub.sub",
                    ..Default::default()
                },
            )
            .await
            .expect_err("still one level deep");
        assert_eq!(err.code, ErrorCode::Forbidden);
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
                MintRequest {
                    module_id: &modules[0],
                    agent_name: "agent.mod.schema",
                    ..Default::default()
                },
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
            .complete_module(&actor, &modules[0], "Tables landed.", &[], None)
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
                MintRequest {
                    module_id: &modules[1],
                    agent_name: "agent.mod.api",
                    ..Default::default()
                },
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
                MintRequest {
                    module_id: &modules[0],
                    agent_name: "agent.mod.schema",
                    ..Default::default()
                },
            )
            .await
            .expect("mint");
        let second = store
            .mint_worker(
                &Actor::new("laptop"),
                Some(&key.info.id),
                MintRequest {
                    module_id: &modules[0],
                    agent_name: "agent.mod.schema.retry",
                    ..Default::default()
                },
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
                MintRequest {
                    module_id: &modules[0],
                    agent_name: "agent.mod.schema",
                    ttl_minutes: Some(5),
                    ..Default::default()
                },
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
                MintRequest {
                    module_id: &modules[0],
                    agent_name: "agent.mod.schema",
                    ttl_minutes: Some(60 * 24 * 7),
                    ..Default::default()
                },
            )
            .await
            .expect("mint");
        let ceiling = chrono::Utc::now() + chrono::Duration::hours(25);
        assert!(week.expires_at < ceiling, "a week is clamped to a day");

        let instant = store
            .mint_worker(
                &Actor::new("laptop"),
                Some(&key.info.id),
                MintRequest {
                    module_id: &modules[1],
                    agent_name: "agent.mod.api",
                    ttl_minutes: Some(0),
                    ..Default::default()
                },
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

/// The four ways a token stops resolving must read as four different
/// things, because they want four different reactions and a subagent
/// that cannot tell them apart correctly refuses to guess. Collapsing
/// them into one sentence cost a working box twenty minutes and a
/// round trip to a human.
#[tokio::test]
async fn the_four_ways_a_token_dies_are_told_apart() {
    with_scratch("dlgwhy", |store, url| async move {
        let (_feature_id, modules) = seed(&store).await;
        let laptop = manager_key(&store).await;
        let box_key = store
            .issue_key("operator", "Branch box", "box-01", KeyRole::Worker)
            .await
            .expect("issue");

        let mint = |module: String, name: &'static str| {
            let store = &store;
            let key_id = laptop.info.id.clone();
            async move {
                store
                    .mint_worker(
                        &Actor::new("laptop"),
                        Some(&key_id),
                        MintRequest { module_id: &module, agent_name: name, ..Default::default() },
                    )
                    .await
                    .expect("mint")
            }
        };

        // Unknown: a well-formed token this server never minted. The
        // secret half decides this, so a guesser holding only an id
        // lands here too and learns nothing about the row behind it.
        let live = mint(modules[0].clone(), "agent.mod.schema").await;
        let (head, _) = live.delegation_token.rsplit_once('_').expect("token has a secret half");
        let forged = format!("{head}_{}", "0".repeat(64));
        let unknown = store
            .resolve_delegation(Some(&laptop.info.id), &forged)
            .await
            .expect_err("a forged secret resolves to nothing");
        assert!(
            unknown.message.contains("does not resolve to any identity"),
            "unknown reads as unknown: {}", unknown.message
        );

        // Wrong key: structurally fine, presented from the wrong
        // machine. This is an OPERATOR error with a specific remedy,
        // and it must not read as a dead credential.
        let wrong = store
            .resolve_delegation(Some(&box_key.info.id), &live.delegation_token)
            .await
            .expect_err("the box's key does not carry the laptop's token");
        assert!(
            wrong.message.contains("bound to a different"),
            "the cross-key case is named: {}", wrong.message
        );
        assert_eq!(wrong.data["bound_key_id"], serde_json::json!(laptop.info.id));
        assert_eq!(wrong.data["presented_key_id"], serde_json::json!(box_key.info.id));
        assert!(
            wrong.hint.contains("for_key_id"),
            "and it names the remedy: {}", wrong.hint
        );

        // Retired: the module ended, or a replacement was minted.
        let superseded = live;
        let _replacement = mint(modules[0].clone(), "agent.mod.schema.retry").await;
        let retired = store
            .resolve_delegation(Some(&laptop.info.id), &superseded.delegation_token)
            .await
            .expect_err("a superseded token resolves to nothing");
        assert!(
            retired.message.contains("retired"),
            "retired reads as retired: {}", retired.message
        );

        // Expired: past its TTL, which no tool can bring about in a test.
        let aging = mint(modules[1].clone(), "agent.mod.api").await;
        let mut conn = PgConnection::connect(&url).await.expect("connect");
        conn.execute(
            format!(
                "UPDATE delegations SET expires_at = now() - interval '1 minute'
                 WHERE module_id = '{}'",
                modules[1]
            )
            .as_str(),
        )
        .await
        .expect("age the token");
        let expired = store
            .resolve_delegation(Some(&laptop.info.id), &aging.delegation_token)
            .await
            .expect_err("an expired token resolves to nothing");
        assert!(
            expired.message.contains("expired"),
            "expired reads as expired: {}", expired.message
        );

        // All four are still refusals, and all four still say the one
        // thing they share: do not fall back to the machine's identity.
        for err in [&unknown, &wrong, &retired, &expired] {
            assert_eq!(err.code, ErrorCode::Unauthorized);
            assert!(
                err.hint.contains("Do not fall back"),
                "every cause forbids the silent fall-back: {}", err.hint
            );
        }
    })
    .await;
}

/// `for_key_id` moves where the boundary sits without changing its
/// shape: still exactly ONE key honours the token, but it can be a key
/// that lives on another box. This is the fleet case — a manager
/// dispatching work to a machine that holds a credential of its own.
#[tokio::test]
async fn a_token_can_be_bound_to_the_key_of_the_box_that_will_use_it() {
    with_scratch("dlgforkey", |store, _url| async move {
        let (feature_id, modules) = seed(&store).await;
        let laptop = manager_key(&store).await;
        let remote = store
            .issue_worker_key(&Actor::new("laptop"), "box.branch-a", None)
            .await
            .expect("issue the box a key");

        let minted = store
            .mint_worker(
                &Actor::new("laptop"),
                Some(&laptop.info.id),
                MintRequest {
                    module_id: &modules[0],
                    agent_name: "agent.mod.schema",
                    for_key_id: Some(&remote.info.id),
                    ..Default::default()
                },
            )
            .await
            .expect("mint");
        assert_eq!(minted.bound_key_id.as_deref(), Some(remote.info.id.as_str()));
        assert!(
            minted.instructions.contains("NOT to this machine"),
            "the manager is told where this one works: {}", minted.instructions
        );

        // It works there, and only there — including not here, on the
        // key that minted it.
        store
            .resolve_delegation(Some(&remote.info.id), &minted.delegation_token)
            .await
            .expect("the box it was minted for resolves it");
        let err = store
            .resolve_delegation(Some(&laptop.info.id), &minted.delegation_token)
            .await
            .expect_err("not even the minting key resolves a token bound elsewhere");
        assert_eq!(err.code, ErrorCode::Unauthorized);

        // The ledger records which credential was allowed to present
        // it, because that is the question an audit asks and it cannot
        // be reconstructed from the minter.
        let events = store.get_events(Some(&feature_id), 0, 200).await.expect("events");
        let mint_event = events
            .iter()
            .find(|e| e.kind == "worker_minted")
            .expect("worker_minted recorded");
        assert_eq!(mint_event.payload["bound_key_id"], serde_json::json!(remote.info.id));
        assert_eq!(mint_event.payload["delegated_off_machine"], serde_json::json!(true));
    })
    .await;
}

/// A token bound to a credential that could never present it is born
/// dead, and the box holding it would read that as an error of its own.
/// So the target is checked at mint time, where the manager that made
/// the mistake is still the one listening.
#[tokio::test]
async fn a_token_cannot_be_bound_to_a_key_that_could_not_present_it() {
    with_scratch("dlgbadtarget", |store, _url| async move {
        let (_feature_id, modules) = seed(&store).await;
        let laptop = manager_key(&store).await;
        let console = store
            .issue_key("operator", "Console", "console", KeyRole::Console)
            .await
            .expect("issue");
        let revoked = store
            .issue_key("operator", "Old box", "box-old", KeyRole::Worker)
            .await
            .expect("issue");
        store.revoke_key("operator", &revoked.info.id).await.expect("revoke");

        for (target, code) in [
            ("mcpm_deadbeefdead_x", ErrorCode::NotFound),
            (console.info.id.as_str(), ErrorCode::Forbidden),
            (revoked.info.id.as_str(), ErrorCode::Forbidden),
        ] {
            let err = store
                .mint_worker(
                    &Actor::new("laptop"),
                    Some(&laptop.info.id),
                    MintRequest {
                        module_id: &modules[0],
                        agent_name: "agent.mod.schema",
                        for_key_id: Some(target),
                        ..Default::default()
                    },
                )
                .await
                .expect_err("a token bound to this would never work");
            assert_eq!(err.code, code, "refusing {target}");
        }

        // And nothing was written on the way to those refusals.
        assert!(
            store
                .get_events(None, 0, 200)
                .await
                .expect("events")
                .iter()
                .all(|e| e.kind != "worker_minted"),
            "a refused mint mints nothing"
        );
    })
    .await;
}

/// The fleet case proper: boxes that are not this machine get keys of
/// their own, so they are distinct identities in the ledger and cannot
/// complete each other's modules. Delegation is not stretched to cover
/// this — it stays the same-machine mechanism it was built as.
#[tokio::test]
async fn a_manager_can_give_each_box_an_identity_of_its_own() {
    with_scratch("dlgboxkeys", |store, _url| async move {
        let (_feature_id, modules) = seed(&store).await;
        // The operator's own key, so the ledger below has one of each
        // door to tell apart.
        let _operator_issued = manager_key(&store).await;
        let manager = Actor::new("laptop");

        let a = store
            .issue_worker_key(&manager, "box.branch-a", Some("Branch A"))
            .await
            .expect("issue A");
        let b = store
            .issue_worker_key(&manager, "box.branch-b", None)
            .await
            .expect("issue B");

        // The role is not the manager's to choose. A manager that could
        // issue manager keys could hand a subagent the planning surface.
        assert_eq!(a.info.role, KeyRole::Worker);
        assert_eq!(b.info.role, KeyRole::Worker);
        assert_eq!(a.info.created_by, "laptop");
        assert_eq!(b.info.label, "box.branch-b", "the label defaults to the name");

        // Each key verifies back to its OWN identity — the thing a
        // shared fleet key cannot do.
        assert_eq!(store.verify_key(&a.token).await.expect("verify").agent_name, "box.branch-a");
        assert_eq!(store.verify_key(&b.token).await.expect("verify").agent_name, "box.branch-b");

        // Which is what makes mutual exclusion between boxes work.
        store.claim_module(&Actor::new("box.branch-a"), &modules[0]).await.expect("A claims");
        let err = store
            .complete_module(&Actor::new("box.branch-b"), &modules[0], "not mine", &[], None)
            .await
            .expect_err("one box cannot finish another's module");
        assert_eq!(err.code, ErrorCode::NotClaimedByYou);

        // Issuance is on the record, and says which door it came
        // through — a key an agent minted is not a key the operator did.
        let events = store.get_events(None, 0, 200).await.expect("events");
        let issued: Vec<&str> = events
            .iter()
            .filter(|e| e.kind == "key_issued")
            .filter_map(|e| e.payload["via"].as_str())
            .collect();
        assert_eq!(issued, ["cli", "issue_worker_key", "issue_worker_key"]);
    })
    .await;
}

/// Two live keys under one agent name are two boxes the ledger cannot
/// tell apart and that can complete each other's modules — the exact
/// failure a key per box exists to fix, arrived at from the other side.
/// Rotation is revoke-then-issue, which leaves the withdrawal on the
/// record.
#[tokio::test]
async fn one_live_worker_key_per_agent_name() {
    with_scratch("dlgdupname", |store, _url| async move {
        let manager = Actor::new("laptop");
        let first = store
            .issue_worker_key(&manager, "box.branch-a", None)
            .await
            .expect("issue");

        let err = store
            .issue_worker_key(&manager, "box.branch-a", None)
            .await
            .expect_err("a second live key under one name is refused");
        assert_eq!(err.code, ErrorCode::PlanConflict);
        assert_eq!(err.data["key_id"], serde_json::json!(first.info.id));

        // Whitespace is not a second box either.
        assert!(store.issue_worker_key(&manager, "  box.branch-a  ", None).await.is_err());
        assert!(store.issue_worker_key(&manager, "   ", None).await.is_err());

        // Rotation works, once the old key is actually withdrawn.
        store.revoke_key("operator", &first.info.id).await.expect("revoke");
        store
            .issue_worker_key(&manager, "box.branch-a", None)
            .await
            .expect("re-issue after revocation");
    })
    .await;
}

/// Issuing a credential is the heaviest thing on the manager surface,
/// so it is bounded the same way minting is: not by a delegated
/// identity, and not by a worker key. The store repeats what the gate
/// says because the gate binds only where a role was proven, and a
/// local stdio session proves nothing.
#[tokio::test]
async fn issuing_a_key_is_the_managers_act_alone() {
    with_scratch("dlgissuegate", |store, _url| async move {
        let (_feature_id, modules) = seed(&store).await;
        let key = manager_key(&store).await;
        let minted = store
            .mint_worker(
                &Actor::new("laptop"),
                Some(&key.info.id),
                MintRequest {
                    module_id: &modules[0],
                    agent_name: "agent.mod.schema",
                    ..Default::default()
                },
            )
            .await
            .expect("mint");
        let delegated = Actor::delegated(&minted.agent_name, &minted.module_id);

        let err = store
            .issue_worker_key(&delegated, "box.sneaky", None)
            .await
            .expect_err("a delegated identity cannot create credentials");
        assert_eq!(err.code, ErrorCode::Forbidden);

        assert!(!KeyRole::Worker.may_call("issue_worker_key"));
        assert!(KeyRole::Manager.may_call("issue_worker_key"));
        assert!(!KeyRole::Console.may_call("issue_worker_key"));
    })
    .await;
}
