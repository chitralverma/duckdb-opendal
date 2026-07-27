# `s3://` — S3 and S3-compatible storage

Amazon S3, and anything else that speaks the S3 API — MinIO, Cloudflare R2,
Ceph, Wasabi, and so on. They all use the `s3` secret type; only the endpoint
and addressing style differ.

## URL

```text
s3://bucket/path/to/object
```

The authority is the **bucket**.

## Secret

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

SELECT * FROM read_parquet('s3://warehouse/events/2026.parquet');
```

| Key                         | Meaning                                                     |
| --------------------------- | ----------------------------------------------------------- |
| `access_key_id`             | Access key.                                                 |
| `secret_access_key`         | Secret key.                                                 |
| `session_token`             | Session token for temporary credentials.                    |
| `region`                    | Bucket region.                                              |
| `endpoint`                  | Custom endpoint for S3-compatible services.                 |
| `enable_virtual_host_style` | `true` for virtual-host addressing; `false` for path style. |
| `allow_anonymous`           | Allow unsigned access to public buckets.                    |
| `server_side_encryption`    | e.g. `AES256`, or `aws:kms` with the KMS key id keys.       |
| `enable_versioning`         | Enable version-aware stat, list and reads.                  |

## S3-compatible services

Point `endpoint` at the service. Self-hosted ones usually also need path-style
addressing:

```sql
-- MinIO (and most self-hosted gateways)
CREATE SECRET minio (
    TYPE s3,
    SCOPE 's3://warehouse',
    config MAP{
        'access_key_id': 'minioadmin',
        'secret_access_key': 'minioadmin',
        'region': 'us-east-1',
        'endpoint': 'http://127.0.0.1:9000',
        'enable_virtual_host_style': 'false'
    }
);

-- Cloudflare R2: one endpoint per account, region is always `auto`
CREATE SECRET r2 (
    TYPE s3,
    SCOPE 's3://warehouse',
    config MAP{
        'access_key_id': '...',
        'secret_access_key': '...',
        'region': 'auto',
        'endpoint': 'https://<account-id>.r2.cloudflarestorage.com'
    }
);
```

If a service rejects requests with a signature error, the usual culprits are the
region and `enable_virtual_host_style`.

## Notes

- `s3://` is also claimed by DuckDB's native `httpfs` extension. If both are
  loaded, say which one you want:
  `SET opendal_override_native_filesystems = 's3';` — see
  [coexistence](../coexistence.md).
- Large writes use multipart uploads; tune concurrency and part size with
  `io_config` (see [configuration](../configuration.md)).
