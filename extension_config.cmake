# This file is included by DuckDB's build system. It specifies which extension to load

# Extension from this repo (C++ + Rust both live under opendal/)
duckdb_extension_load(opendal
    SOURCE_DIR ${CMAKE_CURRENT_LIST_DIR}
    INCLUDE_DIR ${CMAKE_CURRENT_LIST_DIR}/opendal/src/include
)

# Any extra extensions that should be built
# e.g.: duckdb_extension_load(json)