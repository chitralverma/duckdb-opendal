# `minio://`, `r2://` — MinIO and Cloudflare R2

S3 presets with a smaller config and provider defaults. Keys that do not apply
are rejected. For other S3-compatible stores, or options these lack, use
[`s3://`](s3-like.md).

## URL

```text
minio://bucket/path/to/object
r2://bucket/path/to/object
```

The authority is the **bucket**.

## Secret

```sql
CREATE SECRET minio (
    TYPE minio,
    SCOPE 'minio://warehouse',
    config MAP{
        'endpoint': 'http://127.0.0.1:9000',
        'access_key_id': 'minioadmin',
        'secret_access_key': 'minioadmin'
    }
);

CREATE SECRET r2 (
    TYPE r2,
    SCOPE 'r2://warehouse',
    config MAP{
        'account_id': '<account-id>',
        'access_key_id': '...',
        'secret_access_key': '...'
    }
);
```

For all `config` keys, see the OpenDAL configuration reference for
[`minio`](https://opendal.apache.org/services/minio/#configuration-reference)
and [`r2`](https://opendal.apache.org/services/r2/#configuration-reference).

Without credentials, both read `AWS_*` env vars and the shared AWS credentials
file.

## Emulator

For `r2://`, Cloudflare's local R2 simulator
([Miniflare](https://developers.cloudflare.com/r2/local-development/), what
`wrangler dev` runs) serves a local bucket over R2's S3 API once the binding has
S3 credentials. Point `endpoint` at its `/cdn-cgi/local/r2/s3` path:

```sql
CREATE SECRET r2_local (
    TYPE r2,
    SCOPE 'r2://warehouse',
    config MAP{
        'endpoint': 'http://127.0.0.1:8787/cdn-cgi/local/r2/s3',
        'access_key_id': 'local-access-key-id',
        'secret_access_key': 'local-secret-access-key'
    }
);
```

It enforces R2's own rules, such as equal-size multipart parts, and only answers
requests addressed to `localhost` or `127.0.0.1`. The `r2` test tier runs on it
(`test/services/r2/`); the `minio` tier runs on MinIO.

## Notes

- **Counterpart:** [`httpfs`](../coexistence.md#claimed-schemes) for both.
  `r2://` is a [shared scheme](../coexistence.md#terms). `httpfs` serves MinIO
  through `s3://`. `minio://` is not a claimed scheme.
- R2 multipart uploads need equal-size parts, so `r2://` writes use 8 MiB parts
  unless `io_config` sets `write.chunk_size`. Through `s3://` pointed at R2, set
  `write.chunk_size` yourself: without it, large writes fail at completion.
