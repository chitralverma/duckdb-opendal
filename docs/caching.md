# Caching

Repeated reads can be cached three ways. They stack, but you normally pick one —
caching the same bytes twice just wastes memory. None of them require changing
your queries.

|                  | DuckDB external cache   | OpenDAL data cache             | `cache_httpfs`                        |
| ---------------- | ----------------------- | ------------------------------ | ------------------------------------- |
| Setup            | none, on by default     | `cache_config`                 | `LOAD` + wrap                         |
| Lives in         | RAM (buffer pool)       | RAM and/or disk                | disk (default)                        |
| Survives restart | no                      | with `disk_path`               | yes                                   |
| Applies to       | any seekable filesystem | all `opendal` schemes          | any filesystem, by name               |
| Best for         | most workloads          | a persistent cache per service | large, repeatedly scanned remote data |

Start with the first — there is nothing to do. Move to the second when you want
a persistent on-disk cache without another extension. Reach for `cache_httpfs`
when you need its extra features (metadata caching, profiling).

## 1. DuckDB's external file cache (default)

DuckDB caches byte ranges for any filesystem that reports itself seekable,
including `opendal`. It is on by default.

```sql
SELECT current_setting('enable_external_file_cache');   -- true

SELECT * FROM read_parquet('s3://warehouse/data.parquet');

-- What's cached right now:
SELECT path, count(*) AS ranges, sum(nr_bytes) AS bytes
FROM duckdb_external_file_cache()
GROUP BY path;
```

- Cached blocks live in DuckDB's buffer pool, bounded by its memory limit. Under
  memory pressure they are dropped and re-fetched — nothing is written to disk.
- DuckDB re-checks size and mtime, so stale data is not served.
- Turn it off with `SET enable_external_file_cache = false;`.

## 2. The OpenDAL data cache

The extension can cache object data itself, in memory and optionally on disk.
Configure it globally or per secret with `cache_config`.

```sql
-- Everything, in memory:
SET GLOBAL opendal_cache_config = MAP{'memory_size': '256 MiB', 'shards': '4'};

-- One bucket, with a persistent disk tier:
CREATE SECRET s3_cached (
    TYPE s3, SCOPE 's3://warehouse',
    config MAP{'access_key_id': '...', 'secret_access_key': '...', 'region': 'us-east-1'},
    cache_config MAP{
        'memory_size': '256 MiB',
        'disk_path': '/var/cache/opendal',
        'disk_size': '4 GiB',
        'block_size': '4 MiB'
    }
);

SELECT * FROM read_parquet('s3://warehouse/data.parquet');   -- served via the cache
```

- Memory only, until you set `disk_path`; that adds a persistent tier that
  evicts by capacity.
- Each service/bucket gets its own cache namespace, so identical paths in
  different places never collide.
- Consider `SET enable_external_file_cache = false;` so the same bytes are not
  also held in DuckDB's buffer pool.
- Caching is best-effort: if the cache cannot be set up (an unwritable
  `disk_path`, say) the query still runs, uncached, with a warning.
- Object data only — no metadata or listing results are cached.

All keys are listed in [configuration](configuration.md#cache_config).

## 3. The `cache_httpfs` extension

[`cache_httpfs`](https://github.com/dentiny/duck-read-cache-fs) wraps any DuckDB
filesystem by name, including this one, and adds a richer on-disk cache with
metadata caching and profiling.

```sql
LOAD cache_httpfs;   -- INSTALL cache_httpfs FROM community; first, if needed
SELECT cache_httpfs_wrap_cache_filesystem('OpenDalFileSystem');

SELECT * FROM read_parquet('fs:///data/big.parquet');   -- now cached by cache_httpfs
```

- On-disk and persistent by default, with LRU/deadline eviction; in-memory and
  metadata caching are also available.
- It disables DuckDB's external file cache by default; re-enable with
  `SET enable_external_file_cache = true;` if you want both.
- **`s3://` caveat:** loading `cache_httpfs` pulls in DuckDB's native `httpfs`,
  which also claims `s3://` and may take precedence. Either wrap `opendal` for
  schemes `httpfs` does not own (`fs://`, the Azure schemes, …), or force
  `opendal` to win with `SET opendal_override_native_filesystems = 's3';` and use
  option 1 or 2 instead. See [coexistence](coexistence.md).
