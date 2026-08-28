//! Control Center project-management core.
//!
//! Domain model + Postgres store for the agent-facing PM system:
//! Feature → Stage (sequential, gate-enforced) → Module (one worker,
//! concurrent within a stage) → Task, plus the want pool that features
//! are composed from, scoped memories, and the append-only event
//! ledger. The server is a gatekeeper, not an
//! orchestrator: nothing here spawns agents — [`Store`] enforces the
//! invariants (stage order, exclusive claims, checklist-proven
//! completion) so a bad plan fails loudly at the tool boundary.

mod error;
mod ids;
mod store;
mod types;

pub use error::{McpmError, ErrorCode};
pub use ids::{id_level, new_id, new_want_id, normalize_tag, Level};
pub use store::Store;
pub use types::*;
