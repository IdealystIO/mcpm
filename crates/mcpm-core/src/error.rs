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
    /// No usable credential was presented: the header is missing,
    /// malformed, unknown, or revoked.
    Unauthorized,
    /// The credential is good but does not authorize this call — a
    /// worker key on a manager-only tool, a console key on the MCP
    /// surface.
    Forbidden,
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

    /// No usable credential. The hint is deliberately incurious about
    /// WHICH way the key failed — unknown, revoked and malformed all
    /// read the same to a caller, so a probe learns nothing from the
    /// wording.
    pub fn unauthorized() -> Self {
        Self::new(
            ErrorCode::Unauthorized,
            "This request carries no valid API key.",
            serde_json::Value::Null,
            "Send `Authorization: Bearer <token>` with a key issued by this deployment \
             (`mcpm-mcp --issue-key`). If yours was working, it has been revoked — ask \
             the operator for a new one.",
        )
    }

    /// A delegation token that resolved to nothing, and WHY.
    ///
    /// Unlike [`unauthorized`](Self::unauthorized) these are
    /// forthcoming, because the caller has ALREADY authenticated — it
    /// is a subagent holding a token its manager handed it, and the
    /// four causes want four different reactions from it. Collapsing
    /// them into one sentence made a subagent that could not tell an
    /// operator error from a dead credential block and escalate, which
    /// is the correct move given four causes and no way to choose.
    ///
    /// The one thing they share: never fall back to the session's own
    /// identity. That would attribute a subagent's work to the machine,
    /// quietly, in exactly the case nobody is watching.
    ///
    /// A probe learns nothing here that the key it already presented
    /// did not tell it — with one deliberate exception, spelled out on
    /// [`delegation_wrong_key`](Self::delegation_wrong_key).
    fn delegation_dead(message: &str, data: serde_json::Value, remedy: &str) -> Self {
        Self::new(
            ErrorCode::Unauthorized,
            message.to_string(),
            data,
            format!(
                "{remedy} Do not fall back to calling without the token — that would \
                 attribute your work to the machine instead of to you."
            ),
        )
    }

    /// No such token: malformed, never minted, or the secret half does
    /// not match. Deliberately one error and not three — those three
    /// differ only for someone guessing, and a guesser is the one
    /// caller who must learn nothing.
    pub fn delegation_unknown() -> Self {
        Self::delegation_dead(
            "This delegation_token does not resolve to any identity.",
            serde_json::Value::Null,
            "The token is not one this server minted, or it was copied incompletely. \
             Check you pasted all of it; if it is intact, ask your manager for a fresh \
             mint_worker. Retrying will not help.",
        )
    }

    /// The token was real and is now past its TTL.
    pub fn delegation_expired(expires_at: chrono::DateTime<chrono::Utc>) -> Self {
        Self::delegation_dead(
            "This delegation_token has expired.",
            serde_json::json!({ "expires_at": expires_at }),
            "A token is time-boxed on purpose. Report where you got to and ask your \
             manager for a fresh mint_worker — with a longer ttl_minutes if the work \
             legitimately runs this long.",
        )
    }

    /// The token was retired: its module completed or was released, or
    /// the manager minted a replacement for the same module.
    pub fn delegation_retired() -> Self {
        Self::delegation_dead(
            "This delegation_token has been retired.",
            serde_json::Value::Null,
            "Its module was completed or released, or your manager minted a replacement \
             identity for it — which means someone else is now doing this work. Stop and \
             report to your manager rather than asking for another token.",
        )
    }

    /// The token is structurally fine and presented by the wrong key.
    ///
    /// This is an OPERATOR error with a specific remedy, and it used to
    /// read as a dead credential. The bound key id is named because a
    /// key id is the public half by design (`--list-keys` prints it),
    /// the caller already holds both the token and a valid key, and
    /// without it the remedy — present the right key, or re-mint bound
    /// to this one — is not actionable from the error.
    pub fn delegation_wrong_key(bound: Option<&str>, presented: Option<&str>) -> Self {
        let where_it_works = match bound {
            Some(id) => format!("the key '{id}' it was minted for"),
            None => "a local stdio session with no key at all".to_string(),
        };
        let presenting = match presented {
            Some(id) => format!("key '{id}'"),
            None => "no key".to_string(),
        };
        Self::delegation_dead(
            &format!(
                "This delegation_token is live, but it is bound to a different \
                 credential: it is only honoured alongside {where_it_works}, and this \
                 request presented {presenting}."
            ),
            serde_json::json!({ "bound_key_id": bound, "presented_key_id": presented }),
            "A token is paired to one key so that one leaked off its machine is inert. \
             You are on the wrong machine for it. Report this to your manager: either \
             you should be running where that key lives, or the token needs re-minting \
             with for_key_id set to the key you actually hold. Retrying will not help.",
        )
    }

    /// A good credential, used somewhere it does not reach.
    pub fn forbidden(action: &str, role: crate::KeyRole) -> Self {
        Self::new(
            ErrorCode::Forbidden,
            format!("A {} key may not {action}.", role.as_str()),
            serde_json::json!({ "role": role.as_str(), "action": action }),
            "This is a permission boundary, not a transient failure — retrying will not \
             help. Report it to your manager, who holds a key that can.",
        )
    }

    /// A delegated identity reaching outside the one module it was
    /// minted for. This is the rule that makes an in-band token safe to
    /// paste into a prompt, so it is enforced on every write and not
    /// only on the claim.
    pub fn out_of_scope(module_id: &str, scope: &str) -> Self {
        Self::new(
            ErrorCode::Forbidden,
            format!(
                "Your delegation is scoped to module '{scope}' — it cannot touch \
                 '{module_id}'."
            ),
            serde_json::json!({ "module_id": module_id, "scope": scope }),
            "You were minted for one module. Work that one; if the work you found belongs \
             to another, report it to your manager rather than reaching for it — retrying \
             will not help.",
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
