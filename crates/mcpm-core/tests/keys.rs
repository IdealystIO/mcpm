//! The API key lifecycle against a real database.
//!
//! The unit tests in `keys.rs` cover the token format; these cover what
//! only the store can answer — that a key verifies exactly once it has
//! been issued and not after it has been revoked, that verification
//! returns the identity the key was issued FOR rather than anything a
//! caller supplied, and that a good token for a different deployment's
//! database is refused. Those are the properties the whole remote
//! transport rests on, so they get a test rather than a read-through.

use mcpm_core::*;
use sqlx::{Connection, Executor, PgConnection};

fn base_url() -> String {
    std::env::var("DATABASE_URL")
        .unwrap_or_else(|_| "postgres://app:app@localhost:55432/app".to_string())
}

/// Run `body` against a freshly migrated scratch database, then drop it.
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

#[tokio::test]
async fn a_key_verifies_to_the_identity_it_was_issued_for() {
    with_scratch("keyid", |store| async move {
        let issued = store
            .issue_key("operator", "CI worker", "worker-03", KeyRole::Worker)
            .await
            .expect("issue");

        let identity = store.verify_key(&issued.token).await.expect("verify");
        // The point of the whole design: the caller never gets to say
        // who it is, so these come off the key.
        assert_eq!(identity.agent_name, "worker-03");
        assert_eq!(identity.role, KeyRole::Worker);
        assert_eq!(identity.key_id, issued.info.id);
        assert_eq!(identity.label, "CI worker");
    })
    .await;
}

/// The secret is shown once and stored only as a digest. A dump of
/// `api_keys` must not be replayable.
#[tokio::test]
async fn the_stored_row_does_not_contain_the_token() {
    with_scratch("keyhash", |store| async move {
        let issued = store
            .issue_key("operator", "", "mgr", KeyRole::Manager)
            .await
            .expect("issue");

        let hash: String = sqlx::query_scalar("SELECT hash FROM api_keys WHERE id = $1")
            .bind(&issued.info.id)
            .fetch_one(store.pool())
            .await
            .expect("read the row back");

        assert!(
            !issued.token.contains(&hash),
            "the stored digest must not be a substring of the token"
        );
        assert!(
            !hash.is_empty() && hash != issued.token,
            "the token itself must never be stored"
        );
        // An empty --label falls back to the agent name rather than
        // leaving an unnamed key in the operator's list.
        assert_eq!(issued.info.label, "mgr");
    })
    .await;
}

#[tokio::test]
async fn a_revoked_key_stops_verifying_and_stays_listed() {
    with_scratch("keyrevoke", |store| async move {
        let issued = store
            .issue_key("operator", "temp", "worker-x", KeyRole::Worker)
            .await
            .expect("issue");
        store.verify_key(&issued.token).await.expect("live before revoke");

        let info = store
            .revoke_key("operator", &issued.info.id)
            .await
            .expect("revoke");
        assert!(info.revoked_at.is_some());
        assert!(!info.is_live());

        let err = store
            .verify_key(&issued.token)
            .await
            .expect_err("a revoked key must not verify");
        assert_eq!(err.code, ErrorCode::Unauthorized);

        // Revoking twice is a no-op that still succeeds: the caller's
        // intent is satisfied either way.
        store
            .revoke_key("operator", &issued.info.id)
            .await
            .expect("revoking twice is idempotent");

        // Withdrawn, not erased — "what did we revoke, and when" is
        // what the operator's list gets asked.
        let listed = store.list_keys().await.expect("list");
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].id, issued.info.id);
        assert!(!listed[0].is_live());
        assert_eq!(store.live_key_count().await.expect("count"), 0);
    })
    .await;
}

/// A token that is perfectly well-formed — minted by another
/// deployment, or guessed with the right shape — must be refused, and
/// refused the same way a malformed one is.
#[tokio::test]
async fn a_well_formed_token_this_database_never_issued_is_refused() {
    with_scratch("keyforeign", |store| async move {
        store
            .issue_key("operator", "real", "mgr", KeyRole::Manager)
            .await
            .expect("issue");

        for candidate in [
            // Right shape, never issued here.
            "mcpm_aabbccddeeff_\
             0000000000000000000000000000000000000000000000000000000000000000",
            "not-a-token",
            "",
        ] {
            let err = store
                .verify_key(candidate)
                .await
                .expect_err("must not verify");
            assert_eq!(
                err.code,
                ErrorCode::Unauthorized,
                "every failure reads the same, so a probe learns nothing"
            );
        }
    })
    .await;
}

/// Issuing and revoking are operator actions on who may act — they
/// belong in the audit trail, without the credential itself.
#[tokio::test]
async fn the_ledger_records_the_key_lifecycle_but_never_the_secret() {
    with_scratch("keyevents", |store| async move {
        let issued = store
            .issue_key("operator", "auditable", "worker-9", KeyRole::Worker)
            .await
            .expect("issue");
        store
            .revoke_key("operator", &issued.info.id)
            .await
            .expect("revoke");

        let events = store.get_events(None, 0, 100).await.expect("events");
        let kinds: Vec<&str> = events.iter().map(|e| e.kind.as_str()).collect();
        assert!(kinds.contains(&"key_issued"), "got {kinds:?}");
        assert!(kinds.contains(&"key_revoked"), "got {kinds:?}");

        let dump = serde_json::to_string(&events.iter().map(|e| &e.payload).collect::<Vec<_>>())
            .expect("serialize payloads");
        let secret = issued.token.rsplit('_').next().expect("token has a secret half");
        assert!(
            !dump.contains(secret),
            "the ledger must never carry the token — it is read by everything with console access"
        );
    })
    .await;
}
