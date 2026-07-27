# `gcs://` — Google Cloud Storage

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

| Key                     | Meaning                                                        |
| ----------------------- | -------------------------------------------------------------- |
| `credential`            | Service-account JSON, base64-encoded.                          |
| `credential_path`       | Path to a service-account JSON file.                           |
| `service_account`       | Service-account name to impersonate.                           |
| `token`                 | OAuth2 access token, used directly.                            |
| `scope`                 | OAuth2 scope; defaults to full control.                        |
| `endpoint`              | Custom endpoint, e.g. an emulator.                             |
| `allow_anonymous`       | Allow unsigned access to public buckets and emulators.         |
| `disable_config_load`   | Skip credential discovery from the environment and gcloud.     |
| `disable_vm_metadata`   | Skip the GCE metadata server (avoids a hang off-GCE).          |
| `predefined_acl`        | Predefined ACL applied to written objects.                     |
| `default_storage_class` | Storage class for written objects, e.g. `NEARLINE`.            |

With no credential keys the service falls back to application default
credentials, so on GCE or with `gcloud auth application-default login` an empty
`config MAP{}` is enough.

## Emulators

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

- `gcs://` is also claimed by DuckDB's native `httpfs` extension. If both are
  loaded, say which one you want:
  `SET opendal_override_native_filesystems = 'gcs';` — see
  [coexistence](../coexistence.md).
- Multiple buckets? Create one secret per bucket, each with its own `SCOPE`. The
  most specific matching scope wins.
- There is no server-side rename. Overwriting an existing object copies and
  deletes, and `opendal_copy(..., if_not_exists := true)` is rejected because
  the service does not support that precondition.
- Large writes are resumable uploads; tune them with `io_config` (see
  [configuration](../configuration.md)).
