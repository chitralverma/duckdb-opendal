-- Init script for the gcs:// service (loaded by test/configs/gcs.json).
-- Values come from the config JSON's test_env, so this file is pure SQL. The
-- fake-gcs-server emulator is unauthenticated: allow anonymous access and skip
-- real-GCS credential / VM-metadata discovery.
CREATE SECRET gcs_common (
    TYPE gcs, SCOPE 'gcs://${OPENDAL_GCS_BUCKET}',
    config MAP{
        'endpoint': '${OPENDAL_GCS_ENDPOINT}',
        'allow_anonymous': 'true',
        'disable_config_load': 'true',
        'disable_vm_metadata': 'true'
    }
);
