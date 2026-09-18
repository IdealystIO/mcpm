-- Agent health: whether the MACHINE behind an agent is actually there.
--
-- The roster already knows what an agent holds (its live claims) and
-- when it last spoke (`last_seen`), and neither answers the question a
-- manager has when a box goes quiet. A quota-parked box holds its
-- claims and says nothing; a reclaimed spot instance holds its claims
-- and no longer exists; a box whose dev server is mid-rebuild is fine.
-- From the ledger the three are one silence.
--
-- `health_url` is where to ask. It is registered by the manager when
-- it issues the box's key (`issue_worker_key`), or by the agent itself
-- on `get_context`, and the MCP server probes it on a timer. The
-- verdict is the other columns. They are STORED — unlike readiness,
-- which is derived — because a probe is an observation of something
-- outside the database, not a fact the database can recompute; the
-- rule that turns a response into a state still lives in one place
-- (`HealthState::classify`), and a change of state is an event, so the
-- ledger says when a box went down and the console hears about it the
-- way it hears about everything else.
--
-- `health_url` NULL means no check is configured for this agent, and
-- a NULL `health_state` beside a URL means it has not been probed yet.
ALTER TABLE agents
    ADD COLUMN health_url        TEXT,
    ADD COLUMN health_state      TEXT
                                 CHECK (health_state IN ('up', 'down', 'gone', 'unreachable')),
    ADD COLUMN health_detail     TEXT NOT NULL DEFAULT '',
    ADD COLUMN health_checked_at TIMESTAMPTZ,
    -- When the current state was first observed: "down since".
    ADD COLUMN health_since      TIMESTAMPTZ;

COMMENT ON COLUMN agents.health_url IS
    'Where the server probes to learn whether this agent''s machine is serving. '
    'Accepted only under the host suffixes the server is configured with '
    '(MCPM_HEALTH_HOSTS); NULL means no check.';
COMMENT ON COLUMN agents.health_state IS
    'The last probe''s verdict: up (answered 2xx/3xx/4xx other than 404), down (the front '
    'door answered 5xx — the host is registered but nothing behind it is serving), gone '
    '(404 — nothing is registered at that name any more), unreachable (no HTTP answer at '
    'all: the network between the server and the URL, not the box). NULL = not probed yet.';
