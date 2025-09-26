#define DUCKDB_EXTENSION_MAIN

#include "opendalfs_extension.hpp"
#include "duckdb.hpp"
#include "duckdb/common/exception.hpp"
#include "duckdb/function/scalar_function.hpp"
#include <duckdb/parser/parsed_data/create_scalar_function_info.hpp>

// OpenSSL linked through vcpkg
#include <openssl/opensslv.h>

namespace duckdb {

inline void OpendalfsScalarFun(DataChunk &args, ExpressionState &state, Vector &result) {
	auto &name_vector = args.data[0];
	UnaryExecutor::Execute<string_t, string_t>(name_vector, result, args.size(), [&](string_t name) {
		return StringVector::AddString(result, "Opendalfs " + name.GetString() + " 🐥");
	});
}

inline void OpendalfsOpenSSLVersionScalarFun(DataChunk &args, ExpressionState &state, Vector &result) {
	auto &name_vector = args.data[0];
	UnaryExecutor::Execute<string_t, string_t>(name_vector, result, args.size(), [&](string_t name) {
		return StringVector::AddString(result, "Opendalfs " + name.GetString() + ", my linked OpenSSL version is " +
		                                           OPENSSL_VERSION_TEXT);
	});
}

static void LoadInternal(ExtensionLoader &loader) {
	// Register a scalar function
	auto opendalfs_scalar_function = ScalarFunction("opendalfs", {LogicalType::VARCHAR}, LogicalType::VARCHAR, OpendalfsScalarFun);
	loader.RegisterFunction(opendalfs_scalar_function);

	// Register another scalar function
	auto opendalfs_openssl_version_scalar_function = ScalarFunction("opendalfs_openssl_version", {LogicalType::VARCHAR},
	                                                            LogicalType::VARCHAR, OpendalfsOpenSSLVersionScalarFun);
	loader.RegisterFunction(opendalfs_openssl_version_scalar_function);
}

void OpendalfsExtension::Load(ExtensionLoader &loader) {
	LoadInternal(loader);
}
std::string OpendalfsExtension::Name() {
	return "opendalfs";
}

std::string OpendalfsExtension::Version() const {
#ifdef EXT_VERSION_OPENDALFS
	return EXT_VERSION_OPENDALFS;
#else
	return "";
#endif
}

} // namespace duckdb

extern "C" {

DUCKDB_CPP_EXTENSION_ENTRY(opendalfs, loader) {
	duckdb::LoadInternal(loader);
}
}
