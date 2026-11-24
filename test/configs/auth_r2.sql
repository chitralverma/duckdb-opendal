-- Init script for the r2:// service (loaded by test/configs/r2.json).

-- httpfs also claims r2://.
SET GLOBAL opendal_override_schemes = 'r2';

-- Real R2 would use `account_id`; this tier points `endpoint` at MinIO.
CREATE SECRET r2_common (
    TYPE r2, SCOPE 'r2://warehouse',
    config MAP{
        'endpoint': '{OPENDAL_R2_ENDPOINT}',
        'access_key_id': '{OPENDAL_R2_ACCESS_KEY_ID}',
        'secret_access_key': '{OPENDAL_R2_SECRET_ACCESS_KEY}'
    }
);

-- test/sql/common/cache.test reads one object through the data cache and the
-- counting proxy.
CREATE SECRET r2_cached (
    TYPE r2, SCOPE '{OPENDAL_CACHED_FILE}',
    config MAP{
        'endpoint': '{OPENDAL_PROXY}',
        'access_key_id': '{OPENDAL_R2_ACCESS_KEY_ID}',
        'secret_access_key': '{OPENDAL_R2_SECRET_ACCESS_KEY}'
    },
    cache_config MAP{'memory_size': '64 MiB'}
);
