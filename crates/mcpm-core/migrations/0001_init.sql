-- Control Center schema (architecture doc §8).
--
-- Held state only: stage and feature *statuses* are derived from module
-- and task rows at read time, so the gate can never drift from the
-- checklist truth. Events are append-only; memories are immutable.

CREATE TABLE project (
    id          SMALLINT PRIMARY KEY DEFAULT 1 CHECK (id = 1),
    name        TEXT NOT NULL,
    repo_path   TEXT NOT NULL DEFAULT '',
    description TEXT NOT NULL DEFAULT '',
    created_at  TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE TABLE agents (
    name       TEXT PRIMARY KEY,
    role       TEXT NOT NULL CHECK (role IN ('manager', 'worker', 'observer')),
    first_seen TIMESTAMPTZ NOT NULL DEFAULT now(),
    last_seen  TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE TABLE features (
    id          TEXT PRIMARY KEY,
    name        TEXT NOT NULL UNIQUE,
    description TEXT NOT NULL DEFAULT '',
    status      TEXT NOT NULL DEFAULT 'planning'
                CHECK (status IN ('planning', 'in_progress', 'done', 'shelved')),
    summary     TEXT,
    created_by  TEXT,
    created_at  TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE TABLE stages (
    id         TEXT PRIMARY KEY,
    feature_id TEXT NOT NULL REFERENCES features (id) ON DELETE CASCADE,
    name       TEXT NOT NULL,
    position   INT NOT NULL,
    UNIQUE (feature_id, name),
    UNIQUE (feature_id, position) DEFERRABLE INITIALLY DEFERRED
);

CREATE TABLE modules (
    id          TEXT PRIMARY KEY,
    stage_id    TEXT NOT NULL REFERENCES stages (id) ON DELETE CASCADE,
    name        TEXT NOT NULL,
    description TEXT NOT NULL DEFAULT '',
    status      TEXT NOT NULL DEFAULT 'todo'
                CHECK (status IN ('todo', 'in_progress', 'blocked', 'done')),
    claimed_by  TEXT,
    summary     TEXT,
    UNIQUE (stage_id, name)
);

CREATE TABLE tasks (
    id         TEXT PRIMARY KEY,
    module_id  TEXT NOT NULL REFERENCES modules (id) ON DELETE CASCADE,
    name       TEXT NOT NULL,
    status     TEXT NOT NULL DEFAULT 'open'
               CHECK (status IN ('open', 'done', 'skipped')),
    origin     TEXT NOT NULL DEFAULT 'planned'
               CHECK (origin IN ('planned', 'discovered')),
    note       TEXT,
    position   INT NOT NULL DEFAULT 0,
    created_by TEXT,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

-- tsv covers content only: array_to_string is STABLE, not IMMUTABLE, so
-- tags can't join the generated column; tag filtering uses `tags && $n`.
CREATE TABLE memories (
    id         TEXT PRIMARY KEY,
    level      TEXT NOT NULL CHECK (level IN ('task', 'module', 'stage', 'feature')),
    subject_id TEXT NOT NULL,
    content    TEXT NOT NULL,
    tags       TEXT[] NOT NULL DEFAULT '{}',
    author     TEXT NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    tsv        tsvector GENERATED ALWAYS AS (to_tsvector('english', content)) STORED
);
CREATE INDEX memories_tsv_idx ON memories USING GIN (tsv);
CREATE INDEX memories_subject_idx ON memories (level, subject_id);

CREATE TABLE events (
    seq        BIGSERIAL PRIMARY KEY,
    ts         TIMESTAMPTZ NOT NULL DEFAULT now(),
    type       TEXT NOT NULL,
    feature_id TEXT,
    subject_id TEXT,
    agent      TEXT,
    payload    JSONB NOT NULL DEFAULT '{}'
);
CREATE INDEX events_feature_idx ON events (feature_id, seq);
