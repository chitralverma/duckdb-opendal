#pragma once

#include "duckdb/function/table_function.hpp"
#include "duckdb/main/extension/extension_loader.hpp"

namespace duckdb {

class OpenDalFileSystem;

// Register the ls(), stat() and du() table functions.
//
//   opendal_ls(url [, recursive := false])
//     Columns: path VARCHAR, name VARCHAR, type VARCHAR ('file'|'directory'),
//              size BIGINT, size_pretty VARCHAR, modified TIMESTAMP
//
//   opendal_stat(url)
//     Same columns as ls() but exactly one row for the given path.
//
//   opendal_du(url)
//     Recursive size rollup. Columns:
//       directory VARCHAR, file_count BIGINT, total_size BIGINT, size_pretty VARCHAR
//
// The functions operate through the OpenDAL Rust core via the given filesystem.
void RegisterOpenDalTableFunctions(ExtensionLoader &loader, OpenDalFileSystem *fs);

} // namespace duckdb
