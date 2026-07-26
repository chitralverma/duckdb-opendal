# Services

Every service follows the same URL shape and is configured the same way — a
scoped secret whose `config` map is passed straight to OpenDAL (see
[configuration.md](configuration.md) for the generic contract and the
cross-service `io_config` / `retry_config` / `timeout_config` / `cache_config`
sections):

```text
scheme://authority/operation/path
```

What the **authority** means is the one thing that differs per service, because
each OpenDAL service parses it itself. This page lists that per service, with a
minimal working secret. Valid `config` keys are the service's own; see the
[OpenDAL service reference](https://opendal.apache.org/services/).

List what your build actually ships:

```sql
FROM opendal_schemes();
```

| Scheme              | Authority    | Required in `config`     |
| ------------------- | ------------ | ------------------------ |
| `fs://` (`file://`) | *(empty)*    | `root`                   |
| `memory://`         | *(empty)*    | —                        |
| `s3://`             | bucket       | credentials, `region`    |
| `azblob://`         | container    | `endpoint`, credentials  |
| `azdls://`          | account host | `filesystem`, credential |
| `azfile://`         | account host | `share_name`, credential |

## `fs` — local filesystem

Empty authority, so paths use a triple slash. The root is explicit config; it is
never inferred from the URL.

```sql
CREATE SECRET local_root (
    TYPE fs,
    SCOPE 'fs://',
    config MAP{'root': '/var/data'}
);

SELECT * FROM read_parquet('fs:///events/2026.parquet');
```

## `memory` — in-process

Needs no configuration at all. Also triple slash:
`memory:///objects/a.parquet`. Legacy `memory://objects/a.parquet` treats
`objects` as the authority under the universal contract.

## `s3` — S3 and S3-compatible

Authority is the bucket. Set `endpoint` for S3-compatible services (MinIO, R2,
…) and `enable_virtual_host_style` as they require.

```sql
CREATE SECRET warehouse (
    TYPE s3,
    SCOPE 's3://warehouse',
    config MAP{
        'access_key_id': '...',
        'secret_access_key': '...',
        'region': 'us-east-1',
        'endpoint': 'http://127.0.0.1:9000'
    }
);

SELECT * FROM read_parquet('s3://warehouse/events/2026.parquet');
```

`s3://` is also claimed by DuckDB's native `httpfs`; see
[schemes.md](schemes.md) for which one wins.

## `azblob` — Azure Blob Storage

Authority is the **container**; the endpoint and credentials come from the
secret.

```sql
CREATE SECRET az_blob (
    TYPE azblob,
    SCOPE 'azblob://warehouse',
    config MAP{
        'endpoint': 'https://myaccount.blob.core.windows.net',
        'account_name': 'myaccount',
        'account_key': '...'
    }
);

SELECT * FROM read_parquet('azblob://warehouse/events/2026.parquet');
```

Azurite (the Azure storage emulator) works with a path-style endpoint that
includes the account segment:
`'endpoint': 'http://127.0.0.1:10000/devstoreaccount1'`.

## `azdls` — Azure Data Lake Storage Gen2

Authority is the **account host**. OpenDAL derives the endpoint
(`https://<authority>`) and the account name (up to the first dot) from it, so
the secret only carries the filesystem and the credential. An `endpoint` in the
secret is ignored — the authority always wins.

```sql
CREATE SECRET az_dls (
    TYPE azdls,
    SCOPE 'azdls://myaccount.dfs.core.windows.net',
    config MAP{'filesystem': 'warehouse', 'account_key': '...'}
);

SELECT * FROM read_parquet(
    'azdls://myaccount.dfs.core.windows.net/events/2026.parquet'
);
```

Instead of `account_key`, a service principal (`client_id`, `client_secret`,
`tenant_id`) or a `sas_token` also works.

## `azfile` — Azure Files

Same shape as `azdls`: authority is the account host, and the secret carries the
share name plus the credential.

```sql
CREATE SECRET az_file (
    TYPE azfile,
    SCOPE 'azfile://myaccount.file.core.windows.net',
    config MAP{'share_name': 'warehouse', 'account_key': '...'}
);

SELECT * FROM read_parquet(
    'azfile://myaccount.file.core.windows.net/events/2026.parquet'
);
```

> Azurite emulates Blob, Queue and Table only — there is no Data Lake or Files
> endpoint, so `azdls` and `azfile` need a real Azure account.
