cmake_minimum_required(VERSION 3.5)

# Set extension name here
set(TARGET_NAME opendal_fs)

set(EXTENSION_NAME ${TARGET_NAME}_extension)
set(LOADABLE_EXTENSION_NAME ${TARGET_NAME}_loadable_extension)

project(${TARGET_NAME} CXX)

set(CMAKE_CXX_STANDARD "17" CACHE STRING "C++ standard to enforce")
set(CMAKE_CXX_STANDARD_REQUIRED ON)

# ─────────────────────────────────────────────────────────────────────────────
# Rust core (opendalfs-core) via Corrosion
#
# The storage core is the OpenDAL Rust crate, statically linked into this C++
# extension. Corrosion drives cargo from CMake and exposes the staticlib as a
# CMake target (named after the crate: `opendalfs-core`). See
# docs/duckdb-opendal-plan.md §7.
# ─────────────────────────────────────────────────────────────────────────────
include(FetchContent)
FetchContent_Declare(
    Corrosion
    GIT_REPOSITORY https://github.com/corrosion-rs/corrosion.git
    GIT_TAG v0.5.1
)
FetchContent_MakeAvailable(Corrosion)

# Propagate the target triple to cargo so cross-compilation (CI, wasm) lines up
# with the C++ toolchain. Boilerplate pattern shared by DuckDB Rust hybrids.
set(Rust_CARGO_TARGET "")
if("${OS_NAME}" STREQUAL "linux")
    if("${OS_ARCH}" STREQUAL "arm64")
        set(Rust_CARGO_TARGET "aarch64-unknown-linux-gnu")
    else()
        set(Rust_CARGO_TARGET "x86_64-unknown-linux-gnu")
    endif()
elseif("${OS_NAME}" STREQUAL "osx")
    if("${OSX_BUILD_ARCH}" STREQUAL "arm64")
        set(Rust_CARGO_TARGET "aarch64-apple-darwin")
    elseif("${OSX_BUILD_ARCH}" STREQUAL "x86_64")
        set(Rust_CARGO_TARGET "x86_64-apple-darwin")
    elseif("${OS_ARCH}" STREQUAL "arm64")
        set(Rust_CARGO_TARGET "aarch64-apple-darwin")
    endif()
elseif(WIN32)
    if(MSVC AND "${OS_ARCH}" STREQUAL "arm64")
        set(Rust_CARGO_TARGET "aarch64-pc-windows-msvc")
    elseif(MSVC)
        set(Rust_CARGO_TARGET "x86_64-pc-windows-msvc")
    endif()
endif()

# Fallback: detect host target from rustc when CI variables are not set.
if("${Rust_CARGO_TARGET}" STREQUAL "")
    execute_process(
        COMMAND rustc --version --verbose
        OUTPUT_VARIABLE _RUSTC_VERBOSE
        OUTPUT_STRIP_TRAILING_WHITESPACE)
    if(_RUSTC_VERBOSE MATCHES "host: ([a-zA-Z0-9_-]+)")
        set(Rust_CARGO_TARGET "${CMAKE_MATCH_1}")
    endif()
endif()
message(STATUS "opendal_fs: Rust_CARGO_TARGET = ${Rust_CARGO_TARGET}")

corrosion_import_crate(
    MANIFEST_PATH ${CMAKE_CURRENT_SOURCE_DIR}/Cargo.toml
    CRATES opendalfs-core
    PROFILE release)

# ─────────────────────────────────────────────────────────────────────────────
# Extension
# ─────────────────────────────────────────────────────────────────────────────
include_directories(src/include)

set(EXTENSION_SOURCES
    src/opendal_fs_extension.cpp
    src/opendal_filesystem.cpp
    src/opendal_table_functions.cpp)

build_static_extension(${TARGET_NAME} ${EXTENSION_SOURCES})
build_loadable_extension(${TARGET_NAME} " " ${EXTENSION_SOURCES})

# System libraries the Rust staticlib depends on (rustls/tokio/reqwest).
if(APPLE)
    set(RUST_SYSTEM_LIBS "-framework Security" "-framework CoreFoundation" "-framework SystemConfiguration")
elseif(UNIX)
    set(RUST_SYSTEM_LIBS pthread dl m)
elseif(WIN32)
    set(RUST_SYSTEM_LIBS ntdll bcrypt userenv ws2_32)
endif()

# Link the Rust core into both the static and loadable extension.
#
# NOTE: DuckDB's build_*_extension macros define the extension targets in a
# CMake scope where Corrosion's IMPORTED targets (`opendalfs-core`,
# `opendalfs-core-static`) are not visible as targets, so linking them by name
# degrades to `-l<name>` and fails. To be deterministic across scopes we link
# the absolute path of the staticlib that Corrosion produces, and add an
# explicit dependency on the Corrosion build target for correct ordering.
set(RUST_STATICLIB "${CMAKE_BINARY_DIR}/extension/opendal_fs/libopendalfs_core.a")

add_dependencies(${EXTENSION_NAME} cargo-build_opendalfs_core)
add_dependencies(${LOADABLE_EXTENSION_NAME} cargo-build_opendalfs_core)

target_link_libraries(${EXTENSION_NAME} ${RUST_STATICLIB} ${RUST_SYSTEM_LIBS})
target_link_libraries(${LOADABLE_EXTENSION_NAME} ${RUST_STATICLIB} ${RUST_SYSTEM_LIBS})

install(
  TARGETS ${EXTENSION_NAME}
  EXPORT "${DUCKDB_EXPORT_SET}"
  LIBRARY DESTINATION "${INSTALL_LIB_DIR}"
  ARCHIVE DESTINATION "${INSTALL_LIB_DIR}")
