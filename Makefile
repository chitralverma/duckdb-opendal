PROJ_DIR := $(dir $(abspath $(lastword $(MAKEFILE_LIST))))

# Configuration of extension
EXT_NAME=opendal_fs
EXT_CONFIG=${PROJ_DIR}extension_config.cmake

# Include the Makefile from extension-ci-tools
include extension-ci-tools/makefiles/duckdb_extension.Makefile

# Also clean the Rust workspace so `make clean` is consistent (the
# extension-ci-tools `clean` only removes build/ + testext/ + the DuckDB tree).
# Adding a prerequisite to the existing `clean` target runs this alongside it.
.PHONY: cargo-clean
cargo-clean:
	cargo clean

clean: cargo-clean