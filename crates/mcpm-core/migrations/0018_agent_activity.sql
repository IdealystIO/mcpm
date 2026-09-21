-- "When did this agent last do anything?" — the liveness signal behind
-- the console's spinners. A module is drawn as MOVING while the agent
-- holding it has written to the ledger recently, and as merely held
-- once it has gone quiet; the ledger already carries every write, so
-- the answer is one index probe per claimant rather than a new column
-- that some path would forget to bump.
CREATE INDEX events_agent_idx ON events (agent, seq DESC);
