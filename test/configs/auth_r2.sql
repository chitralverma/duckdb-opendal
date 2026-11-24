-- Init script for the r2:// service (loaded by test/configs/r2.json).
--
-- r2:// is also claimed by DuckDB's native httpfs, so claim it explicitly
-- instead of depending on extension load order.
SET GLOBAL opendal_override_schemes = 'r2';

-- Real R2 derives the endpoint from `account_id`; this tier points `endpoint`
-- at MinIO instead (the preset allows exactly one of the two).
CREATE SECRET r2_common (
    TYPE r2, SCOPE 'r2://warehouse',
    config MAP{
        'endpoint': '${OPENDAL_R2_ENDPOINT}',
        'access_key_id': '${OPENDAL_R2_ACCESS_KEY_ID}',
        'secret_access_key': '${OPENDAL_R2_SECRET_ACCESS_KEY}'
    }
);

-- test/sql/common/cache.test reads one object through the data cache and the
-- counting proxy.
CREATE SECRET r2_cached (
    TYPE r2, SCOPE '${OPENDAL_CACHED_FILE}',
    config MAP{
        'endpoint': '${OPENDAL_PROXY}',
        'access_key_id': '${OPENDAL_R2_ACCESS_KEY_ID}',
        'secret_access_key': '${OPENDAL_R2_SECRET_ACCESS_KEY}'
    },
    cache_config MAP{'memory_size': '64 MiB'}
);
