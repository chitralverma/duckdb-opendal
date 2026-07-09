#!/usr/bin/env bash
# Seed a local MinIO instance with fixtures for the opendalfs S3 test.
#
# Starts MinIO in Docker (idempotent), creates the `warehouse` bucket, and
# uploads a few Parquet/CSV objects (including a Hive-partitioned prefix).
#
# Usage:
#   scripts/s3_test_seed.sh up      # start MinIO + seed, print the env to export
#   scripts/s3_test_seed.sh down    # stop + remove MinIO
#
# After `up`, run the gated S3 test with:
#   OPENDAL_S3_TEST=1 \
#   AWS_ENDPOINT_URL=http://127.0.0.1:19100 \
#   AWS_ACCESS_KEY_ID=minioadmin AWS_SECRET_ACCESS_KEY=minioadmin \
#   AWS_REGION=us-east-1 \
#   ./build/release/test/unittest --test-dir . test/sql/opendal_fs_s3.test

set -euo pipefail

CONTAINER=opendalfs-minio-test
PORT=19100
ENDPOINT="http://127.0.0.1:${PORT}"
USER=minioadmin
PASS=minioadmin
BUCKET=warehouse

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
DUCKDB_BIN="${REPO_ROOT}/build/release/duckdb"

up() {
	docker rm -f "$CONTAINER" >/dev/null 2>&1 || true
	docker run -d --name "$CONTAINER" -p "${PORT}:9000" \
		-e "MINIO_ROOT_USER=${USER}" -e "MINIO_ROOT_PASSWORD=${PASS}" \
		minio/minio server /data >/dev/null

	echo "waiting for MinIO..." >&2
	for _ in $(seq 1 30); do
		if curl -sf "${ENDPOINT}/minio/health/live" >/dev/null 2>&1; then
			break
		fi
		sleep 1
	done

	# Generate fixtures locally with DuckDB (native filesystem).
	local seed
	seed="$(mktemp -d)"
	"$DUCKDB_BIN" -c "
		COPY (SELECT range AS id, range*2 AS d FROM range(100)) TO '${seed}/f1.parquet' (FORMAT parquet);
		COPY (SELECT range AS id, range*3 AS d FROM range(200)) TO '${seed}/f2.parquet' (FORMAT parquet);
		COPY (SELECT range AS id FROM range(50)) TO '${seed}/nested.parquet' (FORMAT parquet);
		COPY (SELECT range AS id, 'x'||range AS s FROM range(75)) TO '${seed}/data.csv' (FORMAT csv, HEADER);
	" >/dev/null

	docker run --rm --network host -v "${seed}:/data" --entrypoint sh minio/mc -c "
		mc alias set local ${ENDPOINT} ${USER} ${PASS} >/dev/null 2>&1
		mc mb --ignore-existing local/${BUCKET} >/dev/null 2>&1
		mc cp /data/f1.parquet local/${BUCKET}/f1.parquet >/dev/null 2>&1
		mc cp /data/f2.parquet local/${BUCKET}/f2.parquet >/dev/null 2>&1
		mc cp /data/nested.parquet local/${BUCKET}/year=2024/nested.parquet >/dev/null 2>&1
		mc cp /data/data.csv local/${BUCKET}/data.csv >/dev/null 2>&1
	"
	rm -rf "$seed"

	echo "MinIO seeded. Export the following, then run the S3 test:" >&2
	cat <<EOF
export OPENDAL_S3_TEST=1
export AWS_ENDPOINT_URL=${ENDPOINT}
export AWS_ACCESS_KEY_ID=${USER}
export AWS_SECRET_ACCESS_KEY=${PASS}
export AWS_REGION=us-east-1
EOF
}

down() {
	docker rm -f "$CONTAINER" >/dev/null 2>&1 || true
	echo "MinIO stopped." >&2
}

case "${1:-}" in
up) up ;;
down) down ;;
*)
	echo "usage: $0 {up|down}" >&2
	exit 1
	;;
esac
