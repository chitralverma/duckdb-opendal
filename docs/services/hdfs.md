# `hdfs-native://` — HDFS

Reads and writes against an HDFS cluster over the namenode's RPC protocol, using
a pure-Rust client — no JVM, no `libhdfs`, nothing to install alongside DuckDB.

Unlike the object stores, HDFS has real directories: `opendal_stat` on a prefix
returns `mode = 'directory'`, server-side rename is supported, and recursive
delete removes a subtree.

## URL

```text
hdfs-native://namenode.example.com:8020/path/to/file
```

The authority is the **namenode's RPC address**; it becomes the `name_node`
(`hdfs://<authority>`), and the rest of the URL is the path.

## Secret

A cluster that needs no client-side settings needs no secret:

```sql
SELECT * FROM read_parquet('hdfs-native://namenode.example.com:8020/data/events.parquet');
```

Create one to pass client options or pin a root:

```sql
CREATE SECRET hdfs (
    TYPE "hdfs-native",
    SCOPE 'hdfs-native://namenode.example.com:8020',
    config MAP{'root': '/user/analytics'}
);
```

| Key         | Meaning                                                        |
| ----------- | -------------------------------------------------------------- |
| `name_node` | Namenode URL. Taken from the URL authority; rarely set by hand.  |
| `root`      | Prefix every path is resolved against.                           |
| `options`   | Extra client options, passed through to the HDFS client.         |

`hdfs-native` needs quoting in `CREATE SECRET` (`TYPE "hdfs-native"`) because of
the hyphen.

## Notes

- **Kerberos is not wired up here.** The client authenticates as the OS user
  running DuckDB, so this works against clusters with `dfs.permissions` off, or
  where that user is already authorised.
- **Datanode reachability matters.** The namenode hands back block locations
  pointing at datanode hostnames; DuckDB connects to those directly, so they must
  resolve and be reachable from the client — the usual failure when a cluster
  runs behind NAT or inside Docker.
- Appends are supported (`write_can_append`), which most object stores cannot do.
