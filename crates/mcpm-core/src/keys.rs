//! API keys — the token format and the identity a verified key carries.
//!
//! A key is two halves joined by an underscore: `mcpm_<id>_<secret>`.
//! The **id** is public — stored in the clear, indexed, and printed by
//! `--list-keys`. The **secret** is shown once at issue time and only
//! its SHA-256 is stored, so the table is not a set of usable
//! credentials.
//!
//! Everything here is pure: minting, parsing, hashing. The rules that
//! decide whether a key is *accepted* live in [`crate::Store`] with
//! every other invariant.

use serde::{Deserialize, Serialize};

/// The prefix every key carries, so a leaked one is greppable and
/// obviously ours.
const TOKEN_PREFIX: &str = "mcpm";
/// The prefix a delegation token carries. Deliberately different from
/// the key prefix: the two are accepted in different places, and a
/// token pasted into the wrong one must fail on shape rather than
/// round-trip to the database and fail on lookup.
const DELEGATION_PREFIX: &str = "dlg";
/// Bytes in the public half (→ 12 hex chars).
const ID_BYTES: usize = 6;
/// Bytes in the secret half (→ 64 hex chars, 256 bits).
const SECRET_BYTES: usize = 32;

/// What a key authorizes its bearer to be.
///
/// `Manager` and `Worker` are the two agent roles the MCP server already
/// dispatches on; `Console` is the read/capture surface the web console
/// uses and is refused by the MCP endpoint outright — a console key
/// that leaks cannot claim a module or revise a plan.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum KeyRole {
    Manager,
    Worker,
    Console,
}

impl KeyRole {
    pub fn as_str(self) -> &'static str {
        match self {
            KeyRole::Manager => "manager",
            KeyRole::Worker => "worker",
            KeyRole::Console => "console",
        }
    }

    pub fn parse(s: &str) -> Option<KeyRole> {
        match s {
            "manager" => Some(KeyRole::Manager),
            "worker" => Some(KeyRole::Worker),
            "console" => Some(KeyRole::Console),
            _ => None,
        }
    }

    /// Whether this role may call the MCP tool surface at all.
    pub fn is_agent(self) -> bool {
        matches!(self, KeyRole::Manager | KeyRole::Worker)
    }

    /// Whether a key of this role may call `tool`.
    ///
    /// Defined here rather than in the transport so both the stdio and
    /// the HTTP dispatcher consult ONE list — a tool added to the
    /// manager surface must not stay callable on one transport because
    /// somebody updated the other.
    pub fn may_call(self, tool: &str) -> bool {
        match self {
            KeyRole::Manager => true,
            KeyRole::Worker => !MANAGER_ONLY.contains(&tool),
            // A console key authenticates the dashboard, which reads the
            // store directly. It is not an agent and holds no claim.
            KeyRole::Console => false,
        }
    }
}

/// The tools that shape the plan rather than execute it. A worker that
/// could call these could hand itself work, re-cut a stage around a
/// module it had already failed, or close a feature it had not
/// finished — the gate the whole system exists to enforce would then
/// only bind agents that chose to respect it.
///
/// `mint_worker` is here for a second reason as well: a delegated
/// identity is gated as a worker whatever key carried it, so listing
/// the tool here is what makes delegation exactly one level deep.
pub const MANAGER_ONLY: &[&str] = &[
    "plan_feature",
    "revise_plan",
    "complete_feature",
    "promote_wants",
    "mint_worker",
];

/// A verified key, resolved to who is calling. This is what the MCP
/// server and the console host attribute writes to — never a
/// caller-supplied name.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct KeyIdentity {
    /// The key's public half, for the audit trail.
    pub key_id: String,
    pub label: String,
    /// The agent name every write by this key is recorded under.
    pub agent_name: String,
    pub role: KeyRole,
}

/// Who a write is attributed to, and what it is confined to.
///
/// This is what the store takes instead of a bare agent name, because
/// a name alone cannot express the one thing a delegated identity adds:
/// a boundary. A key's own identity carries `scope: None` and may write
/// to any module it holds the claim on. A minted sub-identity carries
/// `Some(module_id)` and may write to that module and nothing else —
/// however many claims the key that minted it happens to hold.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Actor {
    /// The name the event ledger records.
    pub name: String,
    /// The single module a delegated identity is confined to.
    pub scope: Option<String>,
}

impl Actor {
    /// An identity that speaks for itself: a key's own agent name, or
    /// the name declared on an unauthenticated stdio session.
    pub fn new(name: impl Into<String>) -> Actor {
        Actor { name: name.into(), scope: None }
    }

    /// A minted sub-identity, confined to the module it was minted for.
    pub fn delegated(name: impl Into<String>, module_id: impl Into<String>) -> Actor {
        Actor { name: name.into(), scope: Some(module_id.into()) }
    }

    /// Whether this actor's authority came from a delegation token
    /// rather than from a key of its own. Delegation is one level deep:
    /// a delegated actor cannot mint.
    pub fn is_delegated(&self) -> bool {
        self.scope.is_some()
    }
}

impl From<&str> for Actor {
    fn from(name: &str) -> Actor {
        Actor::new(name)
    }
}

impl From<String> for Actor {
    fn from(name: String) -> Actor {
        Actor::new(name)
    }
}

impl From<&Actor> for Actor {
    fn from(actor: &Actor) -> Actor {
        actor.clone()
    }
}

/// A freshly minted delegation. `token` is the only time the secret
/// exists outside the minting manager's hands — it goes straight into
/// the subagent's prompt.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct MintedWorker {
    pub delegation_token: String,
    pub agent_name: String,
    pub module_id: String,
    pub expires_at: chrono::DateTime<chrono::Utc>,
    /// What the manager should paste into the subagent's prompt, spelled
    /// out — the token is useless to a subagent that was not told it
    /// must pass it on every write.
    pub instructions: String,
}

/// A delegation token resolved back to the identity it carries. The
/// scope is not advisory: it is what [`Actor::delegated`] is built from.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Delegation {
    pub id: String,
    pub agent_name: String,
    pub module_id: String,
    pub expires_at: chrono::DateTime<chrono::Utc>,
}

/// One key as `--list-keys` and the console show it. Carries no secret
/// and no hash — there is nothing here worth stealing.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ApiKeyInfo {
    pub id: String,
    pub label: String,
    pub agent_name: String,
    pub role: KeyRole,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub created_by: String,
    pub last_used: Option<chrono::DateTime<chrono::Utc>>,
    pub revoked_at: Option<chrono::DateTime<chrono::Utc>>,
}

impl ApiKeyInfo {
    pub fn is_live(&self) -> bool {
        self.revoked_at.is_none()
    }
}

/// A freshly minted key. `token` is the ONLY time the secret exists
/// outside the bearer's hands — the caller prints it and drops it.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct IssuedKey {
    pub token: String,
    pub info: ApiKeyInfo,
}

/// A token's two halves, split but not yet verified.
pub struct ParsedToken {
    pub id: String,
    /// SHA-256 of the secret half, hex — what the store compares.
    pub secret_hash: String,
}

/// Mint `(token, id, secret_hash)` from the OS CSPRNG.
pub fn mint() -> (String, String, String) {
    mint_with_prefix(TOKEN_PREFIX)
}

/// Mint a delegation token — same construction, same entropy, its own
/// prefix. It is a weaker credential than a key by SCOPE (one module,
/// worker role, a TTL, and only alongside the key that minted it), not
/// by how hard it is to guess.
pub fn mint_delegation() -> (String, String, String) {
    mint_with_prefix(DELEGATION_PREFIX)
}

fn mint_with_prefix(prefix: &str) -> (String, String, String) {
    let id = hex(&random_bytes(ID_BYTES));
    let secret = hex(&random_bytes(SECRET_BYTES));
    let token = format!("{prefix}_{id}_{secret}");
    let secret_hash = sha256_hex(&secret);
    (token, id, secret_hash)
}

/// Split a presented token into its public id and the hash of its
/// secret. `None` for anything that is not shaped like one of ours —
/// checked before any database round trip, so a malformed header costs
/// nothing.
pub fn parse(token: &str) -> Option<ParsedToken> {
    parse_with_prefix(TOKEN_PREFIX, token)
}

/// The same, for a delegation token. A key presented here — or a
/// delegation token presented to [`parse`] — is refused on its prefix,
/// before any database round trip.
pub fn parse_delegation(token: &str) -> Option<ParsedToken> {
    parse_with_prefix(DELEGATION_PREFIX, token)
}

fn parse_with_prefix(prefix: &str, token: &str) -> Option<ParsedToken> {
    let rest = token.trim().strip_prefix(prefix)?.strip_prefix('_')?;
    let (id, secret) = rest.split_once('_')?;
    if id.len() != ID_BYTES * 2 || secret.len() != SECRET_BYTES * 2 {
        return None;
    }
    if !id.bytes().chain(secret.bytes()).all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    Some(ParsedToken {
        id: id.to_string(),
        secret_hash: sha256_hex(secret),
    })
}

/// Pull the token out of an `Authorization: Bearer <token>` header
/// value. The scheme match is case-insensitive per RFC 7235.
pub fn from_bearer(header: &str) -> Option<&str> {
    let (scheme, token) = header.split_once(' ')?;
    scheme
        .eq_ignore_ascii_case("bearer")
        .then(|| token.trim())
        .filter(|t| !t.is_empty())
}

/// How a key is displayed once its secret is gone: enough to recognize
/// it in a list, not enough to use.
pub fn display_key(id: &str) -> String {
    format!("{TOKEN_PREFIX}_{id}_…")
}

/// Constant-time equality over two hex digests. A short-circuiting
/// `==` on a hash leaks, through timing, how many leading bytes a
/// guess got right — which is enough to walk a forged digest byte by
/// byte if the attacker can also produce the matching preimage.
pub fn hash_eq(a: &str, b: &str) -> bool {
    if a.len() != b.len() {
        return false;
    }
    a.bytes()
        .zip(b.bytes())
        .fold(0u8, |acc, (x, y)| acc | (x ^ y))
        == 0
}

fn random_bytes(n: usize) -> Vec<u8> {
    use rand::RngCore;
    let mut buf = vec![0u8; n];
    // The OS generator, not a thread-local PRNG: these are credentials.
    rand::rngs::OsRng.fill_bytes(&mut buf);
    buf
}

fn sha256_hex(input: &str) -> String {
    use sha2::{Digest, Sha256};
    hex(&Sha256::digest(input.as_bytes()))
}

fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write;
    bytes.iter().fold(String::new(), |mut s, b| {
        let _ = write!(s, "{b:02x}");
        s
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_minted_token_parses_back_to_its_own_id_and_hash() {
        let (token, id, secret_hash) = mint();
        let parsed = parse(&token).expect("a freshly minted token must parse");
        assert_eq!(parsed.id, id);
        assert!(hash_eq(&parsed.secret_hash, &secret_hash));
    }

    #[test]
    fn two_keys_never_collide() {
        let (a, ..) = mint();
        let (b, ..) = mint();
        assert_ne!(a, b);
    }

    /// Malformed input must be rejected on shape alone — the store is
    /// never asked about a token that cannot be one of ours.
    #[test]
    fn junk_is_refused_before_the_database() {
        for junk in [
            "",
            "mcpm",
            "mcpm_",
            "mcpm_short_deadbeef",
            "other_aabbccddeeff_00",
            "mcpm_aabbccddeeff_nothexnothexnothex",
        ] {
            assert!(parse(junk).is_none(), "{junk:?} must not parse");
        }
        // Right shape, wrong length on the secret half.
        let (token, ..) = mint();
        assert!(parse(&token[..token.len() - 1]).is_none());
    }

    #[test]
    fn bearer_is_scheme_insensitive_and_rejects_other_schemes() {
        assert_eq!(from_bearer("Bearer abc"), Some("abc"));
        assert_eq!(from_bearer("bearer abc"), Some("abc"));
        assert_eq!(from_bearer("BEARER abc"), Some("abc"));
        assert_eq!(from_bearer("Basic abc"), None);
        assert_eq!(from_bearer("Bearer "), None);
        assert_eq!(from_bearer("abc"), None);
    }

    #[test]
    fn a_worker_key_cannot_reach_the_planning_tools() {
        for tool in MANAGER_ONLY {
            assert!(KeyRole::Manager.may_call(tool), "manager may {tool}");
            assert!(!KeyRole::Worker.may_call(tool), "worker may NOT {tool}");
        }
        // The worker surface itself stays open.
        for tool in ["claim_module", "complete_task", "complete_module", "report_blocker"] {
            assert!(KeyRole::Worker.may_call(tool));
        }
        // A console key is not an agent at all.
        assert!(!KeyRole::Console.may_call("claim_module"));
        assert!(!KeyRole::Console.is_agent());
    }

    /// The two token kinds are accepted in different places and mean
    /// very different things. Neither may be mistaken for the other,
    /// and the refusal must happen on shape — before a lookup that
    /// would otherwise turn a pasted-in-the-wrong-box credential into a
    /// database round trip.
    #[test]
    fn a_key_and_a_delegation_token_never_parse_as_each_other() {
        let (key, ..) = mint();
        let (delegation, ..) = mint_delegation();
        assert!(parse(&key).is_some());
        assert!(parse_delegation(&delegation).is_some());
        assert!(parse(&delegation).is_none(), "a delegation token is not a key");
        assert!(parse_delegation(&key).is_none(), "a key is not a delegation token");
    }

    #[test]
    fn a_minted_delegation_parses_back_to_its_own_id_and_hash() {
        let (token, id, secret_hash) = mint_delegation();
        let parsed = parse_delegation(&token).expect("a freshly minted token must parse");
        assert_eq!(parsed.id, id);
        assert!(hash_eq(&parsed.secret_hash, &secret_hash));
    }

    #[test]
    fn hash_eq_matches_str_eq_on_equal_length_input() {
        assert!(hash_eq("abcd", "abcd"));
        assert!(!hash_eq("abcd", "abce"));
        assert!(!hash_eq("abcd", "abc"));
    }
}
