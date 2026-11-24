# `minio://`, `r2://` — MinIO and Cloudflare R2

Dedicated presets for two S3-compatible stores. They share the `s3` runtime but
take a smaller config with provider defaults, and reject keys that do not apply
rather than ignoring them. For any other S3-compatible store, or an option
these presets lack, use [`s3://`](s3-like.md).

## URL

```text
minio://bucket/path/to/object
r2://bucket/path/to/object
```

The authority is the **bucket**.

## Secret

```sql
-- MinIO: endpoint is required; region defaults to `auto`; always path style.
CREATE SECRET minio (
    TYPE minio,
    SCOPE 'minio://warehouse',
    config MAP{
        'endpoint': 'http://127.0.0.1:9000',
        'access_key_id': 'minioadmin',
        'secret_access_key': 'minioadmin'
    }
);

-- R2: the endpoint is derived from the account id; region is always `auto`.
CREATE SECRET r2 (
    TYPE r2,
    SCOPE 'r2://warehouse',
    config MAP{
        'account_id': '<account-id>',
        'access_key_id': '...',
        'secret_access_key': '...'
    }
);

SELECT * FROM read_parquet('minio://warehouse/events/2026.parquet');
```

| Key                 | Service | Meaning                                                    |
| ------------------- | ------- | ---------------------------------------------------------- |
| `access_key_id`     | both    | Access key; set with `secret_access_key`.                  |
| `secret_access_key` | both    | Secret key.                                                |
| `session_token`     | both    | Session token for temporary credentials.                   |
| `endpoint`          | minio   | Deployment endpoint. Required.                             |
| `region`            | minio   | Signing region. Default `auto`.                            |
| `skip_signature`    | minio   | Send unsigned requests; not with credentials.              |
| `account_id`        | r2      | Derives `https://<account_id>.r2.cloudflarestorage.com`.   |
| `jurisdiction`      | r2      | `eu` or `fedramp`; needs `account_id`.                     |
| `endpoint`          | r2      | Explicit endpoint (proxy, gateway); not with `account_id`. |

Without credential keys, both fall back to the standard `AWS_*` environment
variables and the shared AWS credentials file. They never use EC2 metadata, SSO
or role assumption.

## Notes

- **`r2://` is also claimed by DuckDB's `httpfs`.** To route it through this
  extension: `SET opendal_override_schemes = 'r2';` — see
  [coexistence](../coexistence.md). `minio://` is served only by this extension.
- **R2 multipart uploads need equal-size parts**, so `r2://` writes use 8 MiB
  parts by default. Change it with `io_config MAP{'write.chunk_size': ...}`
  (see [configuration](../configuration.md)), but keep it fixed.
- Existing `TYPE s3` secrets for MinIO or R2 keep working on `s3://`.
