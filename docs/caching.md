# Caching with duckdb-opendal (`opendal_fs`)

Reads through `opendal_fs` can be cached three complementary ways. They are not
mutually exclusive, but you generally pick one to avoid double-caching. All work
without any change to your queries.

## 1. DuckDB's built-in external file cache (automatic)

DuckDB ships an in-memory block cache (`external file cache`) that transparently
caches byte ranges for **any** filesystem that reports itself as seekable —
including `opendal_fs`. It is **on by default** and requires no setup.

```sql
-- It's already enabled:
SELECT current_setting('enable_external_file_cache');   -- true

-- Read something remote through opendal_fs; ranges get cached automatically.
SELECT * FROM read_parquet('s3://bucket/data.parquet');

-- Inspect what's cached:
SELECT path, count(*) AS ranges, sum(nr_bytes) AS bytes
FROM duckdb_external_file_cache()
GROUP BY path;
```

- **Scope:** in-memory only; evicted under memory pressure (LRU).
- **Validation:** DuckDB re-checks size/mtime and invalidates on change.
- **Disable:** `SET enable_external_file_cache = false;`

This is the recommended default for most workloads — nothing to configure.

## 2. OpenDAL foyer layer (internal, opt-in per secret)

`opendal_fs` can attach OpenDAL's [foyer](https://github.com/foyer-rs/foyer)
cache **inside** the operator, configured per secret via the `layers` map. This
caches at the OpenDAL layer (below DuckDB), uniformly across every service.

```sql
CREATE SECRET s3_cached (
    TYPE s3, SCOPE 's3://bucket',
    key_id '...', secret '...',
    region 'us-east-1',
    layers MAP{
        'foyer.enable'    : 'true',
        'foyer.memory_mb' : '256'     -- in-memory cache size (default 256)
    }
);

-- Reads under s3://bucket now go through the foyer cache.
SELECT * FROM read_parquet('s3://bucket/data.parquet');
```

- **Scope:** in-memory (a memory tier). On-disk tiering is planned; the
  `foyer.disk_*` keys are reserved for it.
- **Where it lives:** attached to the operator built for that secret's
  `scheme://authority`, so it is scoped to the buckets that secret matches.
- **Only for secret-backed schemes** (e.g. `s3://`). `fs://`/`memory://` don't
  use secrets, so they don't carry a foyer layer.
- Best used *instead of* the DuckDB external cache to avoid double-caching; if
  you enable foyer, consider `SET enable_external_file_cache = false;`.

## 3. `cache_httpfs` community extension (external, wraps opendal_fs)

The [`cache_httpfs`](https://github.com/dentiny/duck-read-cache-fs) community
extension (a.k.a. `duck-read-cache-fs`) provides a rich on-disk/in-memory read
cache and can wrap **any** DuckDB filesystem by name — including
`OpenDalFileSystem`.

```sql
LOAD cache_httpfs;              -- or: INSTALL cache_httpfs FROM community; LOAD cache_httpfs;
SELECT cache_httpfs_wrap_cache_filesystem('OpenDalFileSystem');

-- Reads through opendal_fs are now cached by cache_httpfs.
SELECT * FROM read_parquet('fs:///data/big.parquet');
```

- **Scope:** on-disk by default (persistent across sessions), LRU/deadline
  eviction; also supports in-memory and metadata/glob caching.
- `cache_httpfs` disables DuckDB's external file cache by default (to avoid
  double-caching); re-enable with `SET enable_external_file_cache = true;`.
- **Caveat on `s3://`:** loading `cache_httpfs` pulls in DuckDB's native
  `httpfs`, which also claims the `s3://` scheme and may take precedence over
  `opendal_fs` for `s3://` URLs. Use `cache_httpfs` wrapping with `opendal_fs`
  for schemes `httpfs` does not own (e.g. `fs://`, or other OpenDAL services),
  or rely on option 1/2 for `s3://`.

## Which one?

| | DuckDB external cache | foyer layer | cache_httpfs |
|---|---|---|---|
| Setup | none (default on) | per-secret `layers` | `LOAD` + wrap |
| Persistence | in-memory | in-memory | on-disk (default) |
| Applies to | all seekable FS | secret-backed schemes | any FS (by name) |
| Best for | general default | uniform per-service internal cache | large, repeated remote reads |

Start with option 1 (nothing to do). Reach for foyer when you want an internal,
per-service cache that ships with the extension. Reach for `cache_httpfs` when
you want a persistent on-disk cache with profiling — keeping the `s3://` scheme
caveat in mind.
