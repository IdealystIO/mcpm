-- Attachments: files pinned to a feature or a want, with a description
-- written for the agent that will read them.
--
-- The BYTES live in an object store (S3, or MinIO in the devcontainer)
-- under `object_key`; this table is the record of what was attached,
-- to what, by whom, and — the part an agent actually needs — what the
-- file is for. A briefing lists these descriptions, and a worker then
-- decides whether to fetch the file itself.
--
-- A feature's files are its own rows PLUS the rows of the wants it was
-- composed from, joined at read time through `want_features`: nothing
-- is copied when wants are promoted, so a file attached to an idea
-- reaches every plan the idea informs and disappears from none of them.
--
-- No foreign key: `subject_id` is polymorphic (feat_ or want_), and the
-- store removes the rows itself when a feature or a want is deleted —
-- alongside deleting the objects, which a cascade could not do.
CREATE TABLE attachments (
    id           TEXT PRIMARY KEY,
    level        TEXT NOT NULL CHECK (level IN ('feature', 'want')),
    subject_id   TEXT NOT NULL,
    -- The file name as attached. Not a path; a display name and the
    -- last segment of the object key.
    name         TEXT NOT NULL,
    description  TEXT NOT NULL DEFAULT '',
    content_type TEXT NOT NULL DEFAULT 'application/octet-stream',
    size_bytes   BIGINT NOT NULL CHECK (size_bytes >= 0),
    -- Where the bytes live in the bucket. Unique because two rows
    -- pointing at one object would make removal of either delete the
    -- other's file.
    object_key   TEXT NOT NULL UNIQUE,
    sha256       TEXT NOT NULL,
    added_by     TEXT NOT NULL,
    created_at   TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at   TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX attachments_by_subject ON attachments (level, subject_id, created_at);
