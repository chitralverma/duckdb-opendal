#!/bin/sh
# Readiness gate for the gcs tier: fake-gcs-server has no shell, so compose
# cannot healthcheck it. Poll the bucket endpoint from the host instead.
set -eu

endpoint="${OPENDAL_GCS_ENDPOINT:-http://127.0.0.1:19200}"
for _ in $(seq 1 30); do
	if curl -sf "$endpoint/storage/v1/b/warehouse" >/dev/null 2>&1; then
		exit 0
	fi
	sleep 1
done

echo "gcs: fake-gcs-server did not become ready at $endpoint" >&2
exit 1
