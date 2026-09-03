-- The pairing rule, restated. `0010_delegations.sql` says `key_id` is
-- "the key that minted it", and that stopped being the whole truth when
-- `mint_worker` grew `for_key_id`: the column is the key the token is
-- honoured ALONGSIDE, which defaults to the minter's and can be another
-- box's. An applied migration is checksummed and must not be edited, so
-- the correction goes where a correction to schema prose belongs — in
-- the schema, on the column, where the next person to read the table
-- finds it without having to know this file exists.
--
-- What did NOT change is the part that matters: exactly ONE key
-- honours a token. That is what makes carrying it in a prompt safe,
-- and any future "any key" resolution gives it up.
COMMENT ON COLUMN delegations.key_id IS
    'The one credential this token is honoured alongside — the minting key by default, '
    'or another agent key when the manager passed for_key_id (a worker on its own box). '
    'NULL means an unauthenticated stdio session, and resolution matches with '
    'IS NOT DISTINCT FROM so a keyed token and a keyless one never satisfy each other. '
    'Exactly one key, always: a token presented by any other is refused.';
