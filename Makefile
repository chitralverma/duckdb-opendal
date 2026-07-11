PROJ_DIR := $(dir $(abspath $(lastword $(MAKEFILE_LIST))))

# Configuration of extension
EXT_NAME=opendal
EXT_CONFIG=${PROJ_DIR}extension_config.cmake

# Include the Makefile from extension-ci-tools. Its first target (`all: release`)
# stays the default goal, so a bare `make` still builds the extension.
include extension-ci-tools/makefiles/duckdb_extension.Makefile

# ── Rust convenience targets ─────────────────────────────────────────────────
.PHONY: rust-build rust-test rust-fmt rust-lint rust-clean cargo-clean clean-all help

rust-build: ## Build the Rust core (release)
	cargo build --release

rust-test: ## Run the Rust unit tests
	cargo test --release

rust-fmt: ## Format the Rust sources
	cargo fmt

rust-lint: ## Lint the Rust sources (clippy, warnings as errors)
	cargo clippy --release -- -D warnings

rust-clean cargo-clean: ## Clean the Rust workspace (target/)
	cargo clean

# Make the extension-ci-tools `clean` also clean the Rust workspace, so a single
# `make clean` is consistent. `clean-all` is a friendly alias.
clean: cargo-clean
clean-all: clean ## Clean everything (build/, testext/, DuckDB tree, and Rust target/)

help: ## Show this help
	@echo "opendal — make targets (bare 'make' builds the extension via 'all: release'):"
	@grep -hE '^[a-zA-Z_-]+.*:.*## .*$$' $(MAKEFILE_LIST) | \
	  awk 'BEGIN {FS = ":.*## "}; {printf "  \033[36m%-14s\033[0m %s\n", $$1, $$2}'