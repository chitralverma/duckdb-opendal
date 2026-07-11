#include "opendal_table_functions.hpp"

#include "opendal_filesystem.hpp"

#include "duckdb/common/types/timestamp.hpp"
#include "duckdb/common/exception.hpp"

#include "rust.h"

#include <algorithm>
#include <map>
#include <string>
#include <vector>

namespace duckdb {

// ── shared helpers ───────────────────────────────────────────────────────────

static std::string HumanSize(int64_t bytes) {
	static const char *units[] = {"B", "KiB", "MiB", "GiB", "TiB", "PiB"};
	double v = (double)bytes;
	int u = 0;
	while (v >= 1024.0 && u < 5) {
		v /= 1024.0;
		u++;
	}
	char buf[64];
	if (u == 0) {
		snprintf(buf, sizeof(buf), "%lld %s", (long long)bytes, units[u]);
	} else {
		snprintf(buf, sizeof(buf), "%.1f %s", v, units[u]);
	}
	return std::string(buf);
}

// A materialized row for ls()/stat().
struct FsRow {
	std::string url;
	std::string name;
	bool is_dir;
	int64_t size;
	int64_t modified_ms;
};

// Free an OdopError message if set.
static void ClearErr(OdopError &err) {
	if (err.message) {
		odop_string_free(err.message);
		err.message = nullptr;
	}
}

static std::string ErrText(OdopError &err) {
	std::string m = err.message ? std::string(err.message) : std::string("unknown error");
	ClearErr(err);
	return m;
}

// ── ls() / stat() ────────────────────────────────────────────────────────────

struct LsBindData : public TableFunctionData {
	OpenDalFileSystem *fs;
	std::string url;
	bool recursive = false;
	bool single = false; // stat() = exactly one row
};

struct LsGlobalState : public GlobalTableFunctionState {
	std::vector<FsRow> rows;
	idx_t offset = 0;
};

static void DefineLsColumns(vector<LogicalType> &types, vector<string> &names) {
	names = {"path", "name", "type", "size", "size_pretty", "modified"};
	types = {LogicalType::VARCHAR, LogicalType::VARCHAR, LogicalType::VARCHAR,
	         LogicalType::BIGINT,  LogicalType::VARCHAR, LogicalType::TIMESTAMP};
}

// We stash the filesystem pointer in the TableFunction's function_info.
struct OpenDalTableInfo : public TableFunctionInfo {
	explicit OpenDalTableInfo(OpenDalFileSystem *fs_) : fs(fs_) {
	}
	OpenDalFileSystem *fs;
};

static unique_ptr<GlobalTableFunctionState> LsInit(ClientContext &context, TableFunctionInitInput &input) {
	auto &bind = input.bind_data->Cast<LsBindData>();
	auto state = make_uniq<LsGlobalState>();

	auto *fs = bind.fs;
	if (!fs) {
		throw InvalidInputException("opendal ls/stat: filesystem not available");
	}

	std::string scheme, auth, rel;
	if (!fs->ParsePublic(bind.url, scheme, auth, rel)) {
		throw InvalidInputException("opendal: unsupported or invalid URL: " + bind.url);
	}
	OdopOperator *op = fs->OperatorForPublic(scheme, auth, bind.url, &context);

	if (bind.single) {
		OdopMetadata meta = {};
		OdopError err = {};
		odop_stat(op, rel.c_str(), &meta, &err);
		if (err.code != OdopErrorCode::Ok) {
			throw IOException("opendal stat: " + bind.url + ": " + ErrText(err));
		}
		ClearErr(err);
		// Derive a display name from the URL.
		std::string name = rel;
		auto slash = name.find_last_of('/');
		if (slash != std::string::npos) {
			name = name.substr(slash + 1);
		}
		FsRow row;
		row.url = bind.url;
		row.name = name;
		row.is_dir = meta.is_dir != 0;
		row.size = (int64_t)meta.content_length;
		row.modified_ms = meta.last_modified_ms;
		state->rows.push_back(std::move(row));
		return std::move(state);
	}

	// ls(): list children (optionally recursive).
	std::string dir = rel;
	if (dir.empty() || dir.back() != '/') {
		dir += "/";
	}
	OdopError err = {};
	OdopEntryList *list = odop_list(op, dir.c_str(), bind.recursive ? 1 : 0, &err);
	if (!list) {
		throw IOException("opendal ls: " + bind.url + ": " + ErrText(err));
	}
	ClearErr(err);
	// Normalized directory path (no trailing slash, no leading slash) used to
	// skip the self-entry. OpenDAL returns entry paths without a leading slash.
	std::string self_path = dir;
	if (!self_path.empty() && self_path.back() == '/') {
		self_path.pop_back();
	}
	while (!self_path.empty() && self_path.front() == '/') {
		self_path.erase(self_path.begin());
	}
	size_t n = odop_list_len(list);
	for (size_t i = 0; i < n; i++) {
		OdopEntry ent = {};
		if (!odop_list_entry(list, i, &ent)) {
			continue;
		}
		std::string epath = ent.path ? std::string(ent.path) : std::string();
		std::string epath_norm = epath;
		if (!epath_norm.empty() && epath_norm.back() == '/') {
			epath_norm.pop_back();
		}
		while (!epath_norm.empty() && epath_norm.front() == '/') {
			epath_norm.erase(epath_norm.begin());
		}
		if (epath_norm == self_path) {
			continue; // the listed directory's own entry
		}
		std::string ename = ent.name ? std::string(ent.name) : std::string();
		if (!ename.empty() && ename.back() == '/') {
			ename.pop_back();
		}
		if (ename.empty()) {
			continue;
		}
		FsRow row;
		row.url = OpenDalFileSystem::BuildUrlPublic(scheme, auth, epath);
		row.name = ename;
		row.is_dir = ent.is_dir != 0;
		row.size = (int64_t)ent.content_length;
		row.modified_ms = ent.last_modified_ms;
		state->rows.push_back(std::move(row));
	}
	odop_list_free(list);

	std::sort(state->rows.begin(), state->rows.end(), [](const FsRow &a, const FsRow &b) { return a.url < b.url; });
	return std::move(state);
}

static void LsFunc(ClientContext &context, TableFunctionInput &data, DataChunk &output) {
	auto &state = data.global_state->Cast<LsGlobalState>();
	idx_t count = 0;
	idx_t remaining = state.rows.size() - state.offset;
	idx_t to_emit = remaining < STANDARD_VECTOR_SIZE ? remaining : STANDARD_VECTOR_SIZE;

	for (idx_t i = 0; i < to_emit; i++) {
		auto &row = state.rows[state.offset + i];
		output.SetValue(0, count, Value(row.url));
		output.SetValue(1, count, Value(row.name));
		output.SetValue(2, count, Value(row.is_dir ? "directory" : "file"));
		if (row.is_dir) {
			output.SetValue(3, count, Value(LogicalType::BIGINT)); // NULL size for dirs
			output.SetValue(4, count, Value(LogicalType::VARCHAR));
		} else {
			output.SetValue(3, count, Value::BIGINT(row.size));
			output.SetValue(4, count, Value(HumanSize(row.size)));
		}
		if (row.modified_ms >= 0) {
			output.SetValue(5, count, Value::TIMESTAMP(Timestamp::FromEpochMs(row.modified_ms)));
		} else {
			output.SetValue(5, count, Value(LogicalType::TIMESTAMP));
		}
		count++;
	}
	state.offset += to_emit;
	output.SetCardinality(count);
}

// ── du() ─────────────────────────────────────────────────────────────────────

struct DuBindData : public TableFunctionData {
	OpenDalFileSystem *fs;
	std::string url;
};

struct DuRow {
	std::string directory;
	int64_t file_count;
	int64_t total_size;
};

struct DuGlobalState : public GlobalTableFunctionState {
	std::vector<DuRow> rows;
	idx_t offset = 0;
};

static unique_ptr<GlobalTableFunctionState> DuInit(ClientContext &context, TableFunctionInitInput &input) {
	auto &bind = input.bind_data->Cast<DuBindData>();
	auto state = make_uniq<DuGlobalState>();
	auto *fs = bind.fs;
	if (!fs) {
		throw InvalidInputException("opendal du: filesystem not available");
	}

	std::string scheme, auth, rel;
	if (!fs->ParsePublic(bind.url, scheme, auth, rel)) {
		throw InvalidInputException("opendal: unsupported or invalid URL: " + bind.url);
	}
	OdopOperator *op = fs->OperatorForPublic(scheme, auth, bind.url, &context);

	std::string dir = rel;
	if (dir.empty() || dir.back() != '/') {
		dir += "/";
	}
	OdopError err = {};
	OdopEntryList *list = odop_list(op, dir.c_str(), /*recursive=*/1, &err);
	if (!list) {
		throw IOException("opendal du: " + bind.url + ": " + ErrText(err));
	}
	ClearErr(err);

	// Roll up sizes per immediate parent directory.
	std::map<std::string, std::pair<int64_t, int64_t>> rollup; // dir -> (count, size)
	size_t n = odop_list_len(list);
	for (size_t i = 0; i < n; i++) {
		OdopEntry ent = {};
		if (!odop_list_entry(list, i, &ent) || ent.is_dir) {
			continue;
		}
		std::string epath = ent.path ? std::string(ent.path) : std::string();
		auto slash = epath.find_last_of('/');
		std::string parent = (slash == std::string::npos) ? "" : epath.substr(0, slash);
		auto &agg = rollup[parent];
		agg.first += 1;
		agg.second += (int64_t)ent.content_length;
	}
	odop_list_free(list);

	for (auto &kv : rollup) {
		DuRow row;
		row.directory = OpenDalFileSystem::BuildUrlPublic(scheme, auth, kv.first);
		row.file_count = kv.second.first;
		row.total_size = kv.second.second;
		state->rows.push_back(std::move(row));
	}
	std::sort(state->rows.begin(), state->rows.end(),
	          [](const DuRow &a, const DuRow &b) { return a.total_size > b.total_size; });
	return std::move(state);
}

static void DuFunc(ClientContext &context, TableFunctionInput &data, DataChunk &output) {
	auto &state = data.global_state->Cast<DuGlobalState>();
	idx_t count = 0;
	idx_t remaining = state.rows.size() - state.offset;
	idx_t to_emit = remaining < STANDARD_VECTOR_SIZE ? remaining : STANDARD_VECTOR_SIZE;
	for (idx_t i = 0; i < to_emit; i++) {
		auto &row = state.rows[state.offset + i];
		output.SetValue(0, count, Value(row.directory));
		output.SetValue(1, count, Value::BIGINT(row.file_count));
		output.SetValue(2, count, Value::BIGINT(row.total_size));
		output.SetValue(3, count, Value(HumanSize(row.total_size)));
		count++;
	}
	state.offset += to_emit;
	output.SetCardinality(count);
}

// ── registration ─────────────────────────────────────────────────────────────

// DuckDB table-function callbacks are plain function pointers, so the
// OpenDalFileSystem pointer is carried on the TableFunction's function_info (an
// OpenDalTableInfo) and read back in each bind via input.info.

static unique_ptr<FunctionData> LsBindWithFs(ClientContext &context, TableFunctionBindInput &input,
                                             vector<LogicalType> &return_types, vector<string> &names) {
	auto result = make_uniq<LsBindData>();
	result->fs = input.info->Cast<OpenDalTableInfo>().fs;
	result->url = input.inputs[0].GetValue<string>();
	result->single = false;
	for (auto &kv : input.named_parameters) {
		if (kv.first == "recursive") {
			result->recursive = BooleanValue::Get(kv.second);
		}
	}
	DefineLsColumns(return_types, names);
	return std::move(result);
}

static unique_ptr<FunctionData> StatBindWithFs(ClientContext &context, TableFunctionBindInput &input,
                                               vector<LogicalType> &return_types, vector<string> &names) {
	auto result = make_uniq<LsBindData>();
	result->fs = input.info->Cast<OpenDalTableInfo>().fs;
	result->url = input.inputs[0].GetValue<string>();
	result->single = true;
	DefineLsColumns(return_types, names);
	return std::move(result);
}

static unique_ptr<FunctionData> DuBindWithFs(ClientContext &context, TableFunctionBindInput &input,
                                             vector<LogicalType> &return_types, vector<string> &names) {
	auto result = make_uniq<DuBindData>();
	result->fs = input.info->Cast<OpenDalTableInfo>().fs;
	result->url = input.inputs[0].GetValue<string>();
	names = {"directory", "file_count", "total_size", "size_pretty"};
	return_types = {LogicalType::VARCHAR, LogicalType::BIGINT, LogicalType::BIGINT, LogicalType::VARCHAR};
	return std::move(result);
}

void RegisterOpenDalTableFunctions(ExtensionLoader &loader, OpenDalFileSystem *fs) {
	// opendal_ls(url, recursive := false)
	TableFunction ls("opendal_ls", {LogicalType::VARCHAR}, LsFunc, LsBindWithFs, LsInit);
	ls.named_parameters["recursive"] = LogicalType::BOOLEAN;
	ls.function_info = make_shared_ptr<OpenDalTableInfo>(fs);
	loader.RegisterFunction(ls);

	// opendal_stat(url)
	TableFunction st("opendal_stat", {LogicalType::VARCHAR}, LsFunc, StatBindWithFs, LsInit);
	st.function_info = make_shared_ptr<OpenDalTableInfo>(fs);
	loader.RegisterFunction(st);

	// opendal_du(url)
	TableFunction du("opendal_du", {LogicalType::VARCHAR}, DuFunc, DuBindWithFs, DuInit);
	du.function_info = make_shared_ptr<OpenDalTableInfo>(fs);
	loader.RegisterFunction(du);
}

} // namespace duckdb
