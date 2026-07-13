# Caching with duckdb-opendal (`opendal`)

Reads through `opendal` can be cached three complementary ways. They are not
mutually exclusive, but you generally pick one to avoid double-caching. All work
without any change to your queries.

## 1. DuckDB's built-in external file cache (automatic)

DuckDB ships a block cache (`external file cache`) that transparently caches
byte ranges for **any** filesystem that reports itself as seekable — including
`opendal`. It is **on by default** and requires no setup.

```sql
-- It's already enabled:
SELECT current_setting('enable_external_file_cache');   -- true

-- Read something remote through opendal; ranges get cached automatically.
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

## 2. OpenDAL data cache (internal, global or scoped)

`opendal` can attach a data cache inside each operator. It works for every
service and supports memory plus an optional persistent disk tier. Configure it
globally or per secret; see [configuration.md](configuration.md) for all keys.

```sql
-- Global in-memory cache for every OpenDAL service/path:
SET GLOBAL opendal_cache_config = MAP{'memory_size':'256 MiB','shards':'4'};

-- Scoped in-memory + on-disk cache:
CREATE SECRET s3_cached_disk (
    TYPE s3, SCOPE 's3://bucket',
    config MAP{'access_key_id':'...','secret_access_key':'...','region':'us-east-1'},
    cache_config MAP{'memory_size':'256 MiB','disk_path':'/var/cache/opendal','disk_size':'4 GiB','block_size':'4 MiB'}
);

-- Reads under s3://bucket now go through the foyer cache.
SELECT * FROM read_parquet('s3://bucket/data.parquet');
```

- **Tiers:** in-memory always; `disk_path` adds a persistent on-disk tier. It is
  persistent and evicts by capacity.
- **Scope:** global defaults apply to every service/path; secret cache options
  merge over them for matching scopes. Effective operators use isolated cache
  namespaces so identical paths in different services/buckets cannot collide.
- Best used *instead of* the DuckDB external cache to avoid double-caching; if
  you enable foyer, consider `SET enable_external_file_cache = false;`.
- Best-effort: if the cache fails to build (e.g. an unwritable disk path), the
  operator logs a warning and continues **without** caching rather than failing.

## 3. `cache_httpfs` community extension (external, wraps opendal)

The [`cache_httpfs`](https://github.com/dentiny/duck-read-cache-fs) community
extension (a.k.a. `duck-read-cache-fs`) provides a rich on-disk/in-memory read
cache and can wrap **any** DuckDB filesystem by name — including
`OpenDalFileSystem`.

```sql
LOAD cache_httpfs;              -- or: INSTALL cache_httpfs FROM community; LOAD cache_httpfs;
SELECT cache_httpfs_wrap_cache_filesystem('OpenDalFileSystem');

-- Reads through opendal are now cached by cache_httpfs.
SELECT * FROM read_parquet('fs:///data/big.parquet');
```

- **Scope:** on-disk by default (persistent across sessions), LRU/deadline
  eviction; also supports in-memory and metadata/glob caching.
- `cache_httpfs` disables DuckDB's external file cache by default (to avoid
  double-caching); re-enable with `SET enable_external_file_cache = true;`.
- **Caveat on `s3://`:** loading `cache_httpfs` pulls in DuckDB's native
  `httpfs`, which also claims the `s3://` scheme and may take precedence over
  `opendal` for `s3://` URLs. Use `cache_httpfs` wrapping with `opendal`
  for schemes `httpfs` does not own (e.g. `fs://`, or other OpenDAL services),
  or use the `opendal_override_native_filesystems` setting to make `opendal`
  win `s3://` (see [schemes.md](schemes.md)) and then rely on option 1/2.

## Which one?

| | DuckDB external cache | foyer layer | cache_httpfs |
|---|---|---|---|
| Setup | none (default on) | global or scoped `cache_config` | `LOAD` + wrap |
| Persistence | memory (buffer pool) | memory **and/or on-disk** | on-disk (default) |
| Applies to | all seekable FS | all OpenDAL schemes | any FS (by name) |
| Best for | general default | uniform per-service internal cache | large, repeated remote reads |

Start with option 1 (nothing to do). Reach for foyer when you want an internal,
per-service cache that ships with the extension — including a persistent on-disk
tier via `disk_path`. Reach for `cache_httpfs` when you want an external,
profiled on-disk cache — keeping the `s3://` scheme caveat (below) in mind.
