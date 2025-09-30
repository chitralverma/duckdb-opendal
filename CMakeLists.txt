cmake_minimum_required(VERSION 3.5)
set(CMAKE_EXPORT_COMPILE_COMMANDS ON)

# Set extension name here
set(TARGET_NAME opendalfs)
set(EXTENSION_NAME ${TARGET_NAME}_extension)
set(LOADABLE_EXTENSION_NAME ${TARGET_NAME}_loadable_extension)

project(${TARGET_NAME})
include_directories(src/include)

set(EXTENSION_SOURCES
    src/opendalfs_extension.cpp
    src/opendalfs_filesystem.cpp
)

# --- Add OpenDAL C++ binding subdirectory ---
add_subdirectory(third_party/opendal/bindings/cpp)

# --- Build DuckDB static extension target ---
build_static_extension(${TARGET_NAME} ${EXTENSION_SOURCES})
set_property(TARGET ${EXTENSION_NAME} PROPERTY CXX_STANDARD 17)

# --- Build DuckDB loadable extension target ---
build_loadable_extension(${TARGET_NAME} "" ${EXTENSION_SOURCES})
set_property(TARGET ${LOADABLE_EXTENSION_NAME} PROPERTY CXX_STANDARD 17)

# Link OpenDAL (C++ + Rust core) and CoreFoundation on macOS
if(APPLE)
  find_library(COREFOUNDATION_FRAMEWORK CoreFoundation)
  target_link_libraries(${EXTENSION_NAME} opendal_cpp
                        ${COREFOUNDATION_FRAMEWORK})
  target_link_libraries(${LOADABLE_EXTENSION_NAME} opendal_cpp
                        ${COREFOUNDATION_FRAMEWORK})
else()
  target_link_libraries(${EXTENSION_NAME} opendal_cpp)
  target_link_libraries(${LOADABLE_EXTENSION_NAME} opendal_cpp)
endif()

# --- Optional test executable ---
# add_executable(test_opendal src/test.cpp) target_link_libraries(test_opendal
# PRIVATE duckdb_static opendal_cpp) set_property(TARGET test_opendal PROPERTY
# CXX_STANDARD 17)

# --- Install and export DuckDB extension targets ---
install(
  TARGETS ${EXTENSION_NAME} ${LOADABLE_EXTENSION_NAME} opendal_cpp
  EXPORT "${DUCKDB_EXPORT_SET}"
  LIBRARY DESTINATION "${INSTALL_LIB_DIR}"
  ARCHIVE DESTINATION "${INSTALL_LIB_DIR}"
  RUNTIME DESTINATION "${INSTALL_LIB_DIR}"
  INCLUDES
  DESTINATION "${INSTALL_INCLUDE_DIR}")

# --- Export set for CMake config ---
install(
  EXPORT "${DUCKDB_EXPORT_SET}"
  FILE "${TARGET_NAME}Targets.cmake"
  NAMESPACE "${TARGET_NAME}::"
  DESTINATION "lib/cmake/${TARGET_NAME}")
