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
        'read.chunk': '8388608',
        'write.concurrent': '4',
        'write.chunk': '8388608',
        'concurrent_limit': '16'
    },
    retry_config MAP{
        'max_times': '5',
        'factor': '2',
        'jitter': 'true',
        'min_delay_ms': '100',
        'max_delay_ms': '10000'
    },
    timeout_config MAP{
        'seconds': '60',
        'io_seconds': '15'
    },
    cache_config MAP{
        'memory_mb': '256',
        'disk_path': '/var/cache/opendal',
        'disk_mb': '4096',
        'block_mb': '4',
        'min_file_size': '0',
        'max_file_size': '1073741824',
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

## Global defaults

The cross-service sections have matching global DuckDB settings:

```sql
SET GLOBAL opendal_io_config = MAP{'read.concurrent':'4','read.chunk':'8388608'};
SET GLOBAL opendal_retry_config = MAP{'max_times':'5','jitter':'true'};
SET GLOBAL opendal_timeout_config = MAP{'seconds':'60','io_seconds':'15'};
SET GLOBAL opendal_cache_config = MAP{'memory_mb':'256','shards':'4'};
```

Globals apply to every OpenDAL service and path. Secret sections merge over the
corresponding global map key by key. Reset a global section with, for example,
`RESET opendal_cache_config`.

There is no global service `config`: backend keys are service-specific and may
be invalid for other services.

Changing a global or scoped section produces a new effective-config operator;
existing open handles continue using the operator they were opened with.

## Option reference

### `io_config`

| Key | Meaning |
|---|---|
| `read.concurrent` | Concurrent ranged-read requests |
| `read.chunk` | Read request chunk size in bytes |
| `write.concurrent` | Concurrent multipart-write requests |
| `write.chunk` | Multipart-write chunk size in bytes |
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
| `min_delay_ms` | Minimum backoff delay |
| `max_delay_ms` | Maximum backoff delay |

These are all configurable `RetryLayer` backoff options in OpenDAL 0.58. The
remaining Rust API, `with_notify`, installs a custom `RetryInterceptor` callback
and cannot be represented as a SQL setting.

### `timeout_config`

| Key | Meaning |
|---|---|
| `seconds` | Per-operation timeout |
| `io_seconds` | Per-I/O-chunk timeout |

These are the complete `TimeoutLayer` options in OpenDAL 0.58.

### `cache_config`

The cache is OpenDAL's internal data cache. Each effective operator has an
isolated cache namespace, including a derived subdirectory under `disk_path`.

| Key | Default | Meaning |
|---|---:|---|
| `memory_mb` | `256` | In-memory capacity |
| `disk_path` | unset | Base directory for persistent cache namespaces |
| `disk_mb` | `1024` | On-disk capacity |
| `block_mb` | `4` | On-disk block size |
| `min_file_size` | `0` | Minimum cached object size in bytes |
| `max_file_size` | unlimited | Maximum cached object size in bytes |
| `shards` | `4` | In-memory cache shards |

This cache stores object data only. Unlike `cache_httpfs`, it does not cache
metadata, glob results, or file handles, and currently exposes no profiling,
status, clear, validation, or eviction-policy controls. Use `cache_httpfs` when
those features are required; see [caching.md](caching.md).
