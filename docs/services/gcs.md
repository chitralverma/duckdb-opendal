# `gcs://` — Google Cloud Storage

Object storage in a GCS bucket.

## URL

```text
gcs://bucket/path/to/object
```

The authority is the **bucket**.

## Secret

```sql
CREATE SECRET warehouse (
    TYPE gcs,
    SCOPE 'gcs://warehouse',
    config MAP{
        'credential_path': '/path/to/service-account.json'
    }
);

SELECT * FROM read_parquet('gcs://warehouse/events/2026.parquet');
```

For all `config` keys, see the OpenDAL
[`gcs` configuration reference](https://opendal.apache.org/services/gcs/#configuration-reference).

With no credential keys the service falls back to application default
credentials, so on GCE or with `gcloud auth application-default login` an empty
`config MAP{}` is enough.

## Emulator

`fake-gcs-server` and similar need an explicit endpoint, and the credential
discovery has to be turned off or it will look for real ones:

```sql
CREATE SECRET gcs_emulator (
    TYPE gcs,
    SCOPE 'gcs://warehouse',
    config MAP{
        'endpoint': 'http://127.0.0.1:4443',
        'allow_anonymous': 'true',
        'disable_config_load': 'true',
        'disable_vm_metadata': 'true'
    }
);
```

## Notes

- **Counterpart:** [`httpfs`](../coexistence.md#claimed-schemes). `gcs://` is a
  [shared scheme](../coexistence.md#terms). `httpfs` also serves `gs://`. It
  uses the S3-compatible XML API with HMAC keys or a bearer token. This service
  uses the JSON API with the credentials above.
- There is no server-side rename. Overwriting an existing object copies and
  deletes, and `opendal_copy(..., if_not_exists := true)` is rejected because
  the service does not support that precondition.
- Large writes are resumable uploads; tune concurrency and chunk size with
  `io_config` (see [configuration](../configuration.md)).
- [`opendal_presign`](../table-functions.md#opendal_presign) signs with a
  service-account key (`credential` or `credential_path`). Other credentials
  fail with `service account or token + signer identity required for query
  signing`. With `allow_anonymous`, it returns the URL with no signature. The
  URL uses the XML API host (`storage.googleapis.com`), not the JSON API.
