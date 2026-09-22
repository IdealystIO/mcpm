-- The roadmap: long-horizon INTENT, sitting above the work tree.
--
-- A feature says what is being built. A roadmap item says what the
-- product is becoming, in one paragraph, and NOTHING about how — that
-- is the whole point. An agent planning today's module reads the items
-- around it and learns what not to paint into a corner, without paying
-- for the details of a dozen features it will never touch.
--
-- Three shapes, and each is deliberate:
--
-- * **A DAG, not a ladder.** `roadmap_deps` is edges for the same
--   reason `module_deps` is (migration 0012 retired stages): a roadmap
--   that only expresses "phase 2 after phase 1" makes every planner
--   that needs a different shape hide it in prose. Horizon is a free
--   LABEL with no semantics — it groups cards on a screen; the edges
--   are what ordering means.
--
-- * **Two doors, each gated on the level below.** A feature's work
--   ends at `complete_feature` (status 'done'); SHIPPING is a separate
--   act, `features.released_at`. An item ships when every feature
--   bound to it has been released and every item it depends on has
--   shipped. So "released" keeps meaning a deliberate ship rather than
--   a side effect of the last module going green, and the roadmap can
--   tell the truth about what is live versus what is merely built.
--   A LOOSE feature — no item, which is the normal case for a sprint
--   or a bugfix — releases the moment it completes: nothing holds it,
--   and a second call for every one of those is a habit that would be
--   forgotten and would leave the board lying.
--
-- * **`shipped_at` is held, not derived.** Everything else about an
--   item's state is computed at read time (see `roadmap` in store.rs),
--   but an item can be satisfied by something outside the work tree —
--   an account migration, a contract, a vendor's release — and an item
--   with no features would otherwise hold its dependents shut forever.
--   So shipping is an ACT, refused unless the facts below it allow it,
--   exactly like `complete_feature` is.
CREATE TABLE roadmap_items (
    id         TEXT PRIMARY KEY,
    name       TEXT NOT NULL UNIQUE,
    -- One paragraph, in product terms: the outcome, not the plan.
    -- This is the text that travels into every agent's context.
    intent     TEXT NOT NULL DEFAULT '',
    -- Optional long form, Markdown. Read whole when an agent needs the
    -- detail; NOT carried in the digest. A plain column rather than a
    -- `documents` row because an item is not a tree node and giving it
    -- a Level would widen an enum four call sites deep for one string.
    vision     TEXT NOT NULL DEFAULT '',
    -- Display grouping only: 'now', 'next', 'H1', ''. No semantics —
    -- if it ordered anything it would be a second, silent dependency
    -- system disagreeing with the edges.
    horizon    TEXT NOT NULL DEFAULT '',
    position   INT NOT NULL DEFAULT 0,
    shelved    BOOLEAN NOT NULL DEFAULT FALSE,
    shipped_at TIMESTAMPTZ,
    shipped_by TEXT,
    created_by TEXT,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

-- `hard` is the opt-in that holds WORK, not just shipping.
--
-- The default (soft) is what makes pre-planning possible: plan the
-- feature, dispatch it, build it, complete every module — only the
-- ship door is shut. A hard edge is for a prerequisite that must
-- physically exist before anything downstream can be written at all,
-- and it makes `claim_module` refuse with PREREQS_OPEN, the same
-- refusal and the same worker reaction as an unmet module edge.
CREATE TABLE roadmap_deps (
    item_id    TEXT NOT NULL REFERENCES roadmap_items (id) ON DELETE CASCADE,
    depends_on TEXT NOT NULL REFERENCES roadmap_items (id) ON DELETE CASCADE,
    hard       BOOLEAN NOT NULL DEFAULT FALSE,
    PRIMARY KEY (item_id, depends_on),
    CHECK (item_id <> depends_on)
);
CREATE INDEX roadmap_deps_dep_idx ON roadmap_deps (depends_on);

-- NULL is legal and normal: a loose feature belongs to no item.
-- ON DELETE SET NULL because deleting an item is a roadmap decision,
-- and it must not take real work with it — the features loosen.
ALTER TABLE features ADD COLUMN roadmap_item_id TEXT
    REFERENCES roadmap_items (id) ON DELETE SET NULL;
CREATE INDEX features_roadmap_idx ON features (roadmap_item_id)
    WHERE roadmap_item_id IS NOT NULL;

-- Shipped is a FACT WITH A TIME, not a status value: `status` stays
-- the four it has always been, so every existing reader, view and
-- test keeps working and nothing has to learn a fifth word to answer
-- "is this feature finished".
ALTER TABLE features ADD COLUMN released_at TIMESTAMPTZ;
ALTER TABLE features ADD COLUMN released_by TEXT;
CREATE INDEX features_released_idx ON features (released_at);

-- Every feature that is already done predates the roadmap, is loose,
-- and shipped long ago. Backdating them to their last ledger mention
-- keeps "what is live" true on the first read after this migration
-- rather than showing the whole history as unreleased.
UPDATE features f
   SET released_at = COALESCE(
           (SELECT max(e.ts) FROM events e WHERE e.feature_id = f.id),
           f.created_at)
 WHERE f.status = 'done';
