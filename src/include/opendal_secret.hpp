#pragma once

#include "duckdb/main/extension/extension_loader.hpp"

namespace duckdb {

// Register CREATE SECRET support for the OpenDAL service schemes.
//
// One generic secret type is registered per service (TYPE s3, TYPE gcs, …).
// Each accepts service `config` plus optional `io_config`, `retry_config`,
// `timeout_config`, and `cache_config` maps. SCOPE binds them to a URL prefix.
//
// See docs/configuration.md for keys and global defaults.
void RegisterOpenDalSecrets(ExtensionLoader &loader);

} // namespace duckdb
