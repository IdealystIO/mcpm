-- Discussion: comments on a feature, a want, or a module — the place
-- humans and agents talk about the work beside the work.
--
-- Three kinds of comment, one table. A `note` is what it says. A
-- `question` names who owes the answer (`assigned_to`; NULL means
-- anyone) and BLOCKS its subject until one lands: a module with an
-- open question — or whose feature has one — is not dispatchable and
-- cannot be claimed, and a want with one cannot be promoted. An
-- `answer` points at its question through `answers`, and posting one
-- is what resolves the question (`resolved_at`/`resolved_by` are set
-- on the QUESTION row, so "is this open" is one column, not a join).
--
-- Nothing about readiness is stored elsewhere: the gate reads open
-- questions at claim time, the tree derives `dispatchable` from them
-- at read time. A `blocked` module status still exists — a worker
-- that reports a blocker is asking a question, and the status is the
-- worker-facing flag the answer clears.
CREATE TABLE comments (
    id          TEXT PRIMARY KEY,
    level       TEXT NOT NULL CHECK (level IN ('feature', 'want', 'module')),
    subject_id  TEXT NOT NULL,
    kind        TEXT NOT NULL DEFAULT 'note' CHECK (kind IN ('note', 'question', 'answer')),
    body        TEXT NOT NULL,
    author      TEXT NOT NULL,
    -- question: the agent (or person) who owes the answer.
    assigned_to TEXT,
    -- answer: the question it resolves.
    answers     TEXT REFERENCES comments (id) ON DELETE CASCADE,
    -- question: set when its answer lands.
    resolved_at TIMESTAMPTZ,
    resolved_by TEXT,
    created_at  TIMESTAMPTZ NOT NULL DEFAULT now(),
    edited_at   TIMESTAMPTZ,
    CHECK ((kind = 'answer') = (answers IS NOT NULL)),
    CHECK (kind = 'question' OR (assigned_to IS NULL AND resolved_at IS NULL))
);
CREATE INDEX comments_by_subject ON comments (level, subject_id, created_at);
-- The gate's question: "is anything open here?" — and the agent's:
-- "what awaits me?". Both partial, both small however long the
-- discussion gets.
CREATE INDEX comments_open_questions ON comments (level, subject_id)
    WHERE kind = 'question' AND resolved_at IS NULL;
CREATE INDEX comments_awaiting ON comments (assigned_to)
    WHERE kind = 'question' AND resolved_at IS NULL;

-- A file may now ride a module too, and may belong to a comment. A
-- comment's files are the SUBJECT's files (they list on the feature's
-- Files tab like any other) that happen to have arrived with a
-- comment; the cascade is what keeps a deleted comment from leaving
-- rows behind, and the store collects the object keys first.
ALTER TABLE attachments DROP CONSTRAINT attachments_level_check;
ALTER TABLE attachments ADD CONSTRAINT attachments_level_check
    CHECK (level IN ('feature', 'want', 'module'));
ALTER TABLE attachments ADD COLUMN comment_id TEXT REFERENCES comments (id) ON DELETE CASCADE;
CREATE INDEX attachments_by_comment ON attachments (comment_id) WHERE comment_id IS NOT NULL;
