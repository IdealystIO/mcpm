//! The error envelope (architecture doc §7): a stable machine code,
//! a human sentence, structured facts, and a `hint` written as an
//! instruction to the calling agent — because the caller is a model
//! and the error text is its prompt.

use serde::{Deserialize, Serialize};

/// Stable, machine-readable rejection codes.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ErrorCode {
    StageLocked,
    AlreadyClaimed,
    AlreadyDone,
    NotClaimedByYou,
    TasksOpen,
    /// A state change that loses information needs its reason on the
    /// record: skipping a task, declining a want.
    SkipNeedsReason,
    StagesIncomplete,
    PlanConflict,
    PlanInvalid,
    /// A feature already absorbed this want; it cannot be declined or
    /// re-linked to the same feature.
    WantPromoted,
    AlreadyLinked,
    NotFound,
    NotRegistered,
    Internal,
}

/// One rejection, serialized verbatim into the MCP tool error result.
#[derive(Debug, Serialize, Deserialize, thiserror::Error)]
#[error("{code:?}: {message}")]
pub struct McpmError {
    pub code: ErrorCode,
    pub message: String,
    /// Structured facts an orchestrator could branch on (blocking
    /// stages, open tasks, the claim holder, …).
    #[serde(default, skip_serializing_if = "serde_json::Value::is_null")]
    pub data: serde_json::Value,
    /// What the calling agent should do next.
    pub hint: String,
}

impl McpmError {
    pub fn new(
        code: ErrorCode,
        message: impl Into<String>,
        data: serde_json::Value,
        hint: impl Into<String>,
    ) -> Self {
        Self {
            code,
            message: message.into(),
            data,
            hint: hint.into(),
        }
    }

    pub fn not_found(what: &str, id: &str) -> Self {
        Self::new(
            ErrorCode::NotFound,
            format!("No {what} with id '{id}'."),
            serde_json::json!({ "id": id }),
            "The id is unknown. Re-orient with get_context and use the ids it returns.",
        )
    }

    pub fn internal(err: impl std::fmt::Display) -> Self {
        Self::new(
            ErrorCode::Internal,
            format!("Internal error: {err}"),
            serde_json::Value::Null,
            "This is a server fault, not a protocol violation. Retry once; if it \
             persists, report it to the human operator.",
        )
    }
}

impl From<sqlx::Error> for McpmError {
    fn from(err: sqlx::Error) -> Self {
        McpmError::internal(err)
    }
}
