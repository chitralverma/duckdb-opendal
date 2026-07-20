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
