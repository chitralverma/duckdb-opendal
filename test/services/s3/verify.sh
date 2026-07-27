#!/bin/sh
# Post-test check for the s3 tier: the abort test must not leave orphaned
# multipart uploads behind. Run after `make test-common-s3`, while MinIO is up.
set -eu

cd "$(dirname "$0")"
exec docker compose -f docker-compose.yml run --rm --entrypoint sh init -c '
	mc alias set local http://minio:9000 minioadmin minioadmin >/dev/null 2>&1
	out=$(mc ls --recursive --incomplete local/warehouse/abort-test)
	if [ -n "$out" ]; then
		echo "incomplete multipart uploads remain:"
		echo "$out"
		exit 1
	fi
	echo "No incomplete multipart uploads remain."
'
