# OpenDAL configuration

`opendal` separates service configuration from cross-service I/O and layers.
All option values are strings because OpenDAL parses them per backend/layer.

## Scoped configuration

Use a service-specific DuckDB secret. `SCOPE` is optional; when present, the
secret applies only to matching URL prefixes.

```sql
CREATE SECRET warehouse (
    TYPE s3,
    SCOPE 's3://warehouse',
    config MAP{
        'access_key_id': '...',
        'secret_access_key': '...',
        'region': 'us-east-1',
        'endpoint': 'http://127.0.0.1:9000'
    },
    io_config MAP{
        'read.concurrent': '4',
        'read.chunk_size': '8 MiB',
        'write.concurrent': '4',
        'write.chunk_size': '8 MiB',
        'concurrent_limit': '16'
    },
    retry_config MAP{
        'max_times': '5',
        'factor': '2',
        'jitter': 'true',
        'min_delay': '100ms',
        'max_delay': '10s'
    },
    timeout_config MAP{
        'operation_timeout': '1m',
        'io_timeout': '15s'
    },
    cache_config MAP{
        'memory_size': '256 MiB',
        'disk_path': '/var/cache/opendal',
        'disk_size': '4 GiB',
        'block_size': '4 MiB',
        'min_file_size': '0',
        'max_file_size': '1 GiB',
        'shards': '4'
    }
);
```

Sections are independent. A missing `retry_config`, `timeout_config`, or
`cache_config` means that layer is not applied. `cache_config` presence enables
the cache; there is no `enable` key.

Cross-service sections reject unknown keys, malformed values, and invalid
relationships (for example `retry.factor < 1` or cache min size above max size)
when the effective operator is built. Service `config` remains an arbitrary
passthrough: OpenDAL validates typed values for the selected service, while
unknown service keys may be accepted for forward compatibility.

`config` is passed directly to the selected OpenDAL service. Use the service's
[OpenDAL configuration reference](https://opendal.apache.org/services/) for
valid keys. There are no convenience aliases or generic `layers` bag.

Secret types are registered from OpenDAL's runtime `OperatorRegistry`. Every
scheme compiled into this extension receives the same generic sections; adding
or removing a Cargo `services-*` feature updates dispatch and secret registration
without a C++ service list.

OpenDAL is pinned to upstream commit
`318051086ba99a2c02ac2492105faf4aceb73815`, which contains
`OperatorRegistry::schemes()` from PR #7908. Cargo.lock pins the facade, core,
service, layer, and HTTP-transport crates to that same workspace commit.

## Global defaults

The cross-service sections have matching global DuckDB settings:

```sql
SET GLOBAL opendal_io_config = MAP{'read.concurrent':'4','read.chunk_size':'8 MiB'};
SET GLOBAL opendal_retry_config = MAP{'max_times':'5','jitter':'true'};
SET GLOBAL opendal_timeout_config = MAP{'operation_timeout':'1m','io_timeout':'15s'};
SET GLOBAL opendal_cache_config = MAP{'memory_size':'256 MiB','shards':'4'};
```

Globals apply to every OpenDAL service and path. Secret sections merge over the
corresponding global map key by key. Reset a global section with, for example,
`RESET opendal_cache_config`.

There is no global service `config`: backend keys are service-specific and may
be invalid for other services.

Changing a global or scoped section produces a new effective-config operator;
existing open handles continue using the operator they were opened with.

Size values accept bare bytes and case-sensitive unit strings:

- SI bytes: `KB`, `MB`, `GB`, `TB` (powers of 1000)
- IEC bytes: `KiB`, `MiB`, `GiB`, `TiB` (powers of 1024)
- SI bits: `kb`, `Mb`, `Gb`, `Tb`
- IEC bits: `Kib`, `Mib`, `Gib`, `Tib`

Bit values must convert to a whole number of bytes. Duration values accept bare
seconds or `ns`, `μs`, `ms`, `s`, `m`, `h`, and `d` suffixes.

## Option reference

### `io_config`

| Key | Meaning |
|---|---|
| `read.concurrent` | Concurrent ranged-read requests |
| `read.chunk_size` | Read request chunk size |
| `write.concurrent` | Concurrent multipart-write requests |
| `write.chunk_size` | Multipart-write chunk size |
| `concurrent_limit` | Maximum concurrent service requests |

`read.concurrent` and `write.concurrent` control fan-out within each read or
multipart write. `concurrent_limit` is the shared operator-wide ceiling across
all operations. If the ceiling is lower than the combined read/write fan-out,
requests wait at the concurrent-limit layer; size it for the total concurrency
you want across simultaneous DuckDB reads and writes.

### `retry_config`

| Key | Meaning |
|---|---|
| `max_times` | Maximum retry attempts |
| `factor` | Backoff multiplier |
| `jitter` | Add random jitter (`true` or `1`) |
| `min_delay` | Minimum backoff delay |
| `max_delay` | Maximum backoff delay |

These are all configurable `RetryLayer` backoff options in OpenDAL 0.58. The
remaining Rust API, `with_notify`, installs a custom `RetryInterceptor` callback
and cannot be represented as a SQL setting.

### `timeout_config`

| Key | Meaning |
|---|---|
| `operation_timeout` | Per-operation timeout |
| `io_timeout` | Per-I/O-chunk timeout |

These are the complete `TimeoutLayer` options in OpenDAL 0.58.

### `cache_config`

The cache is OpenDAL's internal data cache. Each effective operator has an
isolated cache namespace, including a derived subdirectory under `disk_path`.

| Key | Default | Meaning |
|---|---:|---|
| `memory_size` | `256 MiB` | In-memory size |
| `disk_path` | unset | Base directory for persistent cache namespaces |
| `disk_size` | `1 GiB` | On-disk size |
| `block_size` | `4 MiB` | On-disk block size |
| `min_file_size` | `0` | Minimum cached object size |
| `max_file_size` | unlimited | Maximum cached object size |
| `shards` | `4` | In-memory cache shards |

This cache stores object data only. Unlike `cache_httpfs`, it does not cache
metadata, glob results, or file handles, and currently exposes no profiling,
status, clear, validation, or eviction-policy controls. Use `cache_httpfs` when
those features are required; see [caching.md](caching.md).
