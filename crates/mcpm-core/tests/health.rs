//! Agent health against a real database: what a registration may
//! name, who may register one, and how a probe's verdict becomes the
//! roster's state and the ledger's event.
//!
//! The pure half — the allowlist and the status-line mapping — is unit
//! tested in `health.rs`. What only the store can answer is here: that
//! a change of state is one event and a steady state is none, that a
//! re-registered URL forgets the old verdict, that a subagent cannot
//! rewrite its machine's URL, and that a key issued with a URL puts
//! the box on the roster before it has spoken.
//!
//! Runs against a real Postgres: set DATABASE_URL (defaults to the
//! devcontainer database published on host port 55432). Each test
//! creates and drops its own scratch database.

use std::sync::Arc;

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
        body(store).await;
    }
    admin
        .execute(format!("DROP DATABASE {db_name} WITH (FORCE)").as_str())
        .await
        .expect("drop scratch database");
}

fn policy() -> Arc<HealthPolicy> {
    Arc::new(HealthPolicy::new([".dev.example.com"]).expect("policy"))
}

fn verdict(state: HealthState, detail: &str) -> HealthVerdict {
    HealthVerdict { state, detail: detail.to_string() }
}

async fn health_of(store: &Store, name: &str) -> Option<AgentHealth> {
    store
        .agents_overview()
        .await
        .expect("overview")
        .into_iter()
        .find(|a| a.name == name)
        .and_then(|a| a.health)
}

async fn health_events(store: &Store) -> Vec<Event> {
    store
        .get_events(None, 0, 500)
        .await
        .expect("events")
        .into_iter()
        .filter(|e| e.kind == "agent_health")
        .collect()
}

#[tokio::test]
async fn with_no_policy_a_registration_is_refused_by_name() {
    with_scratch("hoff", |store| async move {
        store.get_context("box.a", "worker").await.expect("register");
        let err = store
            .set_health_url(&Actor::new("box.a"), Some("https://a.dev.example.com/"))
            .await
            .unwrap_err();
        assert!(err.message.contains("not configured"), "{}", err.message);
        assert!(err.hint.contains(HEALTH_HOSTS_ENV), "the hint names the switch: {}", err.hint);
        assert!(health_of(&store, "box.a").await.is_none(), "nothing was written");
    })
    .await;
}

#[tokio::test]
async fn a_registration_lands_unprobed_and_a_bad_one_changes_nothing() {
    with_scratch("hreg", |store| async move {
        let store = store.with_health(policy());
        store.get_context("box.a", "worker").await.expect("register");

        store
            .set_health_url(&Actor::new("box.a"), Some("https://A.dev.example.com"))
            .await
            .expect("accepted");
        let h = health_of(&store, "box.a").await.expect("registered");
        assert_eq!(h.url, "https://a.dev.example.com/", "normalised");
        assert_eq!(h.state, None, "not probed yet");
        assert!(h.since.is_none() && h.checked_at.is_none());

        // Outside the allowlist: refused, and the good one stays.
        let err = store
            .set_health_url(&Actor::new("box.a"), Some("https://169.254.169.254/"))
            .await
            .unwrap_err();
        assert!(err.message.contains("refused"), "{}", err.message);
        assert_eq!(health_of(&store, "box.a").await.unwrap().url, "https://a.dev.example.com/");

        // get_context echoes what is registered, so a restarted box
        // can see it without asking.
        let ctx = store.get_context("box.a", "worker").await.expect("re-orient");
        assert_eq!(ctx.you.health_url.as_deref(), Some("https://a.dev.example.com/"));

        // Clearing.
        store.set_health_url(&Actor::new("box.a"), None).await.expect("clear");
        assert!(health_of(&store, "box.a").await.is_none());
        assert!(store.health_targets().await.unwrap().is_empty());
    })
    .await;
}

#[tokio::test]
async fn a_subagent_cannot_register_its_machines_url() {
    with_scratch("hdel", |store| async move {
        let store = store.with_health(policy());
        store.get_context("box.a", "worker").await.expect("register");
        let err = store
            .set_health_url(
                &Actor::delegated("box.a.sub", "mod_whatever"),
                Some("https://a.dev.example.com/"),
            )
            .await
            .unwrap_err();
        assert_eq!(err.code, ErrorCode::Forbidden);
        assert!(health_of(&store, "box.a").await.is_none());
    })
    .await;
}

#[tokio::test]
async fn a_change_of_state_is_one_event_and_a_steady_state_is_none() {
    with_scratch("hrec", |store| async move {
        let store = store.with_health(policy());
        store.get_context("box.a", "worker").await.expect("register");
        store
            .set_health_url(&Actor::new("box.a"), Some("https://a.dev.example.com/"))
            .await
            .expect("accepted");

        // First verdict: a change from "unknown".
        assert!(store.record_health("box.a", &verdict(HealthState::Up, "HTTP 200")).await.unwrap());
        let h = health_of(&store, "box.a").await.unwrap();
        assert_eq!(h.state, Some(HealthState::Up));
        assert_eq!(h.detail, "HTTP 200");
        let since = h.since.expect("since set on first verdict");
        let checked = h.checked_at.expect("checked");

        // Same state again: checked_at moves, since and the ledger do not.
        assert!(!store.record_health("box.a", &verdict(HealthState::Up, "HTTP 200")).await.unwrap());
        let h = health_of(&store, "box.a").await.unwrap();
        assert_eq!(h.since, Some(since));
        assert!(h.checked_at.unwrap() >= checked);
        assert_eq!(health_events(&store).await.len(), 1);

        // Down: a second event, from → to on the record, since moves.
        assert!(store.record_health("box.a", &verdict(HealthState::Down, "HTTP 503")).await.unwrap());
        let h = health_of(&store, "box.a").await.unwrap();
        assert_eq!(h.state, Some(HealthState::Down));
        assert!(h.since.unwrap() >= since);
        let events = health_events(&store).await;
        assert_eq!(events.len(), 2);
        let last = events.iter().max_by_key(|e| e.seq).unwrap();
        assert_eq!(last.agent.as_deref(), Some("box.a"));
        assert_eq!(last.payload["from"], "up");
        assert_eq!(last.payload["to"], "down");
        assert_eq!(last.payload["detail"], "HTTP 503");
        assert!(last.feature_id.is_none(), "a box is not a feature");

        // A new URL forgets the old verdict.
        store
            .set_health_url(&Actor::new("box.a"), Some("https://b.dev.example.com/"))
            .await
            .expect("re-register");
        let h = health_of(&store, "box.a").await.unwrap();
        assert_eq!(h.state, None);
        assert_eq!(h.detail, "");
        assert!(h.since.is_none());

        // A verdict for an agent with no URL (cleared between the
        // sweep's read and its write) is dropped, not recorded.
        store.set_health_url(&Actor::new("box.a"), None).await.expect("clear");
        assert!(!store.record_health("box.a", &verdict(HealthState::Up, "HTTP 200")).await.unwrap());
        assert!(health_of(&store, "box.a").await.is_none());
        assert_eq!(health_events(&store).await.len(), 2);
    })
    .await;
}

#[tokio::test]
async fn a_key_issued_with_a_url_puts_the_box_on_the_roster_before_it_speaks() {
    with_scratch("hkey", |store| async move {
        let store = store.with_health(policy());
        let manager = Actor::new("laptop");

        // Refused URL, no key: the cap and the one-per-name rule are
        // not spent on a registration that failed.
        let err = store
            .issue_worker_key(&manager, "box.a", None, Some("https://box-a.elsewhere.example/"))
            .await
            .unwrap_err();
        assert!(err.message.contains("refused"), "{}", err.message);
        let keys = store.list_keys().await.unwrap();
        assert!(keys.iter().all(|k| k.agent_name != "box.a"), "no key was issued");

        store
            .issue_worker_key(&manager, "box.a", None, Some("https://box-a.dev.example.com/"))
            .await
            .expect("issued");
        let h = health_of(&store, "box.a").await.expect("on the roster already");
        assert_eq!(h.url, "https://box-a.dev.example.com/");
        assert_eq!(h.state, None);
        assert_eq!(
            store.health_targets().await.unwrap().iter().map(|t| t.name.as_str()).collect::<Vec<_>>(),
            vec!["box.a"]
        );

        // The box's own first get_context keeps the registration and
        // takes the role the key says.
        let ctx = store.get_context("box.a", "worker").await.expect("first contact");
        assert_eq!(ctx.you.health_url.as_deref(), Some("https://box-a.dev.example.com/"));
    })
    .await;
}
