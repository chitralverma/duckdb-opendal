-- Init script for the s3:// backend (loaded by test/configs/s3.json).
-- Values come from the config JSON's test_env (pre-seeded into the runner's
-- substitution map before on_init runs), so this file is pure SQL. For the
-- MinIO emulator the values are non-secret literals set in s3.json.
CREATE SECRET s3_common (
    TYPE s3, SCOPE 's3://${OPENDAL_S3_BUCKET}',
    config MAP{
        'access_key_id': '${OPENDAL_S3_ACCESS_KEY_ID}',
        'secret_access_key': '${OPENDAL_S3_SECRET_ACCESS_KEY}',
        'region': '${OPENDAL_S3_REGION}',
        'endpoint': '${OPENDAL_S3_ENDPOINT}',
        'enable_virtual_host_style': 'false'
    }
);
