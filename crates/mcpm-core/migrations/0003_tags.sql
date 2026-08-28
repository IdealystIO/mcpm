-- Tags become first-class rows.
--
-- Wants still carry their own `tags TEXT[]` (the tag list of a want is
-- part of that want, and array containment is what the pool's filters
-- search on). This table is the REGISTRY: it lets a tag exist before any
-- want uses it — a preset someone set up to organize with — and gives
-- the console's autocomplete a stable, ordered vocabulary instead of
-- whatever happens to be in use right now.
--
-- `name` is the normalized slug (lowercase, dashes); `label` is how it
-- was first typed, so the UI can show `Field Reports` while the wants
-- carry `field-reports`.

CREATE TABLE tags (
    name       TEXT PRIMARY KEY,
    label      TEXT NOT NULL,
    created_by TEXT NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

-- Backfill every tag already in use, so the registry starts complete.
INSERT INTO tags (name, label, created_by)
SELECT DISTINCT lower(t), t, 'backfill'
FROM wants w, unnest(w.tags) AS t
WHERE btrim(t) <> ''
ON CONFLICT (name) DO NOTHING;
