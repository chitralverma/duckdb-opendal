#include "opendal_secret.hpp"

#include "duckdb/common/exception.hpp"
#include "duckdb/main/database.hpp"
#include "duckdb/main/secret/secret.hpp"
#include "duckdb/main/secret/secret_manager.hpp"
#include "duckdb/common/types/value.hpp"

#include <vector>

namespace duckdb {

static void CopyMap(CreateSecretInput &input, KeyValueSecret &secret, const char *name, const char *prefix) {
	auto it = input.options.find(name);
	if (it == input.options.end() || it->second.IsNull()) {
		return;
	}
	for (auto &entry : ListValue::GetChildren(it->second)) {
		auto &kv = StructValue::GetChildren(entry);
		if (kv.size() == 2) {
			secret.secret_map[std::string(prefix) + kv[0].ToString()] = Value(kv[1].ToString());
		}
	}
}

static unique_ptr<BaseSecret> CreateOpenDalSecret(ClientContext &, CreateSecretInput &input) {
	auto secret = make_uniq<KeyValueSecret>(input.scope, input.type, input.provider, input.name);
	secret->secret_map["__scheme"] = Value(input.type);
	CopyMap(input, *secret, "config", "config.");
	CopyMap(input, *secret, "io_config", "io.");
	CopyMap(input, *secret, "retry_config", "retry.");
	CopyMap(input, *secret, "timeout_config", "timeout.");
	CopyMap(input, *secret, "cache_config", "cache.");
	return std::move(secret);
}

// Register a generic secret function + type for one service scheme.
static void RegisterOneService(ExtensionLoader &loader, const std::string &scheme) {
	// Secret type. Guard against collision: a native/core extension (e.g.
	// httpfs) may already register a secret type with the same name (e.g. "s3").
	// RegisterSecretType throws InternalException ("already registered secret
	// type") on duplicates; swallow only that and let any other error surface.
	SecretType type;
	type.name = scheme;
	type.deserializer = KeyValueSecret::Deserialize<KeyValueSecret>;
	type.default_provider = "config";
	try {
		loader.RegisterSecretType(type);
	} catch (const InternalException &e) {
		if (std::string(e.what()).find("already registered secret type") == std::string::npos) {
			throw;
		}
		// Type already registered by another extension — fine; we still register
		// our create function below with REPLACE_ON_CONFLICT.
	}

	// Create function.
	CreateSecretFunction fn;
	fn.secret_type = scheme;
	fn.provider = "config";
	fn.function = CreateOpenDalSecret;

	for (auto name : {"config", "io_config", "retry_config", "timeout_config", "cache_config"}) {
		fn.named_parameters[name] = LogicalType::MAP(LogicalType::VARCHAR, LogicalType::VARCHAR);
	}

	loader.GetDatabaseInstance().GetSecretManager().RegisterSecretFunction(std::move(fn),
	                                                                       OnCreateConflict::REPLACE_ON_CONFLICT);
}

void RegisterOpenDalSecrets(ExtensionLoader &loader) {
	// Object-store / remote services that use secrets. (fs/memory need none.)
	// Expanded as more services are enabled in later phases.
	for (const char *scheme : {"s3"}) {
		RegisterOneService(loader, scheme);
	}
}

} // namespace duckdb
