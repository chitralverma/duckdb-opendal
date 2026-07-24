#pragma once

#include "duckdb/function/table_function.hpp"
#include "duckdb/main/extension/extension_loader.hpp"

namespace duckdb {

class OpenDalFileSystem;

// Register the OpenDAL table functions (all operate through the Rust core):
//   opendal_stat(url)              — one row; entry (path, name, metadata).
//   opendal_ls(url)               — list a directory or match a glob pattern.
//   opendal_du(url)               — recursive size rollup by parent directory;
//                                   accepts a file, directory, or glob.
//   opendal_copy(src, dst)        — copy a file, directory tree, or glob match;
//                                   one row per copied object.
//   opendal_schemes()             — schemes compiled into this build.
//   opendal_info(url)             — support/secret/operator/capabilities of a URL.
void RegisterOpenDalTableFunctions(ExtensionLoader &loader, OpenDalFileSystem *fs);

} // namespace duckdb
