#include "opendal_filesystem.hpp"

#include "duckdb/common/exception.hpp"
#include "duckdb/common/types/timestamp.hpp"
#include "duckdb/logging/file_system_logger.hpp"
#include "duckdb/logging/logger.hpp"
#include "duckdb/main/database.hpp"
#include "duckdb/main/secret/secret.hpp"
#include "duckdb/main/secret/secret_manager.hpp"
#include "duckdb/catalog/catalog_transaction.hpp"

#include <algorithm>
#include <cctype>
#include <unordered_set>

namespace duckdb {

// ── Native-filesystem override (opendal_override_native_filesystems) ─────────
// A process-global set of schemes for which opendal should win DuckDB's VFS
// dispatch over a native/core extension (e.g. httpfs's s3://). Populated by the
// `opendal_override_native_filesystems` setting.
//
// DuckDB's VFS dispatch calls CanHandleFile(path) and, on the same thread and
// immediately after, IsManuallySet(). We stash the just-matched scheme in a
// thread_local so IsManuallySet() can answer per-scheme without a path arg.
static std::mutex g_override_mu;
static std::unordered_set<std::string> g_override_schemes;
static thread_local std::string t_last_scheme;

static std::mutex g_config_mu;
static std::map<std::string, std::map<std::string, std::string>> g_global_config;

static bool SchemeIsOverridden(const std::string &scheme) {
	std::lock_guard<std::mutex> lk(g_override_mu);
	return g_override_schemes.find(scheme) != g_override_schemes.end();
}

void OpenDalFileSystem::SetOverrideSchemes(const std::string &csv) {
	std::unordered_set<std::string> set;
	std::string cur;
	for (char c : csv) {
		if (c == ',' || c == ' ' || c == '\t') {
			if (!cur.empty()) {
				set.insert(cur);
				cur.clear();
			}
		} else {
			cur.push_back(static_cast<char>(std::tolower(static_cast<unsigned char>(c))));
		}
	}
	if (!cur.empty()) {
		set.insert(cur);
	}
	std::lock_guard<std::mutex> lk(g_override_mu);
	g_override_schemes = std::move(set);
}

void OpenDalFileSystem::SetGlobalConfig(const std::string &section, std::map<std::string, std::string> values) {
	std::lock_guard<std::mutex> lk(g_config_mu);
	g_global_config[section] = std::move(values);
}

static std::map<std::string, std::string> GlobalConfigSnapshot() {
	std::lock_guard<std::mutex> lk(g_config_mu);
	std::map<std::string, std::string> result;
	for (auto &section : g_global_config) {
		for (auto &entry : section.second) {
			result[section.first + "." + entry.first] = entry.second;
		}
	}
	return result;
}

// ─── Error helper ────────────────────────────────────────────────────────────
// Convert an OdError into a thrown IOException (freeing its message), or
// return quietly if there is no error. `context` prefixes the message.
static void ThrowIfError(OdError &err, const std::string &context) {
	if (err.code == OdErrorCode::Ok) {
		return;
	}
	std::string msg = context;
	if (err.message) {
		msg += ": ";
		msg += err.message;
		od_string_free(err.message);
		err.message = nullptr;
	}
	throw IOException(msg);
}

// Free an OdError's message if present (for non-throwing paths).
static void ClearError(OdError &err) {
	if (err.message) {
		od_string_free(err.message);
		err.message = nullptr;
	}
}

static void FreeReader(OdReader *reader) {
	od_reader_free(reader);
}

static void AbortAndFreeWriter(OdWriter *writer) {
	OdError err = {};
	od_writer_abort(writer, &err);
	ClearError(err);
	od_writer_free(writer);
}

// Whether `op` reports support for capability `name` (e.g. "rename", "copy").
// Reads the operator's cached capability flag over a single FFI call. Names
// match OpenDAL's Capability field names.
static bool OperatorSupports(OdOperator *op, const char *name) {
	return od_operator_supports(op, name) != 0;
}

// ─── OpenDalFileHandle ───────────────────────────────────────────────────────
OpenDalFileHandle::OpenDalFileHandle(FileSystem &fs, const std::string &path, FileOpenFlags flags, OdReader *reader_,
                                     int64_t file_size_, int64_t last_modified_ms_)
    : FileHandle(fs, path, flags), reader(reader_), file_size(file_size_), last_modified_ms(last_modified_ms_) {
}

OpenDalFileHandle::OpenDalFileHandle(FileSystem &fs, const std::string &path, FileOpenFlags flags, OdWriter *writer_)
    : FileHandle(fs, path, flags), writer(writer_) {
}

OpenDalFileHandle::~OpenDalFileHandle() {
	CloseInternal(false);
}

void OpenDalFileHandle::Close() {
	CloseInternal(true);
}

void OpenDalFileHandle::CloseInternal(bool throw_on_error) {
	if (reader || writer) {
		DUCKDB_LOG_FILE_SYSTEM_CLOSE((*this));
	}
	if (reader) {
		od_reader_free(reader);
		reader = nullptr;
	}
	if (writer) {
		std::string close_error;
		if (!write_finished) {
			OdError err = {};
			int result = write_committable ? od_writer_close(writer, &err) : od_writer_abort(writer, &err);
			if (result != 0) {
				close_error = err.message ? err.message : "unknown error";
				if (logger) {
					DUCKDB_LOG_ERROR(logger, "OpenDAL writer %s failed for '%s': %s",
					                 write_committable ? "close" : "abort", path, close_error);
				}
			}
			if (err.message) {
				od_string_free(err.message);
			}
			write_finished = true;
		}
		od_writer_free(writer);
		writer = nullptr;
		if (throw_on_error && !close_error.empty()) {
			throw IOException("opendal close (write): " + path + ": " + close_error);
		}
	}
}

// ─── OpenDalFileSystem ───────────────────────────────────────────────────────
OpenDalFileSystem::OpenDalFileSystem() = default;

OpenDalFileSystem::~OpenDalFileSystem() {
	std::lock_guard<std::mutex> lk(mu_);
	for (auto &entry : operators_) {
		if (entry.second) {
			od_operator_free(entry.second);
		}
	}
	operators_.clear();
}

bool OpenDalFileSystem::ParseUrl(const std::string &url, std::string &out_scheme, std::string &out_authority,
                                 std::string &out_path) {
	OdError err = {};
	char *scheme = nullptr;
	char *authority = nullptr;
	char *path = nullptr;
	if (od_url_resolve(url.c_str(), &scheme, &authority, &path, &err) != 0) {
		ClearError(err);
		return false;
	}
	ClearError(err);
	std::unique_ptr<char, void (*)(char *)> scheme_guard(scheme, od_string_free);
	std::unique_ptr<char, void (*)(char *)> authority_guard(authority, od_string_free);
	std::unique_ptr<char, void (*)(char *)> path_guard(path, od_string_free);
	out_scheme = scheme_guard.get();
	out_authority = authority_guard.get();
	out_path = path_guard.get();
	return true;
}

// Rebuild the extension's universal scheme://authority/path URL.
std::string OpenDalFileSystem::BuildUrl(const std::string &scheme, const std::string &authority,
                                        const std::string &entry_path) {
	auto raw = od_url_build(scheme.c_str(), authority.c_str(), entry_path.c_str());
	if (!raw) {
		throw IOException("opendal: failed to build public URL");
	}
	std::string result(raw);
	od_string_free(raw);
	return result;
}

// Whether this build serves `scheme`. Not hardcoded: we ask the Rust core,
// which reads OpenDAL's operator registry, so the set
// tracks exactly the services compiled in via Cargo features.
static bool IsSupportedScheme(const std::string &scheme) {
	return od_scheme_supported(scheme.c_str()) != 0;
}

bool OpenDalFileSystem::ParsePublic(const std::string &url, std::string &out_scheme, std::string &out_authority,
                                    std::string &out_path) {
	if (!ParseUrl(url, out_scheme, out_authority, out_path)) {
		return false;
	}
	return IsSupportedScheme(out_scheme);
}

// Merge a SCOPE-matched secret over global options. Backend config is separate;
// section prefixes (`io.`/`retry.`/`timeout.`/`cache.`) stay intact for Rust.
static void ApplySecret(optional_ptr<ClientContext> context, optional_ptr<DatabaseInstance> db,
                        const std::string &scheme, const std::string &url, std::map<std::string, std::string> &config,
                        std::map<std::string, std::string> &options) {
	if (!context && !db) {
		return;
	}
	SecretManager *sm = context ? &SecretManager::Get(*context) : &SecretManager::Get(*db);
	CatalogTransaction txn = context ? CatalogTransaction::GetSystemCatalogTransaction(*context)
	                                 : CatalogTransaction::GetSystemTransaction(*db);
	auto match = sm->LookupSecret(txn, url, scheme);
	if (!match.HasMatch()) {
		return;
	}
	const auto &base = *match.secret_entry->secret;
	auto kv = dynamic_cast<const KeyValueSecret *>(&base);
	if (!kv) {
		throw IOException("opendal: matched secret for '" + url + "' is not a key-value secret");
	}
	for (auto &entry : kv->secret_map) {
		const std::string &k = entry.first;
		if (k == "__scheme") {
			continue;
		}
		std::string v = entry.second.ToString();
		if (k.rfind("config.", 0) == 0) {
			config[k.substr(7)] = v;
		} else {
			options[k] = v;
		}
	}
}

static std::string CanonicalSchemeUrl(const std::string &url, const std::string &scheme) {
	auto separator = url.find("://");
	return separator == std::string::npos ? url : scheme + url.substr(separator);
}

static std::string ConfigIdentity(const std::string &scheme, const std::string &authority,
                                  const std::map<std::string, std::string> &config,
                                  const std::map<std::string, std::string> &options, uint64_t &hash) {
	hash = 1469598103934665603ULL;
	std::string identity = scheme + "://" + authority;
	auto add = [&](const std::string &value) {
		identity += "|" + std::to_string(value.size()) + ":" + value;
		for (auto c : value) {
			hash ^= static_cast<uint8_t>(c);
			hash *= 1099511628211ULL;
		}
		hash ^= 0xff;
		hash *= 1099511628211ULL;
	};
	auto append = [&](const std::map<std::string, std::string> &values) {
		for (auto &entry : values) {
			add(entry.first);
			add(entry.second);
		}
	};
	add(scheme);
	add(authority);
	append(config);
	append(options);
	return identity;
}

OdOperator *OpenDalFileSystem::OperatorFor(const std::string &scheme, const std::string &authority,
                                           const std::string &url, optional_ptr<FileOpener> opener) {
	auto context = FileOpener::TryGetClientContext(opener);
	auto db = FileOpener::TryGetDatabase(opener);
	return BuildOperator(scheme, authority, url, context, db);
}

OdOperator *OpenDalFileSystem::OperatorForCtx(const std::string &scheme, const std::string &authority,
                                              const std::string &url, optional_ptr<ClientContext> context) {
	optional_ptr<DatabaseInstance> db;
	if (context) {
		db = &DatabaseInstance::GetDatabase(*context);
	}
	return BuildOperator(scheme, authority, url, context, db);
}

OdOperator *OpenDalFileSystem::BuildOperator(const std::string &scheme, const std::string &authority,
                                             const std::string &url, optional_ptr<ClientContext> context,
                                             optional_ptr<DatabaseInstance> db) {
	// Operator URI contains only scheme + authority. URL path is always the
	// operation path; remaining service configuration comes from the secret.
	std::string uri = scheme + "://" + authority;

	std::map<std::string, std::string> config;
	std::map<std::string, std::string> options = GlobalConfigSnapshot();

	// Merge a SCOPE-matched secret's config (if any). These override any config
	// OpenDAL parsed from the URI.
	//
	// When no secret provides credentials/config, we deliberately do NOT inject
	// AWS_* env vars ourselves: the S3 backend already loads them natively at
	// build time -- region (AWS_REGION/AWS_DEFAULT_REGION), endpoint
	// (AWS_ENDPOINT_URL/AWS_ENDPOINT/AWS_S3_ENDPOINT) and the full credential
	// chain (env -> shared profile -> IMDS) via DefaultCredentialProvider.
	// Injecting a partial subset here would only shadow that richer resolution.
	ApplySecret(context, db, scheme, CanonicalSchemeUrl(url, scheme), config, options);
	uint64_t config_hash;
	std::string key = ConfigIdentity(scheme, authority, config, options, config_hash);
	std::string cache_namespace = std::to_string(config_hash);

	{
		std::lock_guard<std::mutex> lk(mu_);
		auto it = operators_.find(key);
		if (it != operators_.end()) {
			return it->second;
		}
	}

	std::vector<std::string> keys, vals, option_keys, option_vals;
	for (auto &entry : config) {
		keys.push_back(entry.first);
		vals.push_back(entry.second);
	}
	for (auto &entry : options) {
		option_keys.push_back(entry.first);
		option_vals.push_back(entry.second);
	}

	std::vector<const char *> key_ptrs, val_ptrs, option_key_ptrs, option_val_ptrs;
	for (size_t i = 0; i < keys.size(); i++) {
		key_ptrs.push_back(keys[i].c_str());
		val_ptrs.push_back(vals[i].c_str());
	}
	for (size_t i = 0; i < option_keys.size(); i++) {
		option_key_ptrs.push_back(option_keys[i].c_str());
		option_val_ptrs.push_back(option_vals[i].c_str());
	}

	// Build the operator OUTSIDE the lock: od_operator_new may do DNS/network
	// work, and holding mu_ across it would serialize operator creation for
	// unrelated schemes/buckets.
	OdError err = {};
	OdOperator *op = od_operator_new(
	    uri.c_str(), key_ptrs.empty() ? nullptr : key_ptrs.data(), val_ptrs.empty() ? nullptr : val_ptrs.data(),
	    keys.size(), option_key_ptrs.empty() ? nullptr : option_key_ptrs.data(),
	    option_val_ptrs.empty() ? nullptr : option_val_ptrs.data(), option_keys.size(), cache_namespace.c_str(), &err);
	if (!op) {
		ThrowIfError(err, "opendal: failed to create operator for '" + uri + "'");
		throw IOException("opendal: null operator for '" + uri + "'");
	}
	ClearError(err);
	if (auto warning = od_operator_warning(op)) {
		if (context) {
			DUCKDB_LOG_WARNING(*context, warning);
		} else if (db) {
			DUCKDB_LOG_WARNING(*db, warning);
		}
	}

	// Publish under the lock. If another thread built the same key concurrently,
	// keep the first one and free ours (last-writer-loses, avoids a leak).
	std::lock_guard<std::mutex> lk(mu_);
	auto res = operators_.emplace(key, op);
	if (!res.second) {
		od_operator_free(op);
		return res.first->second;
	}
	return op;
}

bool OpenDalFileSystem::CanHandleFile(const string &path) {
	t_last_scheme.clear();
	std::string scheme, auth, rel;
	if (!ParseUrl(path, scheme, auth, rel)) {
		return false;
	}
	if (!IsSupportedScheme(scheme)) {
		return false;
	}
	// Record the matched scheme for IsManuallySet() (called next, same thread).
	t_last_scheme = scheme;
	return true;
}

bool OpenDalFileSystem::IsManuallySet() {
	// Win dispatch only for schemes the user put in the override list.
	return SchemeIsOverridden(t_last_scheme);
}

unique_ptr<FileHandle> OpenDalFileSystem::OpenFile(const string &path, FileOpenFlags flags,
                                                   optional_ptr<FileOpener> opener) {
	std::string scheme, auth, rel;
	if (!ParseUrl(path, scheme, auth, rel)) {
		throw IOException("opendal: unrecognized URL: " + path);
	}
	auto *op = OperatorFor(scheme, auth, path, opener);

	if (flags.OpenForWriting()) {
		// Streaming, append-only write. OpenDAL overwrites any existing object
		// on close. (Read-modify-write / partial overwrite is not supported.)
		OdError werr = {};
		OdWriter *writer = od_writer_open(op, rel.c_str(), &werr);
		if (!writer) {
			ThrowIfError(werr, "opendal open (write): " + path);
			throw IOException("opendal: null writer for " + path);
		}
		ClearError(werr);
		std::unique_ptr<OdWriter, void (*)(OdWriter *)> writer_guard(writer, AbortAndFreeWriter);
		auto handle = make_uniq<OpenDalFileHandle>(*this, path, flags, writer);
		writer_guard.release();
		handle->on_disk = false;
		if (opener) {
			handle->TryAddLogger(*opener);
		}
		DUCKDB_LOG_FILE_SYSTEM_OPEN((*handle));
		handle->write_committable = true;
		return std::move(handle);
	}

	// Read mode: stat first to get size + type.
	OdMetadata meta = {};
	OdError serr = {};
	od_stat(op, rel.c_str(), &meta, &serr);
	ThrowIfError(serr, "opendal stat: " + path);
	if (meta.is_dir) {
		throw IOException("opendal: cannot open directory as file: " + path);
	}

	OdError rerr = {};
	OdReader *reader = od_reader_open(op, rel.c_str(), &rerr);
	if (!reader) {
		ThrowIfError(rerr, "opendal open: " + path);
		throw IOException("opendal: null reader for " + path);
	}
	ClearError(rerr);
	std::unique_ptr<OdReader, void (*)(OdReader *)> reader_guard(reader, FreeReader);

	auto handle =
	    make_uniq<OpenDalFileHandle>(*this, path, flags, reader, (int64_t)meta.content_length, meta.last_modified_ms);
	reader_guard.release();
	handle->on_disk = false;
	if (opener) {
		handle->TryAddLogger(*opener);
	}
	DUCKDB_LOG_FILE_SYSTEM_OPEN((*handle));
	return std::move(handle);
}

// ─── Write (streaming, append-only) ─────────────────────────────────────────
void OpenDalFileSystem::Write(FileHandle &handle, void *buffer, int64_t nr_bytes, idx_t location) {
	auto &h = handle.Cast<OpenDalFileHandle>();
	if (!h.writer) {
		throw IOException("opendal: write on a non-writable handle: " + h.path);
	}
	if (h.write_finished) {
		throw IOException("opendal write: writer is already finalized: " + h.path);
	}
	// OpenDAL writers are append-only. DuckDB's Parquet/CSV writers emit data
	// sequentially, so `location` must equal the running byte count.
	if ((int64_t)location != h.bytes_written) {
		throw IOException("opendal: non-sequential write at offset " + std::to_string((int64_t)location) +
		                  " (expected " + std::to_string(h.bytes_written) +
		                  "); random-access writes are not supported for " + h.path);
	}
	OdError err = {};
	if (od_writer_write(h.writer, static_cast<const uint8_t *>(buffer), (uint64_t)nr_bytes, &err) != 0) {
		h.write_finished = true;
		ThrowIfError(err, "opendal write: " + h.path);
		throw IOException("opendal write failed: " + h.path);
	}
	ClearError(err);
	DUCKDB_LOG_FILE_SYSTEM_WRITE(h, nr_bytes, (idx_t)h.bytes_written);
	h.bytes_written += nr_bytes;
}

int64_t OpenDalFileSystem::Write(FileHandle &handle, void *buffer, int64_t nr_bytes) {
	auto &h = handle.Cast<OpenDalFileHandle>();
	Write(handle, buffer, nr_bytes, (idx_t)h.bytes_written);
	return nr_bytes;
}

void OpenDalFileSystem::FileSync(FileHandle &handle) {
	auto &h = handle.Cast<OpenDalFileHandle>();
	if (!h.writer || h.write_finished) {
		return;
	}
	OdError err = {};
	if (od_writer_close(h.writer, &err) != 0) {
		h.write_finished = true;
		ThrowIfError(err, "opendal close (write): " + h.path);
		throw IOException("opendal close failed: " + h.path);
	}
	ClearError(err);
	h.write_finished = true;
}

// Positioned read — one FFI call → one OpenDAL ranged read into DuckDB's buffer.
void OpenDalFileSystem::Read(FileHandle &handle, void *buffer, int64_t nr_bytes, idx_t location) {
	auto &h = handle.Cast<OpenDalFileHandle>();
	if (!h.reader) {
		throw IOException("opendal: read on closed handle: " + h.path);
	}
	OdError err = {};
	int64_t n = od_reader_read(h.reader, (uint64_t)location, (uint64_t)nr_bytes, static_cast<uint8_t *>(buffer), &err);
	if (n < 0) {
		ThrowIfError(err, "opendal read: " + h.path);
		throw IOException("opendal read failed: " + h.path);
	}
	ClearError(err);
	// DuckDB's positioned Read must fill the whole buffer; a short read here is an error.
	if (n != nr_bytes) {
		throw IOException("opendal read: short read at offset " + std::to_string((int64_t)location) + " (" +
		                  std::to_string(n) + " of " + std::to_string(nr_bytes) + " bytes) for " + h.path);
	}
	DUCKDB_LOG_FILE_SYSTEM_READ(h, n, (idx_t)location);
	h.position = (int64_t)location + n;
}

// Sequential read from the current position.
int64_t OpenDalFileSystem::Read(FileHandle &handle, void *buffer, int64_t nr_bytes) {
	auto &h = handle.Cast<OpenDalFileHandle>();
	if (!h.reader) {
		throw IOException("opendal: read on closed handle: " + h.path);
	}
	int64_t remaining = h.file_size - h.position;
	if (remaining <= 0) {
		return 0;
	}
	int64_t to_read = nr_bytes < remaining ? nr_bytes : remaining;

	OdError err = {};
	int64_t n = od_reader_read(h.reader, (uint64_t)h.position, (uint64_t)to_read, static_cast<uint8_t *>(buffer), &err);
	if (n < 0) {
		ThrowIfError(err, "opendal read: " + h.path);
		throw IOException("opendal read failed: " + h.path);
	}
	ClearError(err);
	DUCKDB_LOG_FILE_SYSTEM_READ(h, n, (idx_t)h.position);
	h.position += n;
	return n;
}

int64_t OpenDalFileSystem::GetFileSize(FileHandle &handle) {
	return handle.Cast<OpenDalFileHandle>().file_size;
}

timestamp_t OpenDalFileSystem::GetLastModifiedTime(FileHandle &handle) {
	auto &h = handle.Cast<OpenDalFileHandle>();
	if (h.last_modified_ms >= 0) {
		return Timestamp::FromEpochMs(h.last_modified_ms);
	}
	return Timestamp::GetCurrentTimestamp();
}

FileType OpenDalFileSystem::GetFileType(FileHandle &handle) {
	// Handles are only ever opened for regular files (OpenFile throws on a
	// directory path), so a handle is always a regular file.
	return FileType::FILE_TYPE_REGULAR;
}

bool OpenDalFileSystem::FileExists(const string &filename, optional_ptr<FileOpener> opener) {
	std::string scheme, auth, rel;
	if (!ParseUrl(filename, scheme, auth, rel) || !IsSupportedScheme(scheme)) {
		return false;
	}
	OdOperator *op = OperatorFor(scheme, auth, filename, opener);
	OdMetadata meta = {};
	OdError err = {};
	od_stat(op, rel.c_str(), &meta, &err);
	if (err.code == OdErrorCode::NotFound) {
		ClearError(err);
		return false;
	}
	ThrowIfError(err, "opendal stat: " + filename);
	return !meta.is_dir;
}

bool OpenDalFileSystem::DirectoryExists(const string &directory, optional_ptr<FileOpener> opener) {
	std::string scheme, auth, rel;
	if (!ParseUrl(directory, scheme, auth, rel) || !IsSupportedScheme(scheme)) {
		return false;
	}
	OdOperator *op = OperatorFor(scheme, auth, directory, opener);
	OdMetadata meta = {};
	OdError err = {};
	od_stat(op, rel.c_str(), &meta, &err);
	if (err.code == OdErrorCode::NotFound) {
		ClearError(err);
		return false;
	}
	ThrowIfError(err, "opendal stat: " + directory);
	return meta.is_dir;
}

void OpenDalFileSystem::Seek(FileHandle &handle, idx_t location) {
	handle.Cast<OpenDalFileHandle>().position = (int64_t)location;
}

// List immediate children of a directory. Callback receives (name, is_dir).
bool OpenDalFileSystem::ListFiles(const string &directory, const std::function<void(const string &, bool)> &callback,
                                  FileOpener *opener) {
	std::string scheme, auth, rel;
	if (!ParseUrl(directory, scheme, auth, rel) || !IsSupportedScheme(scheme)) {
		return false;
	}
	OdOperator *op = OperatorFor(scheme, auth, directory, opener);
	// OpenDAL expects a trailing slash to list a directory's children.
	std::string dir = rel;
	if (dir.empty() || dir.back() != '/') {
		dir += "/";
	}

	OdError err = {};
	OdEntryList *list = od_list(op, dir.c_str(), /*recursive=*/0, &err);
	if (!list) {
		if (err.code == OdErrorCode::NotFound) {
			ClearError(err);
			return false;
		}
		ThrowIfError(err, "opendal list: " + directory);
		throw IOException("opendal: null list for " + directory);
	}
	ClearError(err);
	std::unique_ptr<OdEntryList, void (*)(OdEntryList *)> list_guard(list, od_list_free);

	std::string self_path = dir;
	if (!self_path.empty() && self_path.back() == '/') {
		self_path.pop_back();
	}
	while (!self_path.empty() && self_path.front() == '/') {
		self_path.erase(self_path.begin());
	}
	size_t n = od_list_len(list);
	for (size_t i = 0; i < n; i++) {
		OdEntry ent = {};
		if (!od_list_entry(list, i, &ent)) {
			continue;
		}
		std::string epath = ent.path ? std::string(ent.path) : std::string();
		if (!epath.empty() && epath.back() == '/') {
			epath.pop_back();
		}
		while (!epath.empty() && epath.front() == '/') {
			epath.erase(epath.begin());
		}
		if (epath == self_path) {
			continue; // the listed directory's own entry
		}
		std::string name = ent.name ? std::string(ent.name) : std::string();
		if (!name.empty() && name.back() == '/') {
			name.pop_back();
		}
		if (name.empty()) {
			continue;
		}
		callback(name, ent.is_dir != 0);
	}
	return true;
}

// Glob: expand a wildcard pattern into matching file URLs. Supports '*' and '?'
// within a path segment and '**' for recursive descent.
vector<OpenFileInfo> OpenDalFileSystem::Glob(const string &path, FileOpener *opener) {
	vector<OpenFileInfo> results;
	std::string scheme, auth, rel;
	if (!ParseUrl(path, scheme, auth, rel) || !IsSupportedScheme(scheme)) {
		return results;
	}

	bool has_glob = path.find_first_of("*?[") != std::string::npos;
	if (!has_glob) {
		// No wildcards — return the path iff it exists as a file. If we have no
		// opener context to resolve a SCOPE-matched secret (DuckDB sometimes
		// globs a literal path during binding with a null opener), be optimistic
		// and return the path: the subsequent context-aware OpenFile validates
		// it (and surfaces a real error if it truly does not exist).
		bool have_ctx = opener && FileOpener::TryGetClientContext(opener);
		if (!have_ctx) {
			results.emplace_back(path);
		} else if (FileExists(path, opener)) {
			results.emplace_back(path);
		}
		return results;
	}

	OdOperator *op = OperatorFor(scheme, auth, path, opener);

	// Split `rel` into a static directory prefix (up to the last '/' before the
	// first wildcard) and the pattern portion.
	auto star = rel.find_first_of("*?[");
	auto slash_before = rel.rfind('/', star);
	std::string static_dir = (slash_before == std::string::npos) ? "" : rel.substr(0, slash_before);
	std::string pattern = rel;
	while (!pattern.empty() && pattern.front() == '/') {
		pattern.erase(pattern.begin());
	}
	std::string remaining = (slash_before == std::string::npos) ? rel : rel.substr(slash_before + 1);
	bool recursive = remaining.find('/') != std::string::npos || remaining.find("**") != std::string::npos;

	std::string list_dir = static_dir;
	if (list_dir.empty() || list_dir.back() != '/') {
		list_dir += "/";
	}

	OdError err = {};
	OdEntryList *list = od_list(op, list_dir.c_str(), recursive ? 1 : 0, &err);
	if (!list) {
		if (err.code == OdErrorCode::NotFound) {
			ClearError(err);
			return results;
		}
		ThrowIfError(err, "opendal glob: " + path);
		throw IOException("opendal: null list for glob " + path);
	}
	ClearError(err);
	std::unique_ptr<OdEntryList, void (*)(OdEntryList *)> list_guard(list, od_list_free);

	size_t n = od_list_len(list);
	for (size_t i = 0; i < n; i++) {
		OdEntry ent = {};
		if (!od_list_entry(list, i, &ent) || ent.is_dir) {
			continue;
		}
		std::string epath = ent.path ? std::string(ent.path) : std::string();
		while (!epath.empty() && epath.front() == '/') {
			epath.erase(epath.begin());
		}
		if (od_glob_match(pattern.c_str(), epath.c_str()) != 0) {
			results.emplace_back(BuildUrl(scheme, auth, epath));
		}
	}
	std::sort(results.begin(), results.end(),
	          [](const OpenFileInfo &a, const OpenFileInfo &b) { return a.path < b.path; });
	results.erase(std::unique(results.begin(), results.end(),
	                          [](const OpenFileInfo &a, const OpenFileInfo &b) { return a.path == b.path; }),
	              results.end());
	return results;
}

idx_t OpenDalFileSystem::SeekPosition(FileHandle &handle) {
	return (idx_t)handle.Cast<OpenDalFileHandle>().position;
}

bool OpenDalFileSystem::OnDiskFile(FileHandle &handle) {
	// Registry membership does not expose physical-locality metadata. Report the
	// conservative network/default posture for every service.
	return handle.Cast<OpenDalFileHandle>().on_disk;
}

// ─── Mutations ──────────────────────────────────────────────────────────────
void OpenDalFileSystem::CreateDirectory(const string &directory, optional_ptr<FileOpener> opener) {
	std::string scheme, auth, rel;
	if (!ParseUrl(directory, scheme, auth, rel) || !IsSupportedScheme(scheme)) {
		throw IOException("opendal: unsupported or invalid URL: " + directory);
	}
	auto *op = OperatorFor(scheme, auth, directory, opener);
	OdError err = {};
	if (od_create_dir(op, rel.c_str(), &err) != 0) {
		ThrowIfError(err, "opendal create_dir: " + directory);
		throw IOException("opendal create_dir failed: " + directory);
	}
	ClearError(err);
}

void OpenDalFileSystem::RemoveFile(const string &filename, optional_ptr<FileOpener> opener) {
	std::string scheme, auth, rel;
	if (!ParseUrl(filename, scheme, auth, rel) || !IsSupportedScheme(scheme)) {
		throw IOException("opendal: unsupported or invalid URL: " + filename);
	}
	auto *op = OperatorFor(scheme, auth, filename, opener);
	OdError err = {};
	if (od_remove(op, rel.c_str(), /*recursive=*/0, &err) != 0) {
		ThrowIfError(err, "opendal remove: " + filename);
		throw IOException("opendal remove failed: " + filename);
	}
	ClearError(err);
}

void OpenDalFileSystem::RemoveDirectory(const string &directory, optional_ptr<FileOpener> opener) {
	std::string scheme, auth, rel;
	if (!ParseUrl(directory, scheme, auth, rel) || !IsSupportedScheme(scheme)) {
		throw IOException("opendal: unsupported or invalid URL: " + directory);
	}
	auto *op = OperatorFor(scheme, auth, directory, opener);
	// Recursive delete for a directory tree.
	std::string dir = rel;
	if (dir.empty() || dir.back() != '/') {
		dir += "/";
	}
	OdError err = {};
	if (od_remove(op, dir.c_str(), /*recursive=*/1, &err) != 0) {
		ThrowIfError(err, "opendal remove (dir): " + directory);
		throw IOException("opendal remove (dir) failed: " + directory);
	}
	ClearError(err);
}

void OpenDalFileSystem::MoveFile(const string &source, const string &target, optional_ptr<FileOpener> opener) {
	std::string s_scheme, s_auth, s_rel, t_scheme, t_auth, t_rel;
	if (!ParseUrl(source, s_scheme, s_auth, s_rel) || !IsSupportedScheme(s_scheme)) {
		throw IOException("opendal: unsupported or invalid source URL: " + source);
	}
	if (!ParseUrl(target, t_scheme, t_auth, t_rel) || !IsSupportedScheme(t_scheme)) {
		throw IOException("opendal: unsupported or invalid target URL: " + target);
	}
	auto *op = OperatorFor(s_scheme, s_auth, source, opener);
	auto *target_op = OperatorFor(t_scheme, t_auth, target, opener);
	if (op != target_op) {
		throw IOException("opendal: move across effective operators is not supported; use opendal_copy then delete (" +
		                  source + " -> " + target + ")");
	}

	// Prefer server-side rename. If the service lacks it (e.g. s3 has no
	// rename but does have copy), fall back to copy+delete — a non-atomic move.
	// If neither is available, surface a clear error.
	if (OperatorSupports(op, "rename")) {
		OdError err = {};
		if (od_rename(op, s_rel.c_str(), t_rel.c_str(), &err) != 0) {
			ThrowIfError(err, "opendal move: " + source + " -> " + target);
			throw IOException("opendal move failed: " + source + " -> " + target);
		}
		ClearError(err);
		return;
	}

	if (OperatorSupports(op, "copy")) {
		OdError cerr = {};
		if (od_copy(op, s_rel.c_str(), t_rel.c_str(), &cerr) != 0) {
			ThrowIfError(cerr, "opendal move (copy): " + source + " -> " + target);
			throw IOException("opendal move failed (copy): " + source + " -> " + target);
		}
		ClearError(cerr);
		OdError derr = {};
		if (od_remove(op, s_rel.c_str(), /*recursive=*/0, &derr) != 0) {
			ThrowIfError(derr, "opendal move (delete source after copy): " + source);
			throw IOException("opendal move failed (delete source after copy): " + source);
		}
		ClearError(derr);
		return;
	}

	throw IOException("opendal: service '" + s_scheme + "' supports neither rename nor copy; cannot move " + source +
	                  " -> " + target);
}

} // namespace duckdb
