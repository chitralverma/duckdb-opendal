# Services

A *service* is a storage backend, addressed by its own URL scheme. Which ones
your build ships depends on how it was compiled:

```sql
FROM opendal_schemes();
```

| Service                           | Scheme                               | Guide                 |
| --------------------------------- | ------------------------------------ | --------------------- |
| Local filesystem                  | `fs://`, `file://`                   | [fs](fs.md)           |
| In-process memory                 | `memory://`                          | [memory](memory.md)   |
| S3 and S3-compatible              | `s3://`                              | [s3-like](s3-like.md) |
| Google Cloud Storage              | `gcs://`                             | [gcs](gcs.md)         |
| Azure Blob, Data Lake Gen2, Files | `azblob://`, `azdls://`, `azfile://` | [azure](azure.md)     |
| HTTP servers (read-only)          | `http://`, `https://`                | [http](http.md)       |

## How every service works

URLs always look the same:

```text
scheme://authority/path/to/object
```

The **authority** (right after `://`) is the container the service addresses — a
bucket, a container, an account host, or nothing at all. The rest is the object
path and nothing else: it is never re-read as configuration.

Everything else — credentials, endpoints, any service-specific setting — lives
in a DuckDB secret whose `SCOPE` matches the URLs it should serve:

```sql
CREATE SECRET <name> (
    TYPE <scheme>,
    SCOPE '<scheme>://<authority>',
    config MAP{ ... }
);
```

| Scheme      | Authority    | Usually required in `config`     |
| ----------- | ------------ | -------------------------------- |
| `fs://`     | *(empty)*    | `root`                           |
| `memory://` | *(empty)*    | —                                |
| `s3://`     | bucket       | credentials, `region`            |
| `gcs://`    | bucket       | credentials                      |
| `azblob://` | container    | `endpoint`, credentials          |
| `azdls://`  | account host | `filesystem`, credential         |
| `azfile://` | account host | `share_name`, credential         |
| `http://`   | host         | — (credentials only if required) |

Things that hold for all of them:

- **One bucket/container per secret.** Create several, each with its own
  `SCOPE`; the most specific match wins.
- **`config` is passed straight to OpenDAL.** Each guide lists the keys you
  normally need; the full set is in the
  [OpenDAL service reference](https://opendal.apache.org/services/).
- **Tuning is service-independent.** `io_config`, `retry_config`,
  `timeout_config` and `cache_config` work the same everywhere — see
  [configuration](../configuration.md).
- **Not every service does everything.** `http`/`https` are read-only, for
  example. `FROM opendal_info('<url>');` reports a service's capabilities, and
  an unsupported operation fails with `service '<x>' does not support <op>`
  rather than a confusing protocol error.
- **Stuck?** That same function reports whether the URL is usable, which secret
  matched, and what is missing.

## Guide template

Every service guide has the same shape, so they stay comparable and easy to keep
current:

```text
# `<scheme>://` — <service name>      one-line description
## URL                                URL form + what the authority means
## Secret                             a working CREATE SECRET + key table
## Emulator                           optional: local/emulated setup
## Notes                              gotchas, limits, scheme conflicts
```

Add extra sections where a service earns them (`s3-like` has one per compatible
provider), but keep the order and the names above.
