//! Control Center project-management core.
//!
//! Domain model + Postgres store for the agent-facing PM system:
//! Feature → Module (one worker each; a module is claimable once every
//! prerequisite it names is done) → Task, plus the want pool that
//! features are composed from, scoped memories, long-form documents
//! (a feature's whitepaper, a module's handoff), and the append-only
//! event ledger. The server is a gatekeeper, not an orchestrator:
//! nothing here spawns agents — [`Store`] enforces the invariants
//! (prerequisites, exclusive claims, checklist-proven completion) so a
//! bad plan fails loudly at the tool boundary.

mod error;
mod ids;
mod keys;
mod redact;
mod store;
mod types;

pub use error::{McpmError, ErrorCode};
pub use ids::{
    id_level, new_document_id, new_id, new_want_id, normalize_tag, Level, PROJECT_SUBJECT,
};
pub use keys::{
    display_key, from_bearer, Actor, ApiKeyInfo, Delegation, IssuedKey, KeyIdentity, KeyRole,
    MintRequest, MintedWorker, MANAGER_ONLY,
};
pub use redact::redact_url;
pub use store::Store;
pub use types::*;
