//! The liveness signal: a committed event must reach `watch_events`.
//!
//! This is the load-bearing half of the console's WebSocket feed — if
//! the trigger or the LISTEN breaks, the console silently degrades to
//! its 30s fallback poll with nothing in any log to say so. Hence a
//! test rather than a manual check.

use mcpm_core::*;
use sqlx::{Connection, Executor, PgConnection};

fn base_url() -> String {
    std::env::var("DATABASE_URL")
        .unwrap_or_else(|_| "postgres://app:app@localhost:55432/app".to_string())
}

#[tokio::test]
async fn a_committed_event_reaches_the_watcher() {
    use futures_util::StreamExt as _;

    let base = base_url();
    let db_name = format!("mcpm_notify_{}", &uuid::Uuid::new_v4().simple().to_string()[..12]);
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
        let stream = store.watch_events().await.expect("open the listener");
        let mut stream = std::pin::pin!(stream);

        // Any tool call that commits an event will do; planning is the
        // cheapest one that goes through the real store.
        let plan = PlanFeature {
            name: "Watcher".into(),
            modules: vec![PlanModule {
                name: "M".into(),
                description: "d".into(),
                ..Default::default()
            }],
            ..Default::default()
        };
        store.get_context("agent.test", "manager").await.expect("register");
        store.plan_feature("agent.test", plan).await.expect("plan");

        let seq = tokio::time::timeout(std::time::Duration::from_secs(10), stream.next())
            .await
            .expect("a notification within 10s — is the events_notify trigger installed?")
            .expect("stream yielded a seq");
        assert!(seq > 0, "seq should be the events.seq of the committed row");
    }

    admin
        .execute(format!("DROP DATABASE {db_name} WITH (FORCE)").as_str())
        .await
        .expect("drop scratch database");
}

/// Many consoles, one Postgres connection.
///
/// The first cut gave every subscriber its own `PgListener` taken from
/// the query pool, which capped concurrent consoles at the pool size
/// and then starved ordinary reads. Subscribers must fan out from a
/// single listener instead.
#[tokio::test]
async fn many_subscribers_share_one_listener() {
    use futures_util::StreamExt as _;

    let base = base_url();
    let db_name = format!("mcpm_fanout_{}", &uuid::Uuid::new_v4().simple().to_string()[..12]);
    let mut admin = PgConnection::connect(&base).await.expect("connect");
    admin
        .execute(format!("CREATE DATABASE {db_name}").as_str())
        .await
        .expect("create scratch database");
    let (head, _) = base.rsplit_once('/').expect("url has a database segment");
    let scratch_url = format!("{head}/{db_name}");

    {
        let store = Store::connect(&scratch_url).await.expect("connect + migrate");

        // More subscribers than the pool has connections (4).
        let mut streams = Vec::new();
        for _ in 0..10 {
            streams.push(Box::pin(store.watch_events().await.expect("subscribe")));
        }

        // An ordinary query must still get a connection.
        store.get_context("agent.test", "manager").await.expect("register");
        let plan = PlanFeature {
            name: "Fanout".into(),
            modules: vec![PlanModule {
                name: "M".into(),
                description: "d".into(),
                ..Default::default()
            }],
            ..Default::default()
        };
        store.plan_feature("agent.test", plan).await.expect("plan");

        for (i, stream) in streams.iter_mut().enumerate() {
            let seq = tokio::time::timeout(std::time::Duration::from_secs(10), stream.next())
                .await
                .unwrap_or_else(|_| panic!("subscriber {i} never got the event"))
                .expect("stream yielded a seq");
            assert!(seq > 0);
        }
    }

    admin
        .execute(format!("DROP DATABASE {db_name} WITH (FORCE)").as_str())
        .await
        .expect("drop scratch database");
}
