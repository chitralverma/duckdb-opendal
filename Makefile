PROJ_DIR := $(dir $(abspath $(lastword $(MAKEFILE_LIST))))

# Configuration of extension
EXT_NAME=opendal
EXT_CONFIG=${PROJ_DIR}extension_config.cmake

# Submodules live at the root. Point the ci-tools makefile at the DuckDB
# source there (DUCKDB_SRCDIR is declared with ?= so this override wins).
DUCKDB_SRCDIR := ./duckdb/

# Include the Makefile from extension-ci-tools. Its first target (`all: release`)
# stays the default goal, so a bare `make` still builds the extension.
include extension-ci-tools/makefiles/duckdb_extension.Makefile

# ── SQLLogicTest targets: one common suite, run per backend via a test-config ─
# The common suite (test/sql/common/*) is service-agnostic: each config supplies
# ${OPENDAL_BASE} + the backend secret. Service-specific quirks live in
# test/sql/services/<svc>.test. See docs/testing.md.
.PHONY: test-local fixtures s3-up s3-down s3-assert-no-incomplete
UNITTEST_BIN := ./build/release/test/unittest
FIXTURES_DIR := test/services/fixtures
S3_COMPOSE := test/services/s3/docker-compose.yml
S3_MC := docker compose -f $(S3_COMPOSE) run --rm --entrypoint sh minio-init -c

# Shared external-object fixtures: a canonical parquet + csv generated once with
# DuckDB and uploaded (unchanged) by every provisioned backend's init sidecar to
# <backend>/external/, so common/external_read.test can verify reads of objects
# opendal did not write. Backend-agnostic, so all backends share one set.
fixtures: ## Generate the shared external-object test fixtures
	mkdir -p $(FIXTURES_DIR)
	./build/release/duckdb -c "COPY (SELECT range AS id, range * 2 AS d FROM range(100)) TO '$(FIXTURES_DIR)/ext_seed.parquet' (FORMAT parquet); COPY (SELECT range AS id, 'v' || range AS v FROM range(50)) TO '$(FIXTURES_DIR)/ext_seed.csv' (FORMAT csv, HEADER);"

# Run the common suite (and the backend's own quirks test, if any) against one
# backend. `make test-common-<name>` works for any test/configs/<name>.json —
# adding an infra-free backend needs no Makefile change.
test-common-%: ## Run the common suite for a backend (e.g. test-common-fs)
	DUCKDB_TEST_CONFIG=test/configs/$*.json $(UNITTEST_BIN) "test/sql/common/*"
	@if [ -f test/sql/services/$*.test ]; then \
	  DUCKDB_TEST_CONFIG=test/configs/$*.json $(UNITTEST_BIN) "test/sql/services/$*.test"; \
	fi

test-local: test-common-fs test-common-memory ## Run all infra-free tiers (fs + memory)

s3-up: fixtures ## Start + provision the MinIO test backend (buckets + fixtures + fault proxy)
	docker compose -f $(S3_COMPOSE) up -d --wait minio fault-proxy
	docker compose -f $(S3_COMPOSE) run --rm minio-init

s3-down: ## Stop and remove the MinIO test backend
	docker compose -f $(S3_COMPOSE) down -v

s3-assert-no-incomplete: ## Assert no orphaned multipart uploads remain (run after test-common-s3)
	@$(S3_MC) "mc alias set local http://minio:9000 minioadmin minioadmin >/dev/null 2>&1; \
	  out=\$$(mc ls --recursive --incomplete local/warehouse/abort-test); \
	  if [ -n \"\$$out\" ]; then echo 'incomplete multipart uploads remain:'; echo \"\$$out\"; exit 1; fi; \
	  echo 'No incomplete multipart uploads remain.'"

# ── Rust convenience targets (crate lives in opendal/core) ───────────────────
.PHONY: rust-build rust-test rust-fmt cpp-fmt format-all rust-lint rust-clean cargo-clean clean-all help
CARGO_MANIFEST := opendal/Cargo.toml

rust-build: ## Build the Rust core (release)
	cargo build --release --manifest-path $(CARGO_MANIFEST)

rust-test: ## Run the Rust unit tests
	cargo test --release --manifest-path $(CARGO_MANIFEST)

rust-fmt: ## Format the Rust sources
	cargo fmt --manifest-path $(CARGO_MANIFEST)

format-all: format rust-fmt ## Format both C++ and Rust sources

rust-lint: ## Lint the Rust sources (clippy, warnings as errors)
	cargo clippy --release --manifest-path $(CARGO_MANIFEST) -- -D warnings

rust-clean cargo-clean: ## Clean the Rust build artifacts (target/)
	cargo clean --manifest-path $(CARGO_MANIFEST)

# Make the extension-ci-tools `clean` also clean the Rust artifacts, so a single
# `make clean` is consistent. `clean-all` is a friendly alias.
clean: cargo-clean
clean-all: clean ## Clean everything (build/, testext/, DuckDB tree, and Rust target/)

help: ## Show this help
	@echo "opendal — make targets (bare 'make' builds the extension via 'all: release'):"
	@grep -hE '^[a-zA-Z_-]+.*:.*## .*$$' $(MAKEFILE_LIST) | \
	  awk 'BEGIN {FS = ":.*## "}; {printf "  \033[36m%-14s\033[0m %s\n", $$1, $$2}'
