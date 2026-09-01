-- The memory graph: evidence, supersession, and the weights that score
-- them. See KNOWLEDGE.md for the model these tables implement.

-- Evidence about memories, append-only like everything else here.
--
-- Nothing is written when a search RETURNS a memory. A row here means an
-- agent said something about it: `touch` = "I used this", `confirm` =
-- "I checked this and it holds", `dispute` = "this is wrong". A
-- retrieval count would measure what the ranker chose to show and feed
-- that back into the ranker; these are claims someone made.
--
-- No unique constraint: the same agent may touch the same memory many
-- times and every one of those is history. Scoring counts DISTINCT
-- agents at read time, so a loop cannot inflate a score, and the raw log
-- stays complete for whatever we want to ask of it later.
CREATE TABLE memory_signals (
    id        BIGSERIAL PRIMARY KEY,
    memory_id TEXT NOT NULL REFERENCES memories(id),
    agent     TEXT NOT NULL,
    kind      TEXT NOT NULL CHECK (kind IN ('touch', 'confirm', 'dispute')),
    note      TEXT NOT NULL DEFAULT '',
    at        TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX memory_signals_lookup ON memory_signals (memory_id, kind, agent);

-- The supersession spine.
--
-- `from_id` is the newer memory, `to_id` what it supersedes. Edges are
-- only ever declared when the newer memory is COMMITTED, so they always
-- point backwards in creation order — which makes a cycle structurally
-- impossible rather than something to check for. Any future tool that
-- links two pre-existing memories loses that guarantee and must add the
-- check itself.
--
-- The kind is what makes history answerable. "What did we used to
-- believe?" is replaces + refutes; "show me the clean lineage" ignores
-- revises. One untyped edge answers neither.
CREATE TABLE memory_edges (
    id        BIGSERIAL PRIMARY KEY,
    from_id   TEXT NOT NULL REFERENCES memories(id),
    to_id     TEXT NOT NULL REFERENCES memories(id),
    kind      TEXT NOT NULL CHECK (kind IN ('replaces', 'refutes', 'revises', 'consolidates')),
    -- Why this replaced that. The reason a connection exists is a
    -- property of the connection (the same rule want→feature links
    -- follow); a graph of unexplained edges is one nobody trusts.
    rationale TEXT NOT NULL DEFAULT '',
    author    TEXT NOT NULL,
    at        TIMESTAMPTZ NOT NULL DEFAULT now(),
    CHECK (from_id <> to_id),
    UNIQUE (from_id, to_id)
);
CREATE INDEX memory_edges_to   ON memory_edges (to_id);
CREATE INDEX memory_edges_from ON memory_edges (from_id);

-- The scoring constants.
--
-- A table for the same reason `knowledge_synonyms` is one: editable with
-- plain SQL, in effect on the next query, with the defaults seeded here
-- so the starting point is version-controlled. No score is ever stored
-- on a memory — these weights are applied at read time over the signals
-- above, so changing one re-ranks the whole base with no backfill and
-- leaves every entry comparable to every other.
CREATE TABLE knowledge_weights (
    name  TEXT PRIMARY KEY,
    value REAL NOT NULL,
    note  TEXT NOT NULL DEFAULT ''
);

INSERT INTO knowledge_weights (name, value, note) VALUES
    ('signal_touch',   0.15,
     'Per distinct agent that used this. Small: "I relied on it" is weaker than "I checked it".'),
    ('signal_confirm', 1.0,
     'Per distinct agent that verified it holds.'),
    ('signal_dispute', -2.0,
     'Per distinct agent that says it is wrong. Outweighs a confirm: being told something is broken is stronger evidence than being told it works.'),
    ('sigmoid_k',      0.25,
     'Saturation of tanh(k * raw). Set deliberately: at k=1 three confirms and twenty are both 1.00, so the score stops carrying information exactly where it starts mattering. 0.25 keeps resolution across 1-10 confirms.'),
    ('decay_strength', 0.5,
     'How far the newest-to-oldest position within a kind can pull standing down.'),
    ('rank_floor',     0.2,
     'The lowest rank multiplier any memory can reach. NEVER 0: a memory that cannot be found has been deleted, whatever the schema says.'),
    ('penalty_superseded', 0.6,
     'Subtracted from standing once something replaces it. Without this a well-confirmed old belief outranks the entry that corrected it, because evidence accrued while it was current never goes away.'),
    ('penalty_disputed', 0.8,
     'Subtracted while more agents call it wrong than right. Heavier than supersession: a supersession has a successor to read instead, a dispute has nothing.');
