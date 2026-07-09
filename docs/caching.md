# Caching with duckdb-opendal (`opendal_fs`)

Reads through `opendal_fs` can be cached three complementary ways. They are not
mutually exclusive, but you generally pick one to avoid double-caching. All work
without any change to your queries.

## 1. DuckDB's built-in external file cache (automatic)

DuckDB ships a block cache (`external file cache`) that transparently caches
byte ranges for **any** filesystem that reports itself as seekable — including
`opendal_fs`. It is **on by default** and requires no setup.

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

- **Where it lives:** DuckDB's **buffer pool** (RAM), bounded by DuckDB's
  configured memory limit. It is **not** an on-disk cache: cached blocks are
  allocated with `can_destroy = true`, so when memory is reclaimed they are
  **discarded** (re-fetched from source on next access), not written to a temp
  file. For a *persistent* on-disk cache use option 2 (foyer disk tier) or
  option 3 (`cache_httpfs`).
- **Validation:** DuckDB re-checks size/mtime and invalidates on change.
- **Disable:** `SET enable_external_file_cache = false;`

This is the recommended default for most workloads — nothing to configure.

## 2. OpenDAL foyer layer (internal, opt-in per secret)

`opendal_fs` can attach OpenDAL's [foyer](https://github.com/foyer-rs/foyer)
cache **inside** the operator, configured per secret via the `layers` map. This
caches at the OpenDAL layer (below DuckDB), uniformly across every service, and
supports both an in-memory tier and a **persistent on-disk tier**.

```sql
-- In-memory only:
CREATE SECRET s3_cached (
    TYPE s3, SCOPE 's3://bucket',
    key_id '...', secret '...', region 'us-east-1',
    layers MAP{
        'foyer.enable'    : 'true',
        'foyer.memory_mb' : '256'         -- in-memory tier (default 256)
    }
);

-- In-memory + on-disk (persistent across sessions):
CREATE SECRET s3_cached_disk (
    TYPE s3, SCOPE 's3://bucket',
    key_id '...', secret '...', region 'us-east-1',
    layers MAP{
        'foyer.enable'    : 'true',
        'foyer.memory_mb' : '256',
        'foyer.disk_path' : '/var/cache/opendalfs',  -- enables the on-disk tier
        'foyer.disk_mb'   : '4096',                   -- on-disk capacity (default 1024)
        'foyer.block_mb'  : '4'                        -- on-disk block size (default 4)
    }
);

-- Reads under s3://bucket now go through the foyer cache.
SELECT * FROM read_parquet('s3://bucket/data.parquet');
```

- **Tiers:** in-memory always; the on-disk tier is enabled by setting
  `foyer.disk_path` (a directory; created if missing). The disk tier is
  persistent and evicts by capacity.
- **Where it lives:** attached to the operator built for that secret's
  `scheme://authority`, so it is scoped to the buckets that secret matches.
- **Only for secret-backed schemes** (e.g. `s3://`). `fs://`/`memory://` don't
  use secrets, so they don't carry a foyer layer.
- Best used *instead of* the DuckDB external cache to avoid double-caching; if
  you enable foyer, consider `SET enable_external_file_cache = false;`.
- Best-effort: if the cache fails to build (e.g. an unwritable disk path), the
  operator logs a warning and continues **without** caching rather than failing.

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
  or use the `opendal_override_native_filesystems` setting to make `opendal_fs`
  win `s3://` (see [schemes.md](schemes.md)) and then rely on option 1/2.

## Which one?

| | DuckDB external cache | foyer layer | cache_httpfs |
|---|---|---|---|
| Setup | none (default on) | per-secret `layers` | `LOAD` + wrap |
| Persistence | memory (buffer pool) | memory **and/or on-disk** | on-disk (default) |
| Applies to | all seekable FS | secret-backed schemes | any FS (by name) |
| Best for | general default | uniform per-service internal cache | large, repeated remote reads |

Start with option 1 (nothing to do). Reach for foyer when you want an internal,
per-service cache that ships with the extension — including a persistent on-disk
tier via `foyer.disk_path`. Reach for `cache_httpfs` when you want an external,
profiled on-disk cache — keeping the `s3://` scheme caveat (below) in mind.

