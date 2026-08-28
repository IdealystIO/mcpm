-- Move the liveness signal onto the `mcpm_events` channel.
--
-- 0004 is left byte-identical on purpose: sqlx checksums every applied
-- migration, so rewriting it in place would make an existing database
-- refuse to start. The rename therefore lands as a replacement of the
-- function 0004 installed — the trigger itself is unchanged and keeps
-- pointing at `notify_event()`.
CREATE OR REPLACE FUNCTION notify_event() RETURNS trigger AS $$
BEGIN
    PERFORM pg_notify('mcpm_events', NEW.seq::text);
    RETURN NULL;
END;
$$ LANGUAGE plpgsql;
