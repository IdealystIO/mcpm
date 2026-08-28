-- Every committed event announces itself on `ccpm_events`, carrying its
-- seq. The console's WebSocket subscription LISTENs on that channel and
-- nudges a refetch, so an agent's write shows up at once instead of at
-- the end of a poll window.
--
-- This is a TRIGGER rather than a line in `record_event` on purpose:
-- the guarantee is "every committed event notifies", and like every
-- other invariant in this project it belongs where it cannot be
-- bypassed — a future writer (another service, a psql session, a
-- backfill) gets it for free. NOTIFY is transactional: a rolled-back
-- claim sends nothing, while the deliberately-committed
-- `premature_claim` rejection still reaches the console.
CREATE OR REPLACE FUNCTION notify_event() RETURNS trigger AS $$
BEGIN
    PERFORM pg_notify('ccpm_events', NEW.seq::text);
    RETURN NULL;
END;
$$ LANGUAGE plpgsql;

CREATE TRIGGER events_notify
    AFTER INSERT ON events
    FOR EACH ROW
    EXECUTE FUNCTION notify_event();
