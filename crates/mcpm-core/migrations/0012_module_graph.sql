-- Stages become a projection; modules carry their prerequisites.
--
-- ============================================================
-- WHY
-- ============================================================
--
-- A stage was a convenience: "plan sequentially, parallelize within
-- each layer". It expressed exactly one shape of dependency — every
-- module in stage N waits on every module in stage N-1 — and real work
-- is a graph, not a ladder. Measured on one feature (crewforge
-- feat_967547fa, 2026-09-10): stage 2 held three modules that all
-- edited one file, so the manager serialized them by PROMPT TEXT while
-- the plan said they were concurrent. The tool could not say no,
-- because the tool had no place to write the dependency down.
--
-- Now it does. `module_deps` is the graph, and the one coordination
-- rule the server owns changes from "every module in every earlier
-- stage is done" to "every prerequisite of THIS module is done". It is
-- still checked in exactly one place (`claim_module`), inside the same
-- transaction that takes the claim.
--
-- Everything a stage used to answer is DERIVED from the graph at read
-- time, like every other status here: ready = prerequisites done;
-- depth = longest path from a root. Nothing new is stored.
--
-- ============================================================
-- THE LOWERING, which is also the backfill
-- ============================================================
--
-- A feature planned in stages lowers to edges: each module depends on
-- every module of the immediately preceding stage. Transitivity carries
-- the rest, and the gate it produces is IDENTICAL to the old one — a
-- stage-3 module is claimable exactly when every stage-2 module is
-- done, and a stage-2 module could only have finished after stage 1
-- did. So a feature in flight at deploy answers `next_work` the same
-- way before and after this file runs.
--
-- The same lowering serves `plan_feature` for callers that still send
-- `stages`, so every prompt written against the old shape keeps
-- working while the docs move.

-- 1. Modules hang off the feature.
ALTER TABLE modules ADD COLUMN feature_id TEXT REFERENCES features (id) ON DELETE CASCADE;
UPDATE modules m SET feature_id = s.feature_id FROM stages s WHERE s.id = m.stage_id;
ALTER TABLE modules ALTER COLUMN feature_id SET NOT NULL;

-- Optional ownership: the path prefixes a module will write to. Empty
-- means undeclared, and undeclared is exempt from the overlap check —
-- making it required would break every existing prompt.
ALTER TABLE modules ADD COLUMN owns TEXT[] NOT NULL DEFAULT '{}';

-- 2. The graph.
CREATE TABLE module_deps (
    module_id  TEXT NOT NULL REFERENCES modules (id) ON DELETE CASCADE,
    depends_on TEXT NOT NULL REFERENCES modules (id) ON DELETE CASCADE,
    PRIMARY KEY (module_id, depends_on),
    CHECK (module_id <> depends_on)
);
CREATE INDEX module_deps_by_prereq ON module_deps (depends_on);

-- Backfill: stage N depends on stage N-1, within one feature.
INSERT INTO module_deps (module_id, depends_on)
SELECT m.id, p.id
FROM modules m
JOIN stages s  ON s.id = m.stage_id
JOIN stages ps ON ps.feature_id = s.feature_id AND ps.position = s.position - 1
JOIN modules p ON p.stage_id = ps.id;

-- 3. Names are unique per FEATURE now, not per stage. Two stages of one
-- feature could carry the same module name; suffix the later one with
-- its old stage position so the constraint can land without a human.
UPDATE modules m SET name = m.name || ' (' || s.position || ')'
FROM stages s
WHERE s.id = m.stage_id
  AND EXISTS (SELECT 1 FROM modules o JOIN stages os ON os.id = o.stage_id
              WHERE o.feature_id = m.feature_id AND o.name = m.name
                AND o.id <> m.id AND os.position < s.position);
ALTER TABLE modules DROP CONSTRAINT modules_stage_id_name_key;
ALTER TABLE modules ADD CONSTRAINT modules_feature_id_name_key UNIQUE (feature_id, name);

-- 4. Knowledge pinned to a stage moves up to its feature. A subject
-- class that ceases to exist is relocated, not edited: the content is
-- untouched, and the tag says where it used to hang. Memory rows are
-- otherwise immutable (KNOWLEDGE.md), and this is the one migration
-- that touches their scope.
UPDATE memories mem
SET level = 'feature',
    subject_id = s.feature_id,
    tags = array_append(mem.tags, 'from-stage')
FROM stages s
WHERE mem.level = 'stage' AND mem.subject_id = s.id;
-- A stage memory whose stage is already gone has nowhere to go; keep it
-- searchable at project level rather than lose it.
UPDATE memories SET level = 'project', subject_id = 'proj_main',
                    tags = array_append(tags, 'from-stage')
WHERE level = 'stage';
ALTER TABLE memories DROP CONSTRAINT memories_level_check;
ALTER TABLE memories ADD CONSTRAINT memories_level_check
    CHECK (level IN ('project', 'feature', 'module', 'task'));

UPDATE knowledge_synonyms
   SET synonyms = ARRAY['prerequisite', 'depends', 'lock', 'gating', 'order']
 WHERE term = 'gate';

-- 5. Documents: the long-form record beside the memories.
--
-- A memory is a FACT, retrieved by relevance. A document is read whole,
-- by identity: the feature's whitepaper (the plan as prose, what the
-- manager would say to a new hire), and a module's handoff (how to use
-- what this module built — the component, the function, its
-- parameters — written for the agent that continues the work).
-- Append-only revisions; the current one is the newest.
CREATE TABLE documents (
    id         TEXT PRIMARY KEY,
    level      TEXT NOT NULL CHECK (level IN ('feature', 'module')),
    subject_id TEXT NOT NULL,
    kind       TEXT NOT NULL CHECK (kind IN ('whitepaper', 'handoff')),
    revision   INT  NOT NULL,
    title      TEXT NOT NULL DEFAULT '',
    body       TEXT NOT NULL,
    author     TEXT NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (level, subject_id, kind, revision)
);
CREATE INDEX documents_by_subject ON documents (level, subject_id, kind, revision DESC);

-- 6. The stage table goes. Historical events keep their `stage`
-- payloads; the formatter still renders them.
ALTER TABLE modules DROP COLUMN stage_id;
DROP TABLE stages;
