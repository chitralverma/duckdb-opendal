# `azdls://` — Azure Data Lake Storage Gen2

Storage accounts with a hierarchical namespace, over the Data Lake (`dfs`)
endpoint — real directories, directory rename and recursive delete.

## URL

```text
azdls://myaccount.dfs.core.windows.net/path/to/object
```

The authority is the **account host**. The endpoint (`https://` + authority) and
the account name (everything before the first dot) are taken from it, so the
secret only carries the filesystem and the credential.

## Secret

```sql
CREATE SECRET az_dls (
    TYPE azdls,
    SCOPE 'azdls://myaccount.dfs.core.windows.net',
    config MAP{
        'filesystem': 'warehouse',
        'account_key': '...'
    }
);

SELECT * FROM read_parquet(
    'azdls://myaccount.dfs.core.windows.net/events/2026.parquet'
);
```

| Key                                       | Meaning                                                  |
| ----------------------------------------- | -------------------------------------------------------- |
| `filesystem`                              | Container ("filesystem") name. Required.                 |
| `account_key`                             | Shared key.                                              |
| `sas_token`                               | Shared access signature, instead of `account_key`.       |
| `client_id`, `client_secret`, `tenant_id` | Service-principal credentials.                           |
| `authority_host`                          | Login host for service principals, for sovereign clouds. |
| `enable_hns`                              | `true` if the account has a hierarchical namespace.      |

## Notes

- An `endpoint` in the secret is ignored — the URL authority always wins. To
  reach a sovereign cloud, put its host in the URL
  (e.g. `azdls://myaccount.dfs.core.chinacloudapi.cn/…`).
- One filesystem per secret. To use several, create one secret per filesystem
  and give each URL a distinct scope, or set `filesystem` per query with a
  narrower scope.
- The Azurite emulator does not implement the Data Lake endpoint; this service
  needs a real Azure account.
