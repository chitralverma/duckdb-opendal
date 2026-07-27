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

# ── SQLLogicTest targets: common suite run per service (see CONTRIBUTING.md) ──
# No target below names a service: everything a tier needs lives in
# test/services/<svc>/ (docker-compose.yml, optional ready.sh / verify.sh, an
# optional one-shot compose service named `init`).
.PHONY: test-local fixtures
UNITTEST_BIN := ./build/release/test/unittest
SERVICES_DIR := test/services
FIXTURES_DIR := $(SERVICES_DIR)/fixtures
SERVICE_DIR = $(SERVICES_DIR)/$*
COMPOSE = docker compose -f $(SERVICE_DIR)/docker-compose.yml
NEED_COMPOSE = test -f $(SERVICE_DIR)/docker-compose.yml || \
	{ echo "no $(SERVICE_DIR)/docker-compose.yml — '$*' is not a provisioned service" >&2; exit 2; }

fixtures: ## Generate the shared external-object test fixtures
	mkdir -p $(FIXTURES_DIR)
	./build/release/duckdb -c "COPY (SELECT range AS id, range * 2 AS d FROM range(100)) TO '$(FIXTURES_DIR)/ext_seed.parquet' (FORMAT parquet); COPY (SELECT range AS id, 'v' || range AS v FROM range(50)) TO '$(FIXTURES_DIR)/ext_seed.csv' (FORMAT csv, HEADER);"

# The recipe is @-silent because it has to resolve the config first (see
# test/services/render_config.py), so it echoes each unittest invocation itself —
# copy-pasteable to rerun a failing tier directly.
test-common-%: ## Run the common suite for a service (e.g. test-common-fs)
	@cfg=$$(python3 $(SERVICES_DIR)/render_config.py $*) || exit $$?; \
	  run() { echo "DUCKDB_TEST_CONFIG=$$cfg $(UNITTEST_BIN) '$$1'"; \
	          DUCKDB_TEST_CONFIG=$$cfg $(UNITTEST_BIN) "$$1"; }; \
	  run "test/sql/common/*" || exit $$?; \
	  if [ -f test/sql/services/$*.test ]; then run "test/sql/services/$*.test" || exit $$?; fi

test-local: test-common-fs test-common-memory ## Run all infra-free tiers (fs + memory)

%-up: fixtures ## Start + provision a service tier (e.g. s3-up)
	@$(NEED_COMPOSE)
	$(COMPOSE) up -d --wait
	@if [ -f $(SERVICE_DIR)/ready.sh ]; then sh $(SERVICE_DIR)/ready.sh; fi
	@if $(COMPOSE) config --profiles | grep -qx init; then $(COMPOSE) run --rm init; fi

%-down: ## Stop and remove a service tier (e.g. s3-down)
	@$(NEED_COMPOSE)
	$(COMPOSE) down -v

%-verify: ## Run a service's post-test checks, if it has any (e.g. s3-verify)
	@if [ -f $(SERVICE_DIR)/verify.sh ]; then sh $(SERVICE_DIR)/verify.sh; \
	 else echo "$*: no post-test checks"; fi

# ── Rust convenience targets (crate lives in opendal/core) ───────────────────
.PHONY: rust-build rust-test rust-fmt rust-fmt-check cpp-fmt format-all rust-lint rust-clean cargo-clean clean-all help
CARGO_MANIFEST := opendal/Cargo.toml

rust-build: ## Build the Rust core (release)
	cargo build --release --manifest-path $(CARGO_MANIFEST)

rust-test: ## Run the Rust unit tests
	cargo test --release --manifest-path $(CARGO_MANIFEST)

rust-fmt: ## Format the Rust sources
	cargo fmt --manifest-path $(CARGO_MANIFEST)

rust-fmt-check: ## Check Rust formatting without modifying files (CI)
	cargo fmt --manifest-path $(CARGO_MANIFEST) --all -- --check

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
