-- API keys: the identity a remote agent presents instead of typing its
-- own name into get_context.
--
-- A key is split in two. `id` is the PUBLIC half — it is stored in the
-- clear, it is what a lookup matches on, and it is what `--list-keys`
-- and the console print. `hash` is the SHA-256 of the SECRET half,
-- which is shown exactly once at issue time and never stored: a dump of
-- this table cannot be replayed against the server.
--
-- `agent_name` and `role` are what make the key an identity rather than
-- a gate. The MCP server reads them off the verified key and attributes
-- every write to them, so the event ledger's `agent` column stops being
-- self-reported. Role is also the authorization: manager-only tools
-- reject a worker key, and a `console` key cannot call MCP tools at all.
CREATE TABLE api_keys (
    id         TEXT PRIMARY KEY,
    hash       TEXT NOT NULL,
    label      TEXT NOT NULL,
    agent_name TEXT NOT NULL,
    role       TEXT NOT NULL CHECK (role IN ('manager', 'worker', 'console')),
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    created_by TEXT NOT NULL,
    last_used  TIMESTAMPTZ,
    revoked_at TIMESTAMPTZ
);

-- Verification reads by id and then rejects revoked rows; the partial
-- index keeps that path off a sequential scan as keys accumulate.
CREATE INDEX api_keys_live ON api_keys (id) WHERE revoked_at IS NULL;
