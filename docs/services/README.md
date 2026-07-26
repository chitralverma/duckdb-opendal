# Services

A *service* is a storage backend, addressed by its own URL scheme. Which ones
your build ships depends on how it was compiled:

```sql
FROM opendal_schemes();
```

## Pick your service

| Service                      | Scheme             | Guide               |
| ---------------------------- | ------------------ | ------------------- |
| Local filesystem             | `fs://`, `file://` | [fs](fs.md)         |
| In-process memory            | `memory://`        | [memory](memory.md) |
| S3 and S3-compatible         | `s3://`            | [s3](s3.md)         |
| Azure Blob Storage           | `azblob://`        | [azblob](azblob.md) |
| Azure Data Lake Storage Gen2 | `azdls://`         | [azdls](azdls.md)   |
| Azure Files                  | `azfile://`        | [azfile](azfile.md) |

## What every service has in common

URLs always look the same:

```text
scheme://authority/path/to/object
```

The **authority** (the part right after `://`) is the container the service
addresses — a bucket for `s3`, a container for `azblob`, an account host for the
Data Lake and Files services, nothing at all for `fs` and `memory`. The rest of
the URL is the object path and nothing else: it is never re-interpreted as
configuration.

Everything else — credentials, endpoints, and any service-specific setting —
lives in a DuckDB secret whose `SCOPE` matches the URLs it should serve:

```sql
CREATE SECRET <name> (
    TYPE <scheme>,
    SCOPE '<scheme>://<authority>',
    config MAP{ ... }
);
```

| Scheme      | Authority    | Usually required in `config` |
| ----------- | ------------ | ---------------------------- |
| `fs://`     | *(empty)*    | `root`                       |
| `memory://` | *(empty)*    | —                            |
| `s3://`     | bucket       | credentials, `region`        |
| `azblob://` | container    | `endpoint`, credentials      |
| `azdls://`  | account host | `filesystem`, credential     |
| `azfile://` | account host | `share_name`, credential     |

`config` keys are the OpenDAL service's own and are passed straight through, so
the [OpenDAL service reference](https://opendal.apache.org/services/) is the
complete list; each guide here covers the keys you normally need. Secrets also
carry the cross-service tuning sections (`io_config`, `retry_config`,
`timeout_config`, `cache_config`) — see [configuration](../configuration.md).

## Checking a URL

`opendal_info` tells you whether a URL is usable, which secret matched, and what
is missing if it isn't:

```sql
FROM opendal_info('s3://warehouse/events/');
```
