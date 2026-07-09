#pragma once

#include "duckdb/main/extension/extension_loader.hpp"

namespace duckdb {

// Register CREATE SECRET support for the OpenDAL service schemes.
//
// One generic secret type is registered per service (TYPE s3, TYPE gcs, …).
// Each accepts:
//   - a generic `config` MAP(VARCHAR, VARCHAR) — arbitrary OpenDAL config keys
//     passed straight through (this is what makes the registration generic; no
//     per-service key allowlist / codegen is required);
//   - convenience VARCHAR params that mirror common cloud settings
//     (key_id, secret, session_token, region, endpoint);
//   - layer options as a `layers` MAP(VARCHAR, VARCHAR) (retry.*, timeout.*,
//     concurrent_limit — see the Rust layers module);
//   - the native SCOPE clause, which binds the secret to a URL prefix.
//
// The builder copies every provided option into the KeyValueSecret's secret_map
// verbatim; OperatorFor() later reads that map back as the OpenDAL config.
//
// Example:
//   CREATE SECRET minio (
//       TYPE s3,
//       SCOPE 's3://warehouse',
//       key_id 'minioadmin', secret 'minioadmin',
//       region 'us-east-1', endpoint 'http://127.0.0.1:19100'
//   );
void RegisterOpenDalSecrets(ExtensionLoader &loader);

} // namespace duckdb
