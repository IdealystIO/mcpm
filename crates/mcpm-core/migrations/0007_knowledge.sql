-- The memory table becomes the project's knowledge base.
--
-- Three things were missing for that. Knowledge that is not ABOUT a
-- feature had nowhere to live; there was no way to ask for a KIND of
-- knowledge; and search matched only exact lexemes, so a question
-- phrased differently from the note never found it.

-- 1. Project scope.
--
-- Every memory previously had to pin to a node of the tree, which meant
-- a standing convention ("migrations are additive") had to be filed
-- under whichever feature happened to be open when someone noticed it —
-- and then went looking like a fact about that feature. `project` is
-- the scope for knowledge that outlives any one piece of work; it is
-- also what `direction = 'up'` now terminates at, so a worker reading
-- the conventions above its module reaches the project's.
ALTER TABLE memories DROP CONSTRAINT memories_level_check;
ALTER TABLE memories ADD CONSTRAINT memories_level_check
    CHECK (level IN ('project', 'feature', 'stage', 'module', 'task'));

-- 2. Kind.
--
-- A closed set, not free text: it is enumerated in the MCP tool schema,
-- so an agent sees the whole vocabulary and picks from it. `note` is
-- the unclassified default every existing row backfills to — it means
-- "nobody said", which is honestly different from any of the others.
ALTER TABLE memories ADD COLUMN kind TEXT NOT NULL DEFAULT 'note'
    CHECK (kind IN ('convention', 'decision', 'gotcha', 'outcome', 'reference', 'note'));

-- The filter columns. Each one is a facet the search exposes, so each
-- one is indexed: an unindexed facet is a sequential scan wearing a
-- filter's clothes.
CREATE INDEX memories_kind_idx    ON memories (kind);
CREATE INDEX memories_author_idx  ON memories (author);
CREATE INDEX memories_created_idx ON memories (created_at DESC);
CREATE INDEX memories_tags_idx    ON memories USING GIN (tags);

-- 3. Fuzzy matching.
--
-- The tsvector index handles words it can stem to a shared root.
-- Trigrams handle what it cannot: a typo, a truncation, an identifier
-- spelled slightly differently. The two are unioned at query time —
-- neither is a replacement for the other.
CREATE EXTENSION IF NOT EXISTS pg_trgm;
CREATE INDEX memories_trgm_idx ON memories USING GIN (content gin_trgm_ops);

-- 4. Synonyms.
--
-- Postgres' own synonym dictionaries live in files under the server's
-- share directory, which a migration cannot write and a managed or
-- containerized Postgres will not let us near. So expansion happens in
-- the query builder instead, reading this table: a term the caller
-- typed is OR'd with everything listed against it before the tsquery is
-- assembled.
--
-- A table rather than a constant in the binary because a project's
-- vocabulary is the project's, not ours — this one is editable with
-- plain SQL and takes effect on the next query, with no redeploy.
CREATE TABLE knowledge_synonyms (
    term     TEXT PRIMARY KEY,
    synonyms TEXT[] NOT NULL
);

-- A starting vocabulary: the words this system's own domain uses for
-- the same thing, plus the handful of general development terms whose
-- stems genuinely differ. Deliberately small — a synonym list that
-- guesses becomes a list that returns the wrong answers confidently.
INSERT INTO knowledge_synonyms (term, synonyms) VALUES
    ('db',         ARRAY['database', 'postgres', 'postgresql', 'sql']),
    ('database',   ARRAY['db', 'postgres', 'postgresql']),
    ('migration',  ARRAY['schema', 'ddl', 'migrate']),
    ('schema',     ARRAY['migration', 'ddl', 'table']),
    ('auth',       ARRAY['authentication', 'authorization', 'credential', 'key', 'token']),
    ('key',        ARRAY['credential', 'token', 'auth', 'secret']),
    ('token',      ARRAY['credential', 'key', 'auth', 'secret']),
    ('config',     ARRAY['configuration', 'setting', 'env', 'environment']),
    ('env',        ARRAY['environment', 'config', 'variable']),
    ('test',       ARRAY['testing', 'spec', 'assertion', 'coverage']),
    ('bug',        ARRAY['defect', 'issue', 'fault', 'regression']),
    ('ui',         ARRAY['interface', 'frontend', 'console', 'screen']),
    ('api',        ARRAY['endpoint', 'route', 'rpc', 'interface']),
    ('deploy',     ARRAY['deployment', 'release', 'ship', 'rollout']),
    ('perf',       ARRAY['performance', 'latency', 'throughput', 'speed']),
    ('doc',        ARRAY['documentation', 'docs', 'readme']),
    -- This project's own nouns.
    ('want',       ARRAY['idea', 'request', 'pool']),
    ('module',     ARRAY['unit', 'workstream']),
    ('gate',       ARRAY['stage', 'lock', 'gating', 'order']),
    ('claim',      ARRAY['lock', 'assignment', 'ownership'])
ON CONFLICT (term) DO NOTHING;
