#include "opendal_secret.hpp"

#include "duckdb/main/database.hpp"
#include "duckdb/main/secret/secret.hpp"
#include "duckdb/main/secret/secret_manager.hpp"
#include "duckdb/common/types/value.hpp"

#include <vector>

namespace duckdb {

// Convenience VARCHAR params → OpenDAL config keys. These give an ergonomic UX
// (matching common cloud-secret conventions) on top of the generic `config`
// passthrough. Stored in secret_map under their OpenDAL key.
struct ConvenienceParam {
	const char *param; // CREATE SECRET named parameter
	const char *odal;  // OpenDAL config key
};
static const ConvenienceParam kConvenience[] = {
    {"key_id", "access_key_id"},      {"secret", "secret_access_key"},
    {"session_token", "session_token"}, {"region", "region"},
    {"endpoint", "endpoint"},
};

// The single, generic CREATE SECRET builder shared by every OpenDAL service
// type. It copies all provided options into the KeyValueSecret's secret_map:
//   - convenience params are remapped to their OpenDAL config key;
//   - the `config` MAP is flattened into individual secret_map entries;
//   - the `layers` MAP is flattened under a "layer." prefix;
//   - the service type is recorded under "__scheme".
// OperatorFor() reads all of this back when constructing the Operator.
static unique_ptr<BaseSecret> CreateOpenDalSecret(ClientContext &, CreateSecretInput &input) {
	auto secret = make_uniq<KeyValueSecret>(input.scope, input.type, input.provider, input.name);

	// Record the service scheme so the reader knows which OpenDAL service to use.
	secret->secret_map["__scheme"] = Value(input.type);

	// Convenience params → OpenDAL keys.
	for (auto &cp : kConvenience) {
		auto it = input.options.find(cp.param);
		if (it != input.options.end()) {
			secret->secret_map[cp.odal] = it->second;
		}
	}

	// Generic `config` MAP → individual config entries (raw passthrough).
	auto cfg_it = input.options.find("config");
	if (cfg_it != input.options.end() && !cfg_it->second.IsNull()) {
		auto &children = ListValue::GetChildren(cfg_it->second);
		for (auto &entry : children) {
			auto &kv = StructValue::GetChildren(entry);
			if (kv.size() == 2) {
				secret->secret_map[kv[0].ToString()] = Value(kv[1].ToString());
			}
		}
	}

	// `layers` MAP → "layer.<key>" entries.
	auto lay_it = input.options.find("layers");
	if (lay_it != input.options.end() && !lay_it->second.IsNull()) {
		auto &children = ListValue::GetChildren(lay_it->second);
		for (auto &entry : children) {
			auto &kv = StructValue::GetChildren(entry);
			if (kv.size() == 2) {
				secret->secret_map["layer." + kv[0].ToString()] = Value(kv[1].ToString());
			}
		}
	}

	return std::move(secret);
}

// Register a generic secret function + type for one service scheme.
static void RegisterOneService(ExtensionLoader &loader, const std::string &scheme) {
	// Secret type. Guard against collision: a native/core extension (e.g.
	// httpfs) may already register a secret type with the same name (e.g. "s3").
	// RegisterSecretType throws if the type already exists, so ignore that.
	SecretType type;
	type.name = scheme;
	type.deserializer = KeyValueSecret::Deserialize<KeyValueSecret>;
	type.default_provider = "config";
	try {
		loader.RegisterSecretType(type);
	} catch (...) {
		// Type already registered by another extension — fine; we still register
		// our create function below with REPLACE_ON_CONFLICT.
	}

	// Create function.
	CreateSecretFunction fn;
	fn.secret_type = scheme;
	fn.provider = "config";
	fn.function = CreateOpenDalSecret;

	// Generic config passthrough + layer options.
	fn.named_parameters["config"] = LogicalType::MAP(LogicalType::VARCHAR, LogicalType::VARCHAR);
	fn.named_parameters["layers"] = LogicalType::MAP(LogicalType::VARCHAR, LogicalType::VARCHAR);
	// Convenience params.
	for (auto &cp : kConvenience) {
		fn.named_parameters[cp.param] = LogicalType::VARCHAR;
	}

	loader.GetDatabaseInstance().GetSecretManager().RegisterSecretFunction(
	    std::move(fn), OnCreateConflict::REPLACE_ON_CONFLICT);
}

void RegisterOpenDalSecrets(ExtensionLoader &loader) {
	// Object-store / remote services that use secrets. (fs/memory need none.)
	// Expanded as more services are enabled in later phases.
	for (const char *scheme : {"s3"}) {
		RegisterOneService(loader, scheme);
	}
}

} // namespace duckdb
