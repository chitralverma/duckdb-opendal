-- Init script for the azfile:// service. Only referenced by the rendered config
-- (test/services/azfile/config.template.json -> build/test-configs/azfile.json),
-- because the values are real Azure credentials that cannot be checked in; the
-- tracked test/configs/azfile.json stub sets no init_script so the suite skips
-- when credentials are absent. See test/services/render_config.py.
--
-- azfile derives `endpoint` (https://<authority>) and `account_name`
-- (<authority> up to the first dot) from the URL authority, so the secret only
-- has to carry the share name and the credential.
CREATE SECRET azfile_common (
    TYPE azfile, SCOPE 'azfile://${OPENDAL_AZFILE_HOST}',
    config MAP{
        'share_name': '${OPENDAL_AZFILE_SHARE_NAME}',
        'account_key': '${OPENDAL_AZFILE_ACCOUNT_KEY}'
    }
);
