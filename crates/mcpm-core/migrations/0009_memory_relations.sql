-- Standing relations, co-touch batches, and the weights they need.
--
-- 0008 is applied and therefore frozen (sqlx checksums it); everything
-- here is additive.

-- 1. Standing relations join the same edge table as supersession.
--
-- One table so the graph is one graph — stage 3's traversal walks both
-- kinds of edge without unioning two shapes. The distinction lives in
-- the kind, and exactly one place cares: the `ed` CTE that derives
-- "superseded" filters to the four supersession kinds BY NAME. That is
-- deliberate. A kind added here and forgotten there fails safe — the
-- new relation simply does not withdraw anything — whereas the reverse
-- default would silently retire memories nobody meant to retire.
ALTER TABLE memory_edges DROP CONSTRAINT memory_edges_kind_check;
ALTER TABLE memory_edges ADD CONSTRAINT memory_edges_kind_check CHECK (kind IN (
    -- Supersession: these change the target's state.
    'replaces', 'refutes', 'revises', 'consolidates',
    -- Standing: these describe a relation and change nothing.
    'refines',      -- a narrower case of a broader rule
    'depends_on',   -- true only because the target is
    'contradicts',  -- disagrees, and neither has won yet
    'relates_to'    -- plain association, the weakest claim
));

-- 2. Co-touch batches.
--
-- A bulk `touch_memory` is one act: an agent saying "I leaned on these
-- together". Stamping the batch makes that recoverable, which is what
-- stage 3's suggestions read — memories repeatedly used in the same
-- breath are candidates for a relation. Nullable because every signal
-- written before this migration had no batch, and backfilling one would
-- invent co-occurrence that never happened.
ALTER TABLE memory_signals ADD COLUMN batch UUID;
CREATE INDEX memory_signals_batch ON memory_signals (batch) WHERE batch IS NOT NULL;

-- 3. Weights for the two new adjustments.
INSERT INTO knowledge_weights (name, value, note) VALUES
    ('penalty_machine', 0.7,
     'Subtracted from standing for machine-written records (tagged `system`): completion summaries and blocker reports. They summarise everything, so they match everything, and one containing a searched-for word should not outrank the convention that answers the question. Swept against tests/retrieval_eval.rs: below 0.5 the curated answer loses; the margin plateaus at 0.7 as the clamp saturates. Order-preserving WITHIN the machine set, so kinds=[outcome] is unaffected however large it is.'),
    ('signal_touch_batch', 0.0,
     'Reserved: per-batch weighting of touches, once the eval fixture says whether co-touch frequency should move rank at all. 0 means it does not.')
ON CONFLICT (name) DO NOTHING;

-- 4. Trim the synonyms that cross word SENSES.
--
-- Caught by tests/retrieval_eval.rs, which is why it exists: the
-- question "how should I change the database schema" returned a UI note
-- about scene payloads, because `schema` expanded to `table` and the
-- note says "the table SDK". One word, two unrelated senses, and the
-- expansion cannot tell them apart.
--
-- The lesson generalizes past this row. A synonym is safe only when the
-- words mean the same thing IN THIS PROJECT'S vocabulary, and the
-- riskiest entries are the ones that look most obviously correct in the
-- abstract: table, interface, order, unit are all perfectly good
-- synonyms in one domain and noise in another. When in doubt, leave it
-- out — a missing synonym costs one recall; a wrong one poisons every
-- query containing that word.
UPDATE knowledge_synonyms SET synonyms = ARRAY['migration', 'ddl'] WHERE term = 'schema';
UPDATE knowledge_synonyms SET synonyms = ARRAY['frontend', 'console', 'screen'] WHERE term = 'ui';
UPDATE knowledge_synonyms SET synonyms = ARRAY['endpoint', 'route', 'rpc'] WHERE term = 'api';
UPDATE knowledge_synonyms SET synonyms = ARRAY['stage', 'lock', 'gating'] WHERE term = 'gate';
UPDATE knowledge_synonyms SET synonyms = ARRAY['workstream'] WHERE term = 'module';
