# Caching

Repeated reads can be cached three ways. They stack, but you normally pick one,
since caching the same bytes twice just wastes memory. None of them require
changing your queries.

|                  | DuckDB external cache            | OpenDAL data cache              | `cache_httpfs`                        |
| ---------------- | -------------------------------- | ------------------------------- | ------------------------------------- |
| Setup            | none, on by default              | `cache_config`                  | `LOAD` + wrap                         |
| Lives in         | RAM (buffer pool)                | RAM and/or disk                 | disk (default)                        |
| Survives restart | no                               | with `disk_path`                | yes                                   |
| Applies to       | `s3`, `gcs`, `http(s)` URLs      | all `opendal` schemes           | any filesystem, by name               |
| Best for         | most workloads on those schemes  | Azure and `fs`, or a disk cache | large, repeatedly scanned remote data |

Start with the first if your data is on S3, GCS or HTTP. Use the second for the
Azure services and `fs`, which DuckDB does not cache, or when you want a
persistent on-disk cache without another extension. Reach for `cache_httpfs`
when you need its extra features (metadata caching, profiling).

## 1. DuckDB's external file cache (default)

DuckDB caches byte ranges of remote files in its buffer pool. It is on by
default, and decides what is remote from the URL alone: the
[claimed schemes](coexistence.md#claimed-schemes) of `httpfs` and `azure`
qualify, except `abfs://`. The `azblob`, `azdls`, `azfile`, `minio` and `fs`
schemes do not, so their reads are not cached unless you
`SET cache_local_files = true`, which also caches every local file.

```sql
SELECT current_setting('enable_external_file_cache');   -- true

SELECT * FROM read_parquet('s3://warehouse/data.parquet');

-- What's cached right now:
SELECT path, count(*) AS ranges, sum(nr_bytes) AS bytes
FROM duckdb_external_file_cache()
GROUP BY path;
```

- Cached blocks live in DuckDB's buffer pool, bounded by its memory limit. Under
  memory pressure they are dropped and re-fetched. Nothing is written to disk.
- DuckDB re-checks size and mtime, and validates cached ranges against the
  object's ETag when the service provides one.
- Turn it off with `SET enable_external_file_cache = false;`.

## 2. The OpenDAL data cache

The extension can cache object data itself, in memory and optionally on disk,
for every scheme. Configure it globally or per secret with `cache_config`.

```sql
-- Everything, in memory:
SET GLOBAL opendal_cache_config = MAP{'memory_size': '256 MiB'};

-- One container, with a persistent disk tier:
CREATE SECRET azblob_cached (
    TYPE azblob, SCOPE 'azblob://warehouse',
    config MAP{'account_name': '...', 'account_key': '...'},
    cache_config MAP{
        'memory_size': '256 MiB',
        'disk_path': '/var/cache/duckdb-opendal',
        'disk_size': '8 GiB'
    }
);

SELECT * FROM read_parquet('azblob://warehouse/data.parquet');   -- served via the cache
```

- The cache stores fixed-size ranges of each object (`chunk_size`, 256 KiB by
  default), and fetches only the ranges a query reads. Reading a Parquet footer
  costs a few chunks, not the whole file. On a miss it fetches up to 4 MiB of
  adjacent chunks in one request.
- Memory only, until you set `disk_path`; that adds a persistent tier that
  evicts by capacity and is reused after a restart.
- Cached ranges are tied to the object's version, ETag or modification time, so
  a changed object is never served from stale chunks. That metadata is trusted
  for `ttl` (1 minute by default) before it is checked again; writes, renames
  and deletes through `opendal` take effect immediately.
- Each distinct service configuration (endpoint, bucket, credentials, options)
  gets its own cache, and its own subdirectory under `disk_path`, so identical
  paths in different places never collide.
- While a file is read through this cache, DuckDB's external file cache steps
  aside for it, so the bytes are not held twice.
- Caching is best-effort: if the cache cannot be set up (an unwritable
  `disk_path`, or one in use by another process, say) the query still runs,
  uncached, with a warning.
- Object data only. No metadata or listing results are cached.

### The disk tier

- `disk_path` is a local directory. A relative path is resolved against the
  working directory, and `~` against the home directory. When DuckDB restricts
  file access (`enable_external_access = false`), it must be inside
  `allowed_directories`, or the query fails.
- Cached data is stored unencrypted, in a directory readable only by its owner.
  Anyone with access to that account can read it, so do not point an encrypted
  or tightly scoped bucket at a shared or long-lived cache directory.
- One process uses a cache directory at a time. A second DuckDB process with the
  same configuration runs uncached, with a warning, until the first closes.

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
- `cache_httpfs` loads `httpfs`, so load it before `opendal`. The filesystems it
  wraps win over `opendal` on [shared schemes](coexistence.md#terms), even with
  `opendal_override_schemes`. See
  [which filesystem serves a URL](coexistence.md#which-filesystem-serves-a-url).
