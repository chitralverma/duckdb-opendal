#!/bin/sh
# Build the client config: the core-site.xml that MiniDFS writes at startup
# (it holds the KMS address, which has a random port), plus this tier's
# hdfs-site.xml. client.env points HADOOP_CONF_DIR at it.
set -eu

tier_dir="$(dirname "$0")"
conf_dir="$tier_dir/../../../build/test-services/hdfs-native"
mkdir -p "$conf_dir"
docker compose -f "$tier_dir/docker-compose.yml" cp \
	minidfs:/minidfs/target/test/core-site.xml "$conf_dir/core-site.xml"
cp "$tier_dir/hdfs-site.xml" "$conf_dir/hdfs-site.xml"
