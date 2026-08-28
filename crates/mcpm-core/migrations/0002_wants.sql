-- The want pool: loose ideas, captured cheaply, composed into features
-- later. A want is NOT a small feature — several wants compose into one
-- feature, and one want can inform several features, so the relation is
-- many-to-many and lives in its own table.
--
-- Held state stays minimal, as everywhere else in this schema: `state`
-- holds only open/declined. "Promoted" is DERIVED at read time from the
-- existence of a want_features link, so the pool can never claim an idea
-- is planned when no feature actually holds it.

CREATE TABLE wants (
    id             TEXT PRIMARY KEY,
    body           TEXT NOT NULL,
    tags           TEXT[] NOT NULL DEFAULT '{}',
    state          TEXT NOT NULL DEFAULT 'open'
                   CHECK (state IN ('open', 'declined')),
    decline_reason TEXT,
    author         TEXT NOT NULL,
    created_at     TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at     TIMESTAMPTZ NOT NULL DEFAULT now(),
    tsv            tsvector GENERATED ALWAYS AS (to_tsvector('english', body)) STORED,
    -- A decline without a reason is a lost idea; the record must say why.
    CONSTRAINT decline_needs_reason
        CHECK (state <> 'declined' OR decline_reason IS NOT NULL)
);
CREATE INDEX wants_tsv_idx ON wants USING GIN (tsv);
CREATE INDEX wants_state_idx ON wants (state, created_at DESC);

-- The composition record. `rationale` is how THIS want was read into
-- THAT feature — the audit trail from loose idea to planned work.
CREATE TABLE want_features (
    want_id    TEXT NOT NULL REFERENCES wants (id) ON DELETE CASCADE,
    feature_id TEXT NOT NULL REFERENCES features (id) ON DELETE CASCADE,
    rationale  TEXT NOT NULL DEFAULT '',
    linked_by  TEXT,
    linked_at  TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (want_id, feature_id)
);
CREATE INDEX want_features_feature_idx ON want_features (feature_id, linked_at);
