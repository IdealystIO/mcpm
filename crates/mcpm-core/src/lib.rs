//! Control Center project-management core.
//!
//! Domain model + Postgres store for the agent-facing PM system:
//! Feature → Module (one worker each; a module is claimable once every
//! prerequisite it names is done) → Task, plus the want pool that
//! features are composed from, scoped memories, long-form documents
//! (a feature's whitepaper, a module's handoff), files attached to a
//! feature or a want with a description written for the agent that
//! reads them, and the append-only event ledger. The server is a gatekeeper, not an orchestrator:
//! nothing here spawns agents — [`Store`] enforces the invariants
//! (prerequisites, exclusive claims, checklist-proven completion) so a
//! bad plan fails loudly at the tool boundary.

mod error;
mod files;
mod health;
mod ids;
mod keys;
mod redact;
mod store;
mod types;

pub use error::{McpmError, ErrorCode};
pub use files::{FileProvider, MemoryFiles, S3Files, DEFAULT_BUCKET, S3_ENV};
pub use health::{
    probe, probe_client, run_prober, sweep, HealthPolicy, HealthState, HealthVerdict,
    DEFAULT_INTERVAL_SECS, HEALTH_HOSTS_ENV, HEALTH_INTERVAL_ENV, PROBE_TIMEOUT,
};
pub use ids::{
    id_level, new_attachment_id, new_comment_id, new_document_id, new_id, new_want_id, normalize_tag, Level,
    PROJECT_SUBJECT,
};
pub use keys::{
    display_key, from_bearer, Actor, ApiKeyInfo, Delegation, IssuedKey, KeyIdentity, KeyRole,
    MintRequest, MintedWorker, MANAGER_ONLY,
};
pub use redact::redact_url;
pub use store::{Store, ATTACHMENT_LINK_TTL_SECS, MAX_ATTACHMENT_BYTES};
pub use types::*;
