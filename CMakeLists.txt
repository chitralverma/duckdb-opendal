cmake_minimum_required(VERSION 3.22)

# Set extension name here
set(TARGET_NAME opendal)

set(EXTENSION_NAME ${TARGET_NAME}_extension)
set(LOADABLE_EXTENSION_NAME ${TARGET_NAME}_loadable_extension)

project(${TARGET_NAME} CXX)

set(CMAKE_CXX_STANDARD "17" CACHE STRING "C++ standard to enforce")
set(CMAKE_CXX_STANDARD_REQUIRED ON)

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
  if(MINGW)
    set(Rust_CARGO_TARGET "x86_64-pc-windows-gnu")
  elseif(MSVC AND "${OS_ARCH}" STREQUAL "arm64")
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

message(STATUS "Rust_CARGO_TARGET: ${Rust_CARGO_TARGET}")
message(STATUS "OS_NAME: ${OS_NAME}")
message(STATUS "OS_ARCH: ${OS_ARCH}")

# Detect WASM build for conditional compilation
if(DEFINED ENV{DUCKDB_WASM_PLATFORM} OR "${DUCKDB_EXPLICIT_PLATFORM}" MATCHES "^wasm" OR "${Rust_CARGO_TARGET}" STREQUAL "wasm32-unknown-emscripten")
  set(WASM_BUILD TRUE)
  message(STATUS "Building for WASM platform")
else()
  set(WASM_BUILD FALSE)
endif()

# DuckDB extension boilerplate
include_directories(src/include)

# Fetch Corrosion for Rust integration
include(FetchContent)
FetchContent_Declare(
    Corrosion
    GIT_REPOSITORY https://github.com/corrosion-rs/corrosion.git
    GIT_TAG v0.6.1
)
FetchContent_MakeAvailable(Corrosion)

# Import Rust FFI crate
corrosion_import_crate(
    MANIFEST_PATH ${CMAKE_CURRENT_SOURCE_DIR}/opendal/Cargo.toml
    CRATES duckdb-opendal
)

# The Rust target name (with underscores)
set(RUST_LIB duckdb_opendal)

# Set Rust build options for release
if(CMAKE_BUILD_TYPE STREQUAL "Release")
  corrosion_set_env_vars(${RUST_LIB} "CARGO_PROFILE_RELEASE_LTO=true")
endif()

if(WIN32 AND MSVC)
  # DuckDB links the MSVC runtime statically (/MT). Force Cargo/rustc to compile with target-feature=+crt-static to match.
  corrosion_set_env_vars(${RUST_LIB} "RUSTFLAGS=-C target-feature=+crt-static")
endif()

set(EXTENSION_SOURCES
    src/opendal_extension.cpp
    src/opendal_filesystem.cpp
    src/opendal_table_functions.cpp
    src/opendal_secret.cpp)

# Build the static extension
build_static_extension(${TARGET_NAME} ${EXTENSION_SOURCES})

# Build the loadable extension
build_loadable_extension(${TARGET_NAME} " " ${EXTENSION_SOURCES})

# System libraries the Rust staticlib depends on (rustls/tokio/reqwest).
if(APPLE)
  set(RUST_SYSTEM_LIBS "-framework Security" "-framework CoreFoundation" "-framework SystemConfiguration")
elseif(UNIX)
  set(RUST_SYSTEM_LIBS pthread dl m)
elseif(WIN32)
  set(RUST_SYSTEM_LIBS ntdll bcrypt userenv ws2_32)
endif()

# Link Rust library and platform-specific libs
if(WASM_BUILD)
  # WASM: minimal linking, no system libraries, no telemetry
  target_link_libraries(${TARGET_NAME}_extension ${RUST_LIB})
  target_link_libraries(${TARGET_NAME}_loadable_extension ${RUST_LIB})
elseif(UNIX AND NOT APPLE)
  target_link_libraries(${TARGET_NAME}_extension ${RUST_LIB} ${RUST_SYSTEM_LIBS})
  target_link_libraries(${TARGET_NAME}_loadable_extension ${RUST_LIB} ${RUST_SYSTEM_LIBS})
elseif(WIN32)
  target_link_libraries(${TARGET_NAME}_extension ${RUST_LIB} ${RUST_SYSTEM_LIBS})
  target_link_libraries(${TARGET_NAME}_loadable_extension ${RUST_LIB} ${RUST_SYSTEM_LIBS})
elseif(APPLE)
  target_link_libraries(${TARGET_NAME}_extension ${RUST_LIB} ${RUST_SYSTEM_LIBS})
  target_link_libraries(${TARGET_NAME}_loadable_extension ${RUST_LIB} ${RUST_SYSTEM_LIBS})
else()
  target_link_libraries(${TARGET_NAME}_extension ${RUST_LIB})
  target_link_libraries(${TARGET_NAME}_loadable_extension ${RUST_LIB})
endif()

# Add Rust library to DuckDB export set
install(TARGETS ${RUST_LIB}
    EXPORT "${DUCKDB_EXPORT_SET}"
    LIBRARY DESTINATION "${INSTALL_LIB_DIR}"
    ARCHIVE DESTINATION "${INSTALL_LIB_DIR}")


# Install extension
install(TARGETS ${TARGET_NAME}_extension
    EXPORT "${DUCKDB_EXPORT_SET}"
    LIBRARY DESTINATION "${INSTALL_LIB_DIR}"
    ARCHIVE DESTINATION "${INSTALL_LIB_DIR}")
