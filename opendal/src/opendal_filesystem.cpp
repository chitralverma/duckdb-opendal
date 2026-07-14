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
#include <unistd.h>
#include <unordered_set>

// Portable glob wildcard match (POSIX fnmatch replacement, Windows-safe).
// Supports '*' (any sequence) and '?' (any single char). Returns 0 on match.
static int glob_match(const char *pattern, const char *name) {
	while (*pattern && *name) {
		if (*pattern == '*') {
			while (*pattern == '*') {
				++pattern;
			}
			if (!*pattern) {
				return 0;
			}
			while (*name) {
				if (glob_match(pattern, name++) == 0) {
					return 0;
				}
			}
			return 1;
		}
		if (*pattern != '?' && *pattern != *name) {
			return 1;
		}
		++pattern;
		++name;
	}
	while (*pattern == '*') {
		++pattern;
	}
	return (*pattern || *name) ? 1 : 0;
}

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
			cur.push_back(c);
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

// Resolve a possibly-relative `fs` path to an absolute one against the current
// working directory. OpenDAL's fs operator is configured with root "/", so it
// expects absolute paths; DuckDB may hand us relative paths (e.g. test dirs).
static std::string AbsolutizeFsPath(const std::string &path) {
	if (!path.empty() && path[0] == '/') {
		return path;
	}
	char cwd[4096];
	if (getcwd(cwd, sizeof(cwd)) != nullptr) {
		std::string base(cwd);
		if (!base.empty() && base.back() == '/') {
			base.pop_back();
		}
		return base + "/" + path;
	}
	return "/" + path;
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
		if (!write_closed) {
			OdError err = {};
			if (od_writer_close(writer, &err) != 0) {
				close_error = err.message ? err.message : "unknown error";
				if (logger) {
					DUCKDB_LOG_ERROR(logger, "OpenDAL writer close failed for '%s': %s", path, close_error);
				}
				OdError aerr = {};
				if (od_writer_abort(writer, &aerr) != 0 && logger) {
					DUCKDB_LOG_ERROR(logger, "OpenDAL writer abort failed for '%s': %s", path,
					                 aerr.message ? aerr.message : "unknown error");
				}
				if (aerr.message) {
					od_string_free(aerr.message);
				}
			}
			if (err.message) {
				od_string_free(err.message);
			}
			write_closed = true;
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

// Whether a scheme is "path-style": the whole component after :// is a path and
// there is no authority (local/embedded backends like fs, memory). All other
// schemes are "authority-style" — the authority carries a name (bucket for s3,
// container for azblob, …) that OpenDAL's from_uri maps to the right config key.
//
// This distinction is irreducible and mirrors OpenDAL itself: a service's
// from_uri either consumes the authority (s3 → bucket) or ignores it and reads
// the path as root (fs). We only need it to decide how to split the URL and
// whether a SCOPE-matched secret applies; the config mapping is OpenDAL's job.
// Extend this set as path-style schemes are enabled.
static bool SchemeIsPathStyle(const std::string &scheme) {
	return scheme == "fs" || scheme == "memory";
}

// Authority-style schemes carry credentials via a SCOPE-matched secret (or env);
// path-style local schemes (fs/memory) need none.
static bool SchemeNeedsSecret(const std::string &scheme) {
	return !SchemeIsPathStyle(scheme);
}

bool OpenDalFileSystem::ParseUrl(const std::string &url, std::string &out_scheme, std::string &out_authority,
                                 std::string &out_path) {
	// Lightweight local split — this is a hot path (called per FS op), so we do
	// not round-trip through the FFI/OperatorUri here. The canonical OpenDAL
	// parse happens once, operator-side, at construction (see od_operator_new).
	// The split itself is mechanical: scheme, then either an authority (first
	// segment, for object stores) or the whole remainder as a path (fs/memory).
	auto pos = url.find("://");
	if (pos == std::string::npos) {
		return false;
	}
	out_scheme = url.substr(0, pos);
	if (out_scheme.empty()) {
		return false;
	}
	std::string rest = url.substr(pos + 3);
	out_authority.clear();

	if (SchemeIsPathStyle(out_scheme)) {
		// fs / memory: no authority; everything after :// is the path.
		out_path = rest.empty() ? "/" : rest;
		if (out_scheme == "fs") {
			out_path = AbsolutizeFsPath(out_path);
		}
		return true;
	}

	// Authority-style (object stores): scheme://<authority>/<key>. The authority
	// (bucket/container) is mapped to service config by OpenDAL's from_uri; here
	// we only split it off so per-call paths are the object key (leading slash
	// kept; OpenDAL normalizes it).
	auto slash = rest.find('/');
	if (slash == std::string::npos) {
		out_authority = rest;
		out_path = "/";
	} else {
		out_authority = rest.substr(0, slash);
		out_path = rest.substr(slash);
		if (out_path.empty()) {
			out_path = "/";
		}
	}
	return true;
}

// Rebuild a "scheme://[authority/]path" URL from an OpenDAL entry path. OpenDAL
// returns fs entry paths relative to root "/" (without a leading slash), so for
// `fs` we prepend "/". For authority-style schemes we re-insert the authority.
std::string OpenDalFileSystem::BuildUrl(const std::string &scheme, const std::string &authority,
                                        const std::string &entry_path) {
	std::string p = entry_path;
	if (!SchemeIsPathStyle(scheme)) {
		// entry paths are object keys relative to the bucket root (no leading /).
		while (!p.empty() && p.front() == '/') {
			p.erase(p.begin());
		}
		return scheme + "://" + authority + "/" + p;
	}
	if (scheme == "fs" && (p.empty() || p[0] != '/')) {
		p = "/" + p;
	}
	return scheme + "://" + p;
}

// Whether this build serves `scheme`. Not hardcoded: we ask the Rust core,
// which probes OpenDAL's operator registry (od_scheme_supported), so the set
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
	try {
		SecretManager *sm = context ? &SecretManager::Get(*context) : &SecretManager::Get(*db);
		CatalogTransaction txn = context ? CatalogTransaction::GetSystemCatalogTransaction(*context)
		                                 : CatalogTransaction::GetSystemTransaction(*db);
		auto match = sm->LookupSecret(txn, url, scheme);
		if (!match.HasMatch()) {
			return;
		}
		const auto &base = *match.secret_entry->secret;
		const auto &kv = dynamic_cast<const KeyValueSecret &>(base);
		for (auto &entry : kv.secret_map) {
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
	} catch (...) {
		return;
	}
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
	// The operator URI: scheme://authority (no path). OpenDAL's per-service
	// from_uri parsing maps the authority to the right config key — s3 bucket,
	// gcs bucket, azblob container, etc. — so we do not special-case it here.
	// fs/memory have an empty authority. Per-call paths are passed relative to
	// the operator root (which stays default), unaffected by this URI.
	std::string uri = scheme + "://" + authority;

	std::map<std::string, std::string> config;
	std::map<std::string, std::string> options = GlobalConfigSnapshot();

	// The local `fs` service treats the URI path as its root; we instead root it
	// at "/" and pass absolute paths per call (see AbsolutizeFsPath). Set it
	// explicitly as an override since the fs:// URI carries no path.
	if (scheme == "fs") {
		config["root"] = "/";
	}

	// Merge a SCOPE-matched secret's config (if any). These override any config
	// OpenDAL parsed from the URI.
	//
	// When no secret provides credentials/config, we deliberately do NOT inject
	// AWS_* env vars ourselves: the S3 backend already loads them natively at
	// build time -- region (AWS_REGION/AWS_DEFAULT_REGION), endpoint
	// (AWS_ENDPOINT_URL/AWS_ENDPOINT/AWS_S3_ENDPOINT) and the full credential
	// chain (env -> shared profile -> IMDS) via DefaultCredentialProvider.
	// Injecting a partial subset here would only shadow that richer resolution.
	ApplySecret(context, db, scheme, url, config, options);
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
		auto handle = make_uniq<OpenDalFileHandle>(*this, path, flags, writer);
		handle->on_disk = (scheme == "fs");
		if (opener) {
			handle->TryAddLogger(*opener);
		}
		DUCKDB_LOG_FILE_SYSTEM_OPEN((*handle));
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

	auto handle =
	    make_uniq<OpenDalFileHandle>(*this, path, flags, reader, (int64_t)meta.content_length, meta.last_modified_ms);
	handle->on_disk = (scheme == "fs");
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
	// OpenDAL writers are append-only. DuckDB's Parquet/CSV writers emit data
	// sequentially, so `location` must equal the running byte count.
	if ((int64_t)location != h.bytes_written) {
		throw IOException("opendal: non-sequential write at offset " + std::to_string((int64_t)location) +
		                  " (expected " + std::to_string(h.bytes_written) +
		                  "); random-access writes are not supported for " + h.path);
	}
	OdError err = {};
	if (od_writer_write(h.writer, static_cast<const uint8_t *>(buffer), (uint64_t)nr_bytes, &err) != 0) {
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
	if (!h.writer || h.write_closed) {
		return;
	}
	OdError err = {};
	if (od_writer_close(h.writer, &err) != 0) {
		ThrowIfError(err, "opendal close (write): " + h.path);
		throw IOException("opendal close failed: " + h.path);
	}
	ClearError(err);
	h.write_closed = true;
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
		if (!have_ctx && SchemeNeedsSecret(scheme)) {
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
	std::string pattern = (slash_before == std::string::npos) ? rel : rel.substr(slash_before + 1);
	bool recursive = pattern.find("**") != std::string::npos;

	// Strip leading "**/" so the trailing pattern applies to the basename.
	std::string file_pat = pattern;
	while (file_pat.size() >= 3 && file_pat.compare(0, 3, "**/") == 0) {
		file_pat = file_pat.substr(3);
	}
	if (file_pat.empty()) {
		file_pat = "*";
	}

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
		std::string ename = ent.name ? std::string(ent.name) : std::string();
		if (glob_match(file_pat.c_str(), ename.c_str()) == 0) {
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
	// Only the local `fs` scheme is on-disk; cached at open time (no re-parse).
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
	if (s_scheme != t_scheme || s_auth != t_auth) {
		throw IOException("opendal: move across schemes/buckets is not supported (" + source + " -> " + target + ")");
	}
	auto *op = OperatorFor(s_scheme, s_auth, source, opener);

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
