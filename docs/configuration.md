# Configuration

Everything the extension needs — credentials, endpoints, concurrency, retries,
timeouts, caching — is configured in one of two places:

- a **secret**, which applies to the URLs its `SCOPE` matches;
- a **global setting**, which applies to every service and path.

Queries themselves stay plain SQL.

## Secrets

Create one secret per service and scope. `TYPE` is the URL scheme; `SCOPE` is a
URL prefix (optional, but recommended — without it the secret matches every URL
of that type).

```sql
CREATE SECRET warehouse (
    TYPE s3,
    SCOPE 's3://warehouse',
    config MAP{
        'access_key_id': '...',
        'secret_access_key': '...',
        'region': 'us-east-1'
    }
);
```

A secret has five independent sections. `config` is the service's own settings
(see the [service guides](services/README.md)); the other four are the same for
every service:

| Section          | Controls                                  |
| ---------------- | ----------------------------------------- |
| `config`         | Service credentials and options.          |
| `io_config`      | Read/write concurrency and chunk sizes.   |
| `retry_config`   | Retry attempts and backoff.               |
| `timeout_config` | Operation and per-chunk timeouts.         |
| `cache_config`   | Data caching (see [caching](caching.md)). |

```sql
CREATE SECRET warehouse_tuned (
    TYPE s3,
    SCOPE 's3://warehouse',
    config MAP{'access_key_id': '...', 'secret_access_key': '...', 'region': 'us-east-1'},
    io_config MAP{
        'read.concurrent': '4',
        'read.chunk_size': '8 MiB',
        'write.concurrent': '4',
        'write.chunk_size': '8 MiB',
        'concurrent_limit': '16'
    },
    retry_config MAP{'max_times': '5', 'jitter': 'true'},
    timeout_config MAP{'operation_timeout': '1m', 'io_timeout': '15s'}
);
```

Omit a section and its behaviour is simply not applied — there is no `enable`
key anywhere. All values are strings.

### Which secret wins

DuckDB matches the **most specific** scope. So a broad secret can set the
account-level defaults while a narrower one overrides a single prefix:

```sql
CREATE SECRET all_buckets (TYPE s3, SCOPE 's3://', config MAP{...});
CREATE SECRET hot_prefix  (TYPE s3, SCOPE 's3://warehouse/hot', config MAP{...});
```

Use `opendal_info` to see which secret a URL actually resolves to:

```sql
SELECT scheme, secret_name, usable, message FROM opendal_info('s3://warehouse/hot/a.parquet');
```

## Global defaults

The four cross-service sections have matching global settings:

```sql
SET GLOBAL opendal_io_config      = MAP{'read.concurrent':'4','read.chunk_size':'8 MiB'};
SET GLOBAL opendal_retry_config   = MAP{'max_times':'5','jitter':'true'};
SET GLOBAL opendal_timeout_config = MAP{'operation_timeout':'1m','io_timeout':'15s'};
SET GLOBAL opendal_cache_config   = MAP{'memory_size':'256 MiB','shards':'4'};
```

Secrets merge **over** the globals key by key, so a secret only has to state
what it changes. Clear a global with `RESET opendal_cache_config`.

There is no global `config`: service keys mean different things per service.

Changing a setting or secret affects operations started afterwards; files
already open keep the configuration they were opened with.

## URLs

Every service uses the same URL shape:

```text
scheme://authority/path/to/object
scheme:///path/to/object          # services with no authority (fs, memory)
```

The path is only ever an object path — it is never re-read as configuration.
What the authority means is per service; see the
[service guides](services/README.md).

So that one URL always means one object, the extension rejects URLs it cannot
read unambiguously: query strings, fragments, `user@host`, invalid UTF-8, NUL
bytes, and `%2F` (an escaped separator would be indistinguishable from a real
one). Any other percent-encoding is decoded once, so `%20` and friends work:

```sql
FROM opendal_stat('memory:///reports/q1 2026.parquet');     -- fine
FROM opendal_stat('memory:///reports/q1%202026.parquet');   -- same object
FROM opendal_stat('memory:///reports/q1%2F2026.parquet');   -- rejected
```

Leading and trailing whitespace in a path is trimmed.

## Units

Sizes accept plain byte counts or a unit suffix (case matters):

- bytes, powers of 1000: `KB`, `MB`, `GB`, `TB`
- bytes, powers of 1024: `KiB`, `MiB`, `GiB`, `TiB`
- bits: `kb`, `Mb`, `Gb`, `Tb` and `Kib`, `Mib`, `Gib`, `Tib` — must come out to
  a whole number of bytes

Durations accept plain seconds or `ns`, `μs`, `ms`, `s`, `m`, `h`, `d`.

## Option reference

### `io_config`

| Key                | Meaning                            | Notes                              |
| ------------------ | ---------------------------------- | ---------------------------------- |
| `read.concurrent`  | Parallel ranged reads per file     |                                    |
| `read.chunk_size`  | Size of each read request          |                                    |
| `write.concurrent` | Parallel parts per multipart write |                                    |
| `write.chunk_size` | Size of each uploaded part         | Services impose their own minimums |
| `concurrent_limit` | Ceiling on in-flight requests      | Applies across all operations      |

`read.concurrent` and `write.concurrent` control fan-out *within* a single read
or write; `concurrent_limit` caps the operator as a whole. Set the ceiling for
the total concurrency you want across simultaneous queries — if it is lower than
the combined fan-out, requests simply queue.

Bigger chunks favour throughput on large scans; smaller chunks favour latency on
selective reads.

### `retry_config`

| Key         | Meaning                          |
| ----------- | -------------------------------- |
| `max_times` | Maximum attempts after a failure |
| `factor`    | Backoff multiplier               |
| `jitter`    | Randomise backoff (`true`)       |
| `min_delay` | Shortest backoff                 |
| `max_delay` | Longest backoff                  |

Retries cover transient failures (throttling, connection resets). Permanent
errors such as "not found" or "access denied" fail immediately.

### `timeout_config`

| Key                 | Meaning                                     |
| ------------------- | ------------------------------------------- |
| `operation_timeout` | Budget for a whole operation                |
| `io_timeout`        | Budget for a single chunk of data in flight |

### `cache_config`

See [caching](caching.md) for the full guide.

| Key             | Default   | Meaning                             |
| --------------- | --------- | ----------------------------------- |
| `memory_size`   | `256 MiB` | In-memory cache size                |
| `disk_path`     | unset     | Enables the persistent on-disk tier |
| `disk_size`     | `1 GiB`   | On-disk cache size                  |
| `block_size`    | `4 MiB`   | On-disk block size                  |
| `min_file_size` | `0`       | Skip caching objects smaller than   |
| `max_file_size` | unlimited | Skip caching objects larger than    |
| `shards`        | `4`       | In-memory shards                    |

## Errors and validation

The cross-service sections are strict: unknown keys, malformed values and
impossible combinations (a `retry.factor` below 1, a minimum cache size above
the maximum) are reported when the operator is built — that is, on your first
query against a matching URL, not at `CREATE SECRET` time.

Service `config` is a passthrough. OpenDAL validates the keys it knows and may
accept unknown ones, so a typo there usually surfaces as a connection or
permission error rather than a config error — `opendal_info` (above) reports
what the URL actually resolved to.

## Sharing schemes with other extensions

Some secret types (`s3`, for example) are also registered by DuckDB's native
extensions. See [coexistence](coexistence.md) for load order and how to choose
which extension serves a scheme.
