-- Delegated worker identities: one process tree, more than one agent.
--
-- A key is per MACHINE, and a subagent cannot present a different one —
-- it inherits its parent's MCP configuration wholesale and has no way
-- to vary an Authorization header per spawn. So a delegated identity
-- travels IN BAND, as a tool argument the parent hands the subagent in
-- its prompt, and everything that makes that safe is a column here.
--
-- `key_id` is the pairing rule: a token is only honoured alongside the
-- key that minted it, so one leaked off the machine is inert and the
-- in-band exposure costs nothing. It is NULLABLE because an
-- unauthenticated stdio session has no key to pair against — there is
-- no boundary on a local process the operator started, and refusing to
-- mint there would break the one transport subagents are cheapest on.
-- Resolution matches with IS NOT DISTINCT FROM, so a keyed token and a
-- keyless one never satisfy each other.
--
-- `module_id` is the scope, and it is the reason a delegated identity
-- is safe to hand out: the token names ONE module, and presenting it
-- against any other is refused. A confused subagent cannot wander into
-- a sibling's work even though its parent key holds every claim.
--
-- The secret half is stored as a SHA-256 exactly as in `api_keys`: the
-- token is returned once, to the minting manager, and a dump of this
-- table cannot be replayed.
CREATE TABLE delegations (
    id         TEXT PRIMARY KEY,
    hash       TEXT NOT NULL,
    -- The minting key, or NULL for an unauthenticated stdio session.
    key_id     TEXT REFERENCES api_keys (id),
    -- The identity the ledger records for every write made with this
    -- token. Always a worker: a manager key mints it, but it cannot
    -- inherit the minter's authority or the whole point is lost.
    agent_name TEXT NOT NULL,
    module_id  TEXT NOT NULL REFERENCES modules (id) ON DELETE CASCADE,
    minted_by  TEXT NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    expires_at TIMESTAMPTZ NOT NULL,
    revoked_at TIMESTAMPTZ
);

-- Resolution reads by id and then rejects revoked and expired rows.
CREATE INDEX delegations_live ON delegations (id) WHERE revoked_at IS NULL;

-- Minting for a module, and completing or releasing it, both sweep the
-- module's live tokens. Both match on this.
CREATE INDEX delegations_by_module ON delegations (module_id) WHERE revoked_at IS NULL;
