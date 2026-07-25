-- Init script for the azblob:// service (loaded by test/configs/azblob.json).
-- Values come from the config JSON's test_env (pre-seeded into the runner's
-- substitution map before on_init runs), so this file is pure SQL. For the
-- Azurite emulator the account name/key are Azurite's public, well-known
-- development credentials — non-secret literals set in azblob.json.
--
-- The container comes from the URL authority (azblob://<container>/<path>);
-- everything else must come from the secret because the operator URI carries
-- no path or query.
CREATE SECRET azblob_common (
    TYPE azblob, SCOPE 'azblob://${OPENDAL_AZBLOB_CONTAINER}',
    config MAP{
        'endpoint': '${OPENDAL_AZBLOB_ENDPOINT}',
        'account_name': '${OPENDAL_AZBLOB_ACCOUNT_NAME}',
        'account_key': '${OPENDAL_AZBLOB_ACCOUNT_KEY}'
    }
);
