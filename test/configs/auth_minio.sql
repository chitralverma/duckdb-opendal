-- Init script for the minio:// service (loaded by test/configs/minio.json).
CREATE SECRET minio_common (
    TYPE minio, SCOPE 'minio://warehouse',
    config MAP{
        'endpoint': '{OPENDAL_MINIO_ENDPOINT}',
        'access_key_id': '{OPENDAL_MINIO_ACCESS_KEY_ID}',
        'secret_access_key': '{OPENDAL_MINIO_SECRET_ACCESS_KEY}'
    }
);

-- test/sql/common/cache.test reads one object through the data cache and the
-- counting proxy.
CREATE SECRET minio_cached (
    TYPE minio, SCOPE '{OPENDAL_CACHED_FILE}',
    config MAP{
        'endpoint': '{OPENDAL_PROXY}',
        'access_key_id': '{OPENDAL_MINIO_ACCESS_KEY_ID}',
        'secret_access_key': '{OPENDAL_MINIO_SECRET_ACCESS_KEY}'
    },
    cache_config MAP{'memory_size': '64 MiB'}
);
