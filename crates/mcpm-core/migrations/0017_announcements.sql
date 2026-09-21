-- Announcements: an agent says, in its own words, what it is doing.
--
-- "Failed e2e, fixing." "Waiting on a nine-minute wasm build." "Merging
-- development in before the last module." None of that is a task, a
-- question or a blocker, and a ledger that carries only those reads as
-- silence between one checked-off task and the next. An announcement
-- is a `type = 'announcement'` event and nothing else: NOT a column on
-- the agent or the module, because the latest one is derived from the
-- ledger at read time the way readiness is derived from the graph, so
-- the roster, the module card and the feature row cannot disagree
-- with the feed about what was last said.
--
-- Three partial indexes make "the latest announcement on X" one index
-- probe however long X's history gets; without them a module that has
-- never announced anything would scan every event it ever had.
CREATE INDEX events_announce_by_subject ON events (subject_id, seq DESC)
    WHERE type = 'announcement';
CREATE INDEX events_announce_by_feature ON events (feature_id, seq DESC)
    WHERE type = 'announcement';
CREATE INDEX events_announce_by_agent ON events (agent, seq DESC)
    WHERE type = 'announcement';
