-- The console stops loading the whole project at once: it reads a
-- light board, then one feature, one module, one page of events at a
-- time. Two things in the schema support that.

-- 1. A module's history is read by subject, and "has this module ever
--    had a claim rejected" is asked per module across the board. Both
--    were sequential scans of the ledger.
CREATE INDEX events_subject_idx ON events (subject_id, seq);

-- 2. The notification names what landed, so a listener can decide what
--    to refetch without a round trip: an event on another feature
--    refreshes the board's counts, not the open feature's tree.
--    `EventNotice::parse` still accepts the bare integer this used to
--    send, so a listener built before this migration keeps working.
CREATE OR REPLACE FUNCTION notify_event() RETURNS trigger AS $$
BEGIN
    PERFORM pg_notify('mcpm_events', json_build_object(
        'seq',        NEW.seq,
        'type',       NEW.type,
        'feature_id', NEW.feature_id,
        'subject_id', NEW.subject_id
    )::text);
    RETURN NULL;
END;
$$ LANGUAGE plpgsql;
