#include "opendal_table_functions.hpp"

#include "opendal_filesystem.hpp"

#include "duckdb/common/exception.hpp"
#include "duckdb/common/types/timestamp.hpp"

#include "rust.h"

#include <map>
#include <string>

namespace duckdb {

static void ClearError(OdError &err) {
	if (err.message) {
		od_string_free(err.message);
		err.message = nullptr;
	}
}

static void ThrowIfError(OdError &err, const std::string &operation, const std::string &target) {
	if (err.code == OdErrorCode::Ok) {
		return;
	}
	std::string message = "opendal " + operation + ": '" + target + "'";
	if (err.message) {
		message += ": " + std::string(err.message);
		ClearError(err);
	}
	throw IOException(message);
}

static void FreeCursor(OdTableCursor *cursor) {
	od_table_cursor_free(cursor);
}

struct OpenDalTableInfo : public TableFunctionInfo {
	explicit OpenDalTableInfo(OpenDalFileSystem *fs_p) : fs(fs_p) {
	}
	OpenDalFileSystem *fs;
};

enum class EntryScanKind : uint8_t { STAT, LIST, GLOB };

struct EntryBindData : public TableFunctionData {
	OpenDalFileSystem *fs;
	EntryScanKind kind;
	std::string target;
	std::string version;
	std::string if_match;
	std::string if_none_match;
	std::string start_after;
	int64_t if_modified_since_ms = 0;
	int64_t if_unmodified_since_ms = 0;
	idx_t limit = 0;
	bool has_if_modified_since = false;
	bool has_if_unmodified_since = false;
	bool has_limit = false;
	bool recursive = false;
	bool versions = false;
	bool deleted = false;
};

struct EntryGlobalState : public GlobalTableFunctionState {
	std::unique_ptr<OdTableCursor, void (*)(OdTableCursor *)> cursor {nullptr, FreeCursor};
	std::string target;
	bool finished = false;
};

static LogicalType MetadataType() {
	child_list_t<LogicalType> children = {
	    {"mode", LogicalType::VARCHAR},
	    {"content_length", LogicalType::UBIGINT},
	    {"cache_control", LogicalType::VARCHAR},
	    {"content_disposition", LogicalType::VARCHAR},
	    {"content_md5", LogicalType::VARCHAR},
	    {"content_type", LogicalType::VARCHAR},
	    {"content_encoding", LogicalType::VARCHAR},
	    {"etag", LogicalType::VARCHAR},
	    {"last_modified", LogicalType::TIMESTAMP},
	    {"version", LogicalType::VARCHAR},
	    {"is_current", LogicalType::BOOLEAN},
	    {"is_deleted", LogicalType::BOOLEAN},
	    {"user_metadata", LogicalType::MAP(LogicalType::VARCHAR, LogicalType::VARCHAR)},
	};
	return LogicalType::STRUCT(children);
}

static void DefineEntryColumns(vector<LogicalType> &types, vector<string> &names) {
	names = {"path", "name", "metadata"};
	types = {LogicalType::VARCHAR, LogicalType::VARCHAR, MetadataType()};
}

static std::string StringParameter(const named_parameter_map_t &parameters, const std::string &name) {
	auto entry = parameters.find(name);
	return entry == parameters.end() || entry->second.IsNull() ? std::string() : entry->second.ToString();
}

static bool BoolParameter(const named_parameter_map_t &parameters, const std::string &name) {
	auto entry = parameters.find(name);
	return entry != parameters.end() && !entry->second.IsNull() && BooleanValue::Get(entry->second);
}

static void ReadCommonOptions(TableFunctionBindInput &input, EntryBindData &result) {
	result.version = StringParameter(input.named_parameters, "version");
	result.if_match = StringParameter(input.named_parameters, "if_match");
	result.if_none_match = StringParameter(input.named_parameters, "if_none_match");
	auto modified = input.named_parameters.find("if_modified_since");
	if (modified != input.named_parameters.end() && !modified->second.IsNull()) {
		result.has_if_modified_since = true;
		result.if_modified_since_ms = Timestamp::GetEpochMs(TimestampValue::Get(modified->second));
	}
	auto unmodified = input.named_parameters.find("if_unmodified_since");
	if (unmodified != input.named_parameters.end() && !unmodified->second.IsNull()) {
		result.has_if_unmodified_since = true;
		result.if_unmodified_since_ms = Timestamp::GetEpochMs(TimestampValue::Get(unmodified->second));
	}
}

static unique_ptr<FunctionData> BindEntry(EntryScanKind kind, TableFunctionBindInput &input,
                                          vector<LogicalType> &return_types, vector<string> &names) {
	auto result = make_uniq<EntryBindData>();
	result->fs = input.info->Cast<OpenDalTableInfo>().fs;
	result->kind = kind;
	result->target = input.inputs[0].GetValue<string>();
	if (kind == EntryScanKind::STAT) {
		ReadCommonOptions(input, *result);
	} else {
		auto limit = input.named_parameters.find("limit");
		if (limit != input.named_parameters.end() && !limit->second.IsNull()) {
			result->has_limit = true;
			result->limit = limit->second.GetValue<idx_t>();
			if (result->limit == 0) {
				throw InvalidInputException("opendal list: limit must be positive");
			}
		}
		result->start_after = StringParameter(input.named_parameters, "start_after");
		result->recursive = BoolParameter(input.named_parameters, "recursive");
		result->versions = BoolParameter(input.named_parameters, "versions");
		result->deleted = BoolParameter(input.named_parameters, "deleted");
	}
	DefineEntryColumns(return_types, names);
	return std::move(result);
}

static unique_ptr<FunctionData> StatBind(ClientContext &, TableFunctionBindInput &input,
                                         vector<LogicalType> &return_types, vector<string> &names) {
	return BindEntry(EntryScanKind::STAT, input, return_types, names);
}

static unique_ptr<FunctionData> ListBind(ClientContext &, TableFunctionBindInput &input,
                                         vector<LogicalType> &return_types, vector<string> &names) {
	return BindEntry(EntryScanKind::LIST, input, return_types, names);
}

static unique_ptr<FunctionData> GlobBind(ClientContext &, TableFunctionBindInput &input,
                                         vector<LogicalType> &return_types, vector<string> &names) {
	return BindEntry(EntryScanKind::GLOB, input, return_types, names);
}

static unique_ptr<GlobalTableFunctionState> EntryInit(ClientContext &context, TableFunctionInitInput &input) {
	auto &bind = input.bind_data->Cast<EntryBindData>();
	if (!bind.fs) {
		throw InvalidInputException("opendal table scan: filesystem not available");
	}
	std::string scheme, authority, path;
	if (!bind.fs->ParsePublic(bind.target, scheme, authority, path)) {
		throw InvalidInputException("opendal table scan: unsupported or invalid URL: " + bind.target);
	}
	auto *op = bind.fs->OperatorForPublic(scheme, authority, bind.target, &context);
	OdError err = {};
	OdTableCursor *cursor = nullptr;
	if (bind.kind == EntryScanKind::STAT) {
		OdStatOptions options = {};
		options.version = bind.version.empty() ? nullptr : bind.version.c_str();
		options.if_match = bind.if_match.empty() ? nullptr : bind.if_match.c_str();
		options.if_none_match = bind.if_none_match.empty() ? nullptr : bind.if_none_match.c_str();
		options.if_modified_since_ms = bind.if_modified_since_ms;
		options.has_if_modified_since = bind.has_if_modified_since;
		options.if_unmodified_since_ms = bind.if_unmodified_since_ms;
		options.has_if_unmodified_since = bind.has_if_unmodified_since;
		cursor = od_table_stat_open(op, path.c_str(), options, &err);
	} else {
		OdListOptions options = {};
		options.limit = bind.limit;
		options.has_limit = bind.has_limit;
		options.start_after = bind.start_after.empty() ? nullptr : bind.start_after.c_str();
		options.recursive = bind.recursive;
		options.versions = bind.versions;
		options.deleted = bind.deleted;
		cursor = bind.kind == EntryScanKind::GLOB ? od_table_glob_open(op, path.c_str(), options, &err)
		                                          : od_table_list_open(op, path.c_str(), nullptr, options, &err);
	}
	if (!cursor) {
		ThrowIfError(err,
		             bind.kind == EntryScanKind::STAT   ? "stat"
		             : bind.kind == EntryScanKind::GLOB ? "glob"
		                                                : "list",
		             bind.target);
		throw IOException("opendal table scan: null cursor for '" + bind.target + "'");
	}
	ClearError(err);
	auto state = make_uniq<EntryGlobalState>();
	state->cursor.reset(cursor);
	state->target = bind.target;
	return std::move(state);
}

static Value NullableString(const char *value) {
	return value ? Value(value) : Value(LogicalType::VARCHAR);
}

static Value MetadataValue(const OdEntryMetadata &metadata) {
	vector<Value> keys;
	vector<Value> values;
	for (idx_t index = 0; index < metadata.user_metadata_len; index++) {
		keys.emplace_back(metadata.user_metadata_keys[index]);
		values.emplace_back(metadata.user_metadata_values[index]);
	}
	child_list_t<Value> children = {
	    {"mode", Value(metadata.mode == 1   ? "file"
	                   : metadata.mode == 2 ? "directory"
	                                        : "unknown")},
	    {"content_length", Value::UBIGINT(metadata.content_length)},
	    {"cache_control", NullableString(metadata.cache_control)},
	    {"content_disposition", NullableString(metadata.content_disposition)},
	    {"content_md5", NullableString(metadata.content_md5)},
	    {"content_type", NullableString(metadata.content_type)},
	    {"content_encoding", NullableString(metadata.content_encoding)},
	    {"etag", NullableString(metadata.etag)},
	    {"last_modified", metadata.has_last_modified
	                          ? Value::TIMESTAMP(Timestamp::FromEpochMs(metadata.last_modified_ms))
	                          : Value(LogicalType::TIMESTAMP)},
	    {"version", NullableString(metadata.version)},
	    {"is_current",
	     metadata.has_is_current ? Value::BOOLEAN(metadata.is_current != 0) : Value(LogicalType::BOOLEAN)},
	    {"is_deleted", Value::BOOLEAN(metadata.is_deleted != 0)},
	    {"user_metadata", Value::MAP(LogicalType::VARCHAR, LogicalType::VARCHAR, std::move(keys), std::move(values))},
	};
	return Value::STRUCT(std::move(children));
}

static void EntryFunc(ClientContext &, TableFunctionInput &data, DataChunk &output) {
	auto &state = data.global_state->Cast<EntryGlobalState>();
	if (state.finished) {
		output.SetCardinality(0);
		return;
	}
	idx_t count = 0;
	while (count < STANDARD_VECTOR_SIZE) {
		OdEntryRow row = {};
		OdError err = {};
		int8_t result = od_table_cursor_next(state.cursor.get(), &row, &err);
		if (result < 0) {
			ThrowIfError(err, "scan", state.target);
		}
		ClearError(err);
		if (result == 0) {
			state.finished = true;
			break;
		}
		output.SetValue(0, count, Value(row.path));
		output.SetValue(1, count, Value(row.name));
		output.SetValue(2, count, MetadataValue(row.metadata));
		count++;
	}
	output.SetCardinality(count);
}

struct DuBindData : public TableFunctionData {
	OpenDalFileSystem *fs;
	std::string url;
	idx_t limit = 0;
	std::string start_after;
	bool has_limit = false;
	bool versions = false;
	bool deleted = false;
};

struct DuGlobalState : public GlobalTableFunctionState {
	std::unique_ptr<OdDuCursor, void (*)(OdDuCursor *)> cursor {nullptr, od_du_cursor_free};
	std::string scheme;
	std::string authority;
	std::string target;
	bool finished = false;
};

static unique_ptr<GlobalTableFunctionState> DuInit(ClientContext &context, TableFunctionInitInput &input) {
	auto &bind = input.bind_data->Cast<DuBindData>();
	std::string scheme, authority, path;
	if (!bind.fs || !bind.fs->ParsePublic(bind.url, scheme, authority, path)) {
		throw InvalidInputException("opendal du: unsupported or invalid URL: " + bind.url);
	}
	auto *op = bind.fs->OperatorForPublic(scheme, authority, bind.url, &context);
	OdListOptions options = {};
	options.limit = bind.limit;
	options.has_limit = bind.has_limit;
	options.start_after = bind.start_after.empty() ? nullptr : bind.start_after.c_str();
	options.versions = bind.versions;
	options.deleted = bind.deleted;
	OdError err = {};
	auto *cursor = od_table_du_open(op, path.c_str(), options, &err);
	if (!cursor) {
		ThrowIfError(err, "du", bind.url);
		throw IOException("opendal du: null cursor for '" + bind.url + "'");
	}
	ClearError(err);
	auto state = make_uniq<DuGlobalState>();
	state->cursor.reset(cursor);
	state->scheme = scheme;
	state->authority = authority;
	state->target = bind.url;
	return std::move(state);
}

static unique_ptr<FunctionData> DuBind(ClientContext &, TableFunctionBindInput &input, vector<LogicalType> &types,
                                       vector<string> &names) {
	auto result = make_uniq<DuBindData>();
	result->fs = input.info->Cast<OpenDalTableInfo>().fs;
	result->url = input.inputs[0].GetValue<string>();
	auto limit = input.named_parameters.find("limit");
	if (limit != input.named_parameters.end() && !limit->second.IsNull()) {
		result->has_limit = true;
		result->limit = limit->second.GetValue<idx_t>();
		if (result->limit == 0) {
			throw InvalidInputException("opendal du: limit must be positive");
		}
	}
	result->start_after = StringParameter(input.named_parameters, "start_after");
	result->versions = BoolParameter(input.named_parameters, "versions");
	result->deleted = BoolParameter(input.named_parameters, "deleted");
	names = {"directory", "file_count", "total_size"};
	types = {LogicalType::VARCHAR, LogicalType::UBIGINT, LogicalType::UBIGINT};
	return std::move(result);
}

static void DuFunc(ClientContext &, TableFunctionInput &data, DataChunk &output) {
	auto &state = data.global_state->Cast<DuGlobalState>();
	if (state.finished) {
		output.SetCardinality(0);
		return;
	}
	idx_t count = 0;
	while (count < STANDARD_VECTOR_SIZE) {
		OdDuRow row = {};
		OdError err = {};
		auto result = od_du_cursor_next(state.cursor.get(), &row, &err);
		if (result < 0) {
			ThrowIfError(err, "du", state.target);
		}
		ClearError(err);
		if (result == 0) {
			state.finished = true;
			break;
		}
		output.SetValue(0, count,
		                Value(OpenDalFileSystem::BuildUrlPublic(state.scheme, state.authority, row.directory)));
		output.SetValue(1, count, Value::UBIGINT(row.file_count));
		output.SetValue(2, count, Value::UBIGINT(row.total_size));
		count++;
	}
	output.SetCardinality(count);
}

struct CopyBindData : public TableFunctionData {
	OpenDalFileSystem *fs;
	std::string source;
	std::string destination;
	std::string if_match;
	std::string source_version;
	idx_t source_content_length_hint = 0;
	idx_t concurrent = 0;
	idx_t chunk_size = 0;
	bool if_not_exists = false;
	bool has_source_content_length_hint = false;
	bool has_concurrent = false;
	bool has_chunk_size = false;
};

struct CopyGlobalState : public GlobalTableFunctionState {
	bool finished = false;
};

static unique_ptr<FunctionData> CopyBind(ClientContext &, TableFunctionBindInput &input, vector<LogicalType> &types,
                                         vector<string> &names) {
	auto result = make_uniq<CopyBindData>();
	result->fs = input.info->Cast<OpenDalTableInfo>().fs;
	result->source = input.inputs[0].GetValue<string>();
	result->destination = input.inputs[1].GetValue<string>();
	result->if_not_exists = BoolParameter(input.named_parameters, "if_not_exists");
	result->if_match = StringParameter(input.named_parameters, "if_match");
	result->source_version = StringParameter(input.named_parameters, "source_version");
	auto read_size = [&](const std::string &name, idx_t &value, bool &present, bool allow_zero) {
		auto entry = input.named_parameters.find(name);
		if (entry == input.named_parameters.end() || entry->second.IsNull()) {
			return;
		}
		present = true;
		value = entry->second.GetValue<idx_t>();
		if (value == 0 && !allow_zero) {
			throw InvalidInputException("opendal copy: " + name + " must be positive");
		}
	};
	read_size("source_content_length_hint", result->source_content_length_hint, result->has_source_content_length_hint,
	          true);
	read_size("concurrent", result->concurrent, result->has_concurrent, false);
	read_size("chunk_size", result->chunk_size, result->has_chunk_size, false);
	names = {"source", "destination", "bytes_copied", "metadata"};
	types = {LogicalType::VARCHAR, LogicalType::VARCHAR, LogicalType::UBIGINT, MetadataType()};
	return std::move(result);
}

static unique_ptr<GlobalTableFunctionState> CopyInit(ClientContext &context, TableFunctionInitInput &input) {
	return make_uniq<CopyGlobalState>();
}

static void CopyFunc(ClientContext &context, TableFunctionInput &data, DataChunk &output) {
	auto &state = data.global_state->Cast<CopyGlobalState>();
	if (state.finished) {
		output.SetCardinality(0);
		return;
	}
	auto &bind = data.bind_data->Cast<CopyBindData>();
	if (!bind.fs) {
		throw InvalidInputException("opendal copy: filesystem not available");
	}
	std::string source_scheme, source_authority, source_path;
	std::string destination_scheme, destination_authority, destination_path;
	if (!bind.fs->ParsePublic(bind.source, source_scheme, source_authority, source_path)) {
		throw InvalidInputException("opendal copy: unsupported or invalid source URL: " + bind.source);
	}
	if (!bind.fs->ParsePublic(bind.destination, destination_scheme, destination_authority, destination_path)) {
		throw InvalidInputException("opendal copy: unsupported or invalid destination URL: " + bind.destination);
	}
	auto *source_op = bind.fs->OperatorForPublic(source_scheme, source_authority, bind.source, &context);
	auto *destination_op =
	    bind.fs->OperatorForPublic(destination_scheme, destination_authority, bind.destination, &context);
	OdCopyOptions options = {};
	options.if_not_exists = bind.if_not_exists;
	options.if_match = bind.if_match.empty() ? nullptr : bind.if_match.c_str();
	options.source_version = bind.source_version.empty() ? nullptr : bind.source_version.c_str();
	options.source_content_length_hint = bind.source_content_length_hint;
	options.has_source_content_length_hint = bind.has_source_content_length_hint;
	options.concurrent = bind.concurrent;
	options.has_concurrent = bind.has_concurrent;
	options.chunk_size = bind.chunk_size;
	options.has_chunk_size = bind.has_chunk_size;
	OdError err = {};
	std::unique_ptr<OdCopyCursor, void (*)(OdCopyCursor *)> cursor(
	    od_table_copy_open(source_op, source_path.c_str(), destination_op, destination_path.c_str(), options, &err),
	    od_copy_cursor_free);
	if (!cursor) {
		ThrowIfError(err, "copy", bind.source + "' -> '" + bind.destination);
		throw IOException("opendal copy: null cursor");
	}
	ClearError(err);
	OdCopyRow row = {};
	auto result = od_copy_cursor_next(cursor.get(), &row, &err);
	if (result < 0) {
		ThrowIfError(err, "copy", bind.source + "' -> '" + bind.destination);
	}
	ClearError(err);
	state.finished = true;
	if (result == 0) {
		output.SetCardinality(0);
		return;
	}
	output.SetValue(0, 0, Value(bind.source));
	output.SetValue(1, 0, Value(bind.destination));
	output.SetValue(2, 0, Value::UBIGINT(row.bytes_copied));
	output.SetValue(3, 0, MetadataValue(row.metadata));
	output.SetCardinality(1);
}

static void AddStatOptions(TableFunction &function) {
	function.named_parameters["version"] = LogicalType::VARCHAR;
	function.named_parameters["if_match"] = LogicalType::VARCHAR;
	function.named_parameters["if_none_match"] = LogicalType::VARCHAR;
	function.named_parameters["if_modified_since"] = LogicalType::TIMESTAMP;
	function.named_parameters["if_unmodified_since"] = LogicalType::TIMESTAMP;
}

static void AddListOptions(TableFunction &function) {
	function.named_parameters["limit"] = LogicalType::UBIGINT;
	function.named_parameters["start_after"] = LogicalType::VARCHAR;
	function.named_parameters["recursive"] = LogicalType::BOOLEAN;
	function.named_parameters["versions"] = LogicalType::BOOLEAN;
	function.named_parameters["deleted"] = LogicalType::BOOLEAN;
}

void RegisterOpenDalTableFunctions(ExtensionLoader &loader, OpenDalFileSystem *fs) {
	auto info = make_shared_ptr<OpenDalTableInfo>(fs);
	TableFunction stat("opendal_stat", {LogicalType::VARCHAR}, EntryFunc, StatBind, EntryInit);
	AddStatOptions(stat);
	stat.function_info = info;
	loader.RegisterFunction(stat);

	TableFunction list("opendal_ls", {LogicalType::VARCHAR}, EntryFunc, ListBind, EntryInit);
	AddListOptions(list);
	list.function_info = info;
	loader.RegisterFunction(list);

	TableFunction glob("opendal_glob", {LogicalType::VARCHAR}, EntryFunc, GlobBind, EntryInit);
	AddListOptions(glob);
	glob.function_info = info;
	loader.RegisterFunction(glob);

	TableFunction du("opendal_du", {LogicalType::VARCHAR}, DuFunc, DuBind, DuInit);
	du.named_parameters["limit"] = LogicalType::UBIGINT;
	du.named_parameters["start_after"] = LogicalType::VARCHAR;
	du.named_parameters["versions"] = LogicalType::BOOLEAN;
	du.named_parameters["deleted"] = LogicalType::BOOLEAN;
	du.function_info = info;
	loader.RegisterFunction(du);

	TableFunction copy("opendal_copy", {LogicalType::VARCHAR, LogicalType::VARCHAR}, CopyFunc, CopyBind, CopyInit);
	copy.named_parameters["if_not_exists"] = LogicalType::BOOLEAN;
	copy.named_parameters["if_match"] = LogicalType::VARCHAR;
	copy.named_parameters["source_version"] = LogicalType::VARCHAR;
	copy.named_parameters["source_content_length_hint"] = LogicalType::UBIGINT;
	copy.named_parameters["concurrent"] = LogicalType::UBIGINT;
	copy.named_parameters["chunk_size"] = LogicalType::UBIGINT;
	copy.function_info = info;
	loader.RegisterFunction(copy);
}

} // namespace duckdb
