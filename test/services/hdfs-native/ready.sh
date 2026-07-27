#!/bin/sh
# The namenode healthcheck only proves the namenode is up. The client streams
# blocks straight from a datanode, so the tier is not usable until one has
# registered — wait for that before the init step or the suite runs.
set -eu

endpoint="${OPENDAL_HDFS_NATIVE_HTTP_ENDPOINT:-http://127.0.0.1:19410}"
for _ in $(seq 1 60); do
	live=$(curl -sf "$endpoint/jmx?qry=Hadoop:service=NameNode,name=FSNamesystemState" 2>/dev/null |
		tr ',' '\n' | sed -n 's/.*"NumLiveDataNodes"[[:space:]]*:[[:space:]]*\([0-9]*\).*/\1/p')
	if [ "${live:-0}" -ge 1 ] 2>/dev/null; then
		exit 0
	fi
	sleep 2
done

echo "hdfs-native: no datanode registered with the namenode at $endpoint" >&2
exit 1
