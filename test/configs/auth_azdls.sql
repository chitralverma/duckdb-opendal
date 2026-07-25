-- Init script for the azdls:// service. Only referenced by the rendered config
-- (test/services/azdls/config.template.json -> build/test-configs/azdls.json),
-- because the values are real Azure credentials that cannot be checked in; the
-- tracked test/configs/azdls.json stub sets no init_script so the suite skips
-- when credentials are absent. See test/services/render_config.py.
--
-- azdls derives `endpoint` (https://<authority>) and `account_name`
-- (<authority> up to the first dot) from the URL authority, so the secret only
-- has to carry the filesystem and the credential.
CREATE SECRET azdls_common (
    TYPE azdls, SCOPE 'azdls://${OPENDAL_AZDLS_HOST}',
    config MAP{
        'filesystem': '${OPENDAL_AZDLS_FILESYSTEM}',
        'account_key': '${OPENDAL_AZDLS_ACCOUNT_KEY}'
    }
);
