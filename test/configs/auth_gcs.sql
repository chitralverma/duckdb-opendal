-- Init script for the gcs:// service (loaded by test/configs/gcs.json).
-- Values come from the config JSON's test_env, so this file is pure SQL. The
-- fake-gcs-server emulator is unauthenticated: skip request signing and
-- real-GCS credential / VM-metadata discovery.
CREATE SECRET gcs_common (
    TYPE gcs, SCOPE 'gcs://{OPENDAL_GCS_BUCKET}',
    config MAP{
        'endpoint': '{OPENDAL_GCS_ENDPOINT}',
        'skip_signature': 'true',
        'disable_config_load': 'true',
        'disable_vm_metadata': 'true'
    }
);

-- test/sql/common/cache.test reads one object through the data cache and the
-- counting proxy.
CREATE SECRET gcs_cached (
    TYPE gcs, SCOPE '{OPENDAL_CACHED_FILE}',
    config MAP{
        'endpoint': '{OPENDAL_PROXY}',
        'skip_signature': 'true',
        'disable_config_load': 'true',
        'disable_vm_metadata': 'true'
    },
    cache_config MAP{'memory_size': '64 MiB'}
);
