#include "opendal_filesystem.hpp"

#include "duckdb/common/exception.hpp"
#include "duckdb/common/types/timestamp.hpp"
#include "duckdb/main/database.hpp"
#include "duckdb/main/secret/secret.hpp"
#include "duckdb/main/secret/secret_manager.hpp"
#include "duckdb/catalog/catalog_transaction.hpp"

#include <algorithm>
#include <cstring>
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
// A process-global set of schemes for which opendal_fs should win DuckDB's VFS
// dispatch over a native/core extension (e.g. httpfs's s3://). Populated by the
// `opendal_override_native_filesystems` setting.
//
// DuckDB's VFS dispatch calls CanHandleFile(path) and, on the same thread and
// immediately after, IsManuallySet(). We stash the just-matched scheme in a
// thread_local so IsManuallySet() can answer per-scheme without a path arg.
static std::mutex g_override_mu;
static std::unordered_set<std::string> g_override_schemes;
static thread_local std::string t_last_scheme;

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
// Convert an OdopError into a thrown IOException (freeing its message), or
// return quietly if there is no error. `context` prefixes the message.
static void ThrowIfError(OdopError &err, const std::string &context) {
	if (err.code == OdopErrorCode::Ok) {
		return;
	}
	std::string msg = context;
	if (err.message) {
		msg += ": ";
		msg += err.message;
		odop_string_free(err.message);
		err.message = nullptr;
	}
	throw IOException(msg);
}

// Free an OdopError's message if present (for non-throwing paths).
static void ClearError(OdopError &err) {
	if (err.message) {
		odop_string_free(err.message);
		err.message = nullptr;
	}
}

// ─── OpenDalFileHandle ───────────────────────────────────────────────────────
OpenDalFileHandle::OpenDalFileHandle(FileSystem &fs, const std::string &path, FileOpenFlags flags,
                                     OdopReader *reader_, int64_t file_size_, int64_t last_modified_ms_)
    : FileHandle(fs, path, flags), reader(reader_), file_size(file_size_),
      last_modified_ms(last_modified_ms_) {
}

OpenDalFileHandle::OpenDalFileHandle(FileSystem &fs, const std::string &path, FileOpenFlags flags,
                                     OdopWriter *writer_)
    : FileHandle(fs, path, flags), writer(writer_) {
}

OpenDalFileHandle::~OpenDalFileHandle() {
	Close();
}

void OpenDalFileHandle::Close() {
	if (reader) {
		odop_reader_free(reader);
		reader = nullptr;
	}
	if (writer) {
		// If the handle is closed without an explicit FileSync (e.g. on error
		// unwinding), finalize the upload so partial data isn't silently lost;
		// OpenDAL flushes buffered/multipart state on close.
		if (!write_closed) {
			OdopError err = {};
			if (odop_writer_close(writer, &err) != 0) {
				// Best-effort: abort to release the multipart session.
				OdopError aerr = {};
				odop_writer_abort(writer, &aerr);
				if (aerr.message) {
					odop_string_free(aerr.message);
				}
			}
			if (err.message) {
				odop_string_free(err.message);
			}
			write_closed = true;
		}
		odop_writer_free(writer);
		writer = nullptr;
	}
}

// ─── OpenDalFileSystem ───────────────────────────────────────────────────────
OpenDalFileSystem::OpenDalFileSystem() = default;

OpenDalFileSystem::~OpenDalFileSystem() {
	std::lock_guard<std::mutex> lk(mu_);
	for (auto &entry : operators_) {
		if (entry.second) {
			odop_operator_free(entry.second);
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

bool OpenDalFileSystem::ParseUrl(const std::string &url, std::string &out_scheme,
                                 std::string &out_authority, std::string &out_path) {
	// Lightweight local split — this is a hot path (called per FS op), so we do
	// not round-trip through the FFI/OperatorUri here. The canonical OpenDAL
	// parse happens once, operator-side, at construction (see odop_operator_new).
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

// Schemes this build serves. (Expanded per-service in later phases.)
static bool IsSupportedScheme(const std::string &scheme) {
	return scheme == "fs" || scheme == "memory" || scheme == "s3";
}

bool OpenDalFileSystem::ParsePublic(const std::string &url, std::string &out_scheme,
                                    std::string &out_authority, std::string &out_path) {
	if (!ParseUrl(url, out_scheme, out_authority, out_path)) {
		return false;
	}
	return IsSupportedScheme(out_scheme);
}

// Look up a SCOPE-matched secret for `url` of type `scheme`, appending its
// config entries into `keys`/`vals` (OpenDAL config) and `lkeys`/`lvals`
// (layer options, from "layer." prefixed entries). Convenience/config keys were
// already normalized to OpenDAL keys at CREATE SECRET time. Returns true if a
// secret was found and applied.
static bool ApplySecret(optional_ptr<ClientContext> context, optional_ptr<DatabaseInstance> db,
                        const std::string &scheme, const std::string &url,
                        std::vector<std::string> &keys, std::vector<std::string> &vals,
                        std::vector<std::string> &lkeys, std::vector<std::string> &lvals) {
	if (!context && !db) {
		return false;
	}
	try {
		SecretManager *sm = context ? &SecretManager::Get(*context) : &SecretManager::Get(*db);
		CatalogTransaction txn = context ? CatalogTransaction::GetSystemCatalogTransaction(*context)
		                                 : CatalogTransaction::GetSystemTransaction(*db);
		auto match = sm->LookupSecret(txn, url, scheme);
		if (!match.HasMatch()) {
			return false;
		}
		const auto &base = *match.secret_entry->secret;
		const auto &kv = dynamic_cast<const KeyValueSecret &>(base);
		for (auto &entry : kv.secret_map) {
			const std::string &k = entry.first;
			if (k == "__scheme") {
				continue;
			}
			std::string v = entry.second.ToString();
			if (k.rfind("layer.", 0) == 0) {
				lkeys.push_back(k.substr(6));
				lvals.push_back(v);
			} else {
				keys.push_back(k);
				vals.push_back(v);
			}
		}
		return true;
	} catch (...) {
		return false;
	}
}

OdopOperator *OpenDalFileSystem::OperatorFor(const std::string &scheme, const std::string &authority,
                                             const std::string &url, optional_ptr<FileOpener> opener) {
	auto context = FileOpener::TryGetClientContext(opener);
	auto db = FileOpener::TryGetDatabase(opener);
	return BuildOperator(scheme, authority, url, context, db);
}

OdopOperator *OpenDalFileSystem::OperatorForCtx(const std::string &scheme, const std::string &authority,
                                                const std::string &url,
                                                optional_ptr<ClientContext> context) {
	optional_ptr<DatabaseInstance> db;
	if (context) {
		db = &DatabaseInstance::GetDatabase(*context);
	}
	return BuildOperator(scheme, authority, url, context, db);
}

OdopOperator *OpenDalFileSystem::BuildOperator(const std::string &scheme, const std::string &authority,
                                               const std::string &url, optional_ptr<ClientContext> context,
                                               optional_ptr<DatabaseInstance> db) {
	std::string key = scheme + "://" + authority;

	// Fast path: return a cached operator without blocking other schemes.
	{
		std::lock_guard<std::mutex> lk(mu_);
		auto it = operators_.find(key);
		if (it != operators_.end()) {
			return it->second;
		}
	}

	// The operator URI: scheme://authority (no path). OpenDAL's per-service
	// from_uri parsing maps the authority to the right config key — s3 bucket,
	// gcs bucket, azblob container, etc. — so we do not special-case it here.
	// fs/memory have an empty authority. Per-call paths are passed relative to
	// the operator root (which stays default), unaffected by this URI.
	std::string uri = scheme + "://" + authority;

	std::vector<std::string> keys;
	std::vector<std::string> vals;
	std::vector<std::string> lkeys; // layer keys
	std::vector<std::string> lvals; // layer values

	// The local `fs` service treats the URI path as its root; we instead root it
	// at "/" and pass absolute paths per call (see AbsolutizeFsPath). Set it
	// explicitly as an override since the fs:// URI carries no path.
	if (scheme == "fs") {
		keys.push_back("root");
		vals.push_back("/");
	}

	// Merge a SCOPE-matched secret's config (if any). These override any config
	// OpenDAL parsed from the URI.
	bool have_secret = ApplySecret(context, db, scheme, url, keys, vals, lkeys, lvals);

	// Env fallback for object stores when no secret provided the credentials.
	if (scheme == "s3" && !have_secret) {
		auto add_env = [&](const char *env, const char *odal) {
			const char *v = std::getenv(env);
			if (v && *v) {
				keys.push_back(odal);
				vals.push_back(v);
			}
		};
		add_env("AWS_REGION", "region");
		add_env("AWS_ENDPOINT_URL", "endpoint");
		add_env("AWS_ACCESS_KEY_ID", "access_key_id");
		add_env("AWS_SECRET_ACCESS_KEY", "secret_access_key");
		add_env("AWS_SESSION_TOKEN", "session_token");
	}

	std::vector<const char *> key_ptrs, val_ptrs, lkey_ptrs, lval_ptrs;
	for (size_t i = 0; i < keys.size(); i++) {
		key_ptrs.push_back(keys[i].c_str());
		val_ptrs.push_back(vals[i].c_str());
	}
	for (size_t i = 0; i < lkeys.size(); i++) {
		lkey_ptrs.push_back(lkeys[i].c_str());
		lval_ptrs.push_back(lvals[i].c_str());
	}

	// Build the operator OUTSIDE the lock: odop_operator_new may do DNS/network
	// work, and holding mu_ across it would serialize operator creation for
	// unrelated schemes/buckets.
	OdopError err = {};
	OdopOperator *op = odop_operator_new(
	    uri.c_str(), key_ptrs.empty() ? nullptr : key_ptrs.data(),
	    val_ptrs.empty() ? nullptr : val_ptrs.data(), keys.size(),
	    lkey_ptrs.empty() ? nullptr : lkey_ptrs.data(), lval_ptrs.empty() ? nullptr : lval_ptrs.data(),
	    lkeys.size(), &err);
	if (!op) {
		ThrowIfError(err, "opendal: failed to create operator for '" + key + "'");
		throw IOException("opendal: null operator for '" + key + "'");
	}
	ClearError(err);

	// Publish under the lock. If another thread built the same key concurrently,
	// keep the first one and free ours (last-writer-loses, avoids a leak).
	std::lock_guard<std::mutex> lk(mu_);
	auto [it, inserted] = operators_.emplace(key, op);
	if (!inserted) {
		odop_operator_free(op);
		return it->second;
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
		OdopError werr = {};
		OdopWriter *writer = odop_writer_open(op, rel.c_str(), &werr);
		if (!writer) {
			ThrowIfError(werr, "opendal open (write): " + path);
			throw IOException("opendal: null writer for " + path);
		}
		ClearError(werr);
		return make_uniq<OpenDalFileHandle>(*this, path, flags, writer);
	}

	// Read mode: stat first to get size + type.
	OdopMetadata meta = {};
	OdopError serr = {};
	odop_stat(op, rel.c_str(), &meta, &serr);
	ThrowIfError(serr, "opendal stat: " + path);
	if (meta.is_dir) {
		throw IOException("opendal: cannot open directory as file: " + path);
	}

	OdopError rerr = {};
	OdopReader *reader = odop_reader_open(op, rel.c_str(), &rerr);
	if (!reader) {
		ThrowIfError(rerr, "opendal open: " + path);
		throw IOException("opendal: null reader for " + path);
	}
	ClearError(rerr);

	return make_uniq<OpenDalFileHandle>(*this, path, flags, reader, (int64_t)meta.content_length,
	                                    meta.last_modified_ms);
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
	OdopError err = {};
	if (odop_writer_write(h.writer, static_cast<const uint8_t *>(buffer), (uint64_t)nr_bytes, &err) != 0) {
		ThrowIfError(err, "opendal write: " + h.path);
		throw IOException("opendal write failed: " + h.path);
	}
	ClearError(err);
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
	OdopError err = {};
	if (odop_writer_close(h.writer, &err) != 0) {
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
	OdopError err = {};
	int64_t n = odop_reader_read(h.reader, (uint64_t)location, (uint64_t)nr_bytes,
	                             static_cast<uint8_t *>(buffer), &err);
	if (n < 0) {
		ThrowIfError(err, "opendal read: " + h.path);
		throw IOException("opendal read failed: " + h.path);
	}
	ClearError(err);
	// DuckDB's positioned Read must fill the whole buffer; a short read here is an error.
	if (n != nr_bytes) {
		throw IOException("opendal read: short read at offset " + std::to_string((int64_t)location) +
		                  " (" + std::to_string(n) + " of " + std::to_string(nr_bytes) + " bytes) for " +
		                  h.path);
	}
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

	OdopError err = {};
	int64_t n = odop_reader_read(h.reader, (uint64_t)h.position, (uint64_t)to_read,
	                             static_cast<uint8_t *>(buffer), &err);
	if (n < 0) {
		ThrowIfError(err, "opendal read: " + h.path);
		throw IOException("opendal read failed: " + h.path);
	}
	ClearError(err);
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
	OdopOperator *op;
	try {
		op = OperatorFor(scheme, auth, filename, opener);
	} catch (...) {
		return false;
	}
	OdopMetadata meta = {};
	OdopError err = {};
	odop_stat(op, rel.c_str(), &meta, &err);
	bool ok = (err.code == OdopErrorCode::Ok) && !meta.is_dir;
	ClearError(err);
	return ok;
}

bool OpenDalFileSystem::DirectoryExists(const string &directory, optional_ptr<FileOpener> opener) {
	std::string scheme, auth, rel;
	if (!ParseUrl(directory, scheme, auth, rel) || !IsSupportedScheme(scheme)) {
		return false;
	}
	OdopOperator *op;
	try {
		op = OperatorFor(scheme, auth, directory, opener);
	} catch (...) {
		return false;
	}
	OdopMetadata meta = {};
	OdopError err = {};
	odop_stat(op, rel.c_str(), &meta, &err);
	bool ok = (err.code == OdopErrorCode::Ok) && meta.is_dir;
	ClearError(err);
	return ok;
}

void OpenDalFileSystem::Seek(FileHandle &handle, idx_t location) {
	handle.Cast<OpenDalFileHandle>().position = (int64_t)location;
}

// List immediate children of a directory. Callback receives (name, is_dir).
bool OpenDalFileSystem::ListFiles(const string &directory,
                                  const std::function<void(const string &, bool)> &callback,
                                  FileOpener *opener) {
	std::string scheme, auth, rel;
	if (!ParseUrl(directory, scheme, auth, rel) || !IsSupportedScheme(scheme)) {
		return false;
	}
	OdopOperator *op;
	try {
		op = OperatorFor(scheme, auth, directory, opener);
	} catch (...) {
		return false;
	}
	// OpenDAL expects a trailing slash to list a directory's children.
	std::string dir = rel;
	if (dir.empty() || dir.back() != '/') {
		dir += "/";
	}

	OdopError err = {};
	OdopEntryList *list = odop_list(op, dir.c_str(), /*recursive=*/0, &err);
	if (!list) {
		ClearError(err);
		return false;
	}
	ClearError(err);

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
	odop_list_free(list);
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
		if (FileExists(path, opener)) {
			results.emplace_back(path);
		} else if (!have_ctx && SchemeNeedsSecret(scheme)) {
			results.emplace_back(path);
		}
		return results;
	}

	OdopOperator *op;
	try {
		op = OperatorFor(scheme, auth, path, opener);
	} catch (...) {
		return results;
	}

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

	OdopError err = {};
	OdopEntryList *list = odop_list(op, list_dir.c_str(), recursive ? 1 : 0, &err);
	if (!list) {
		ClearError(err);
		return results;
	}
	ClearError(err);

	size_t n = odop_list_len(list);
	for (size_t i = 0; i < n; i++) {
		OdopEntry ent = {};
		if (!odop_list_entry(list, i, &ent) || ent.is_dir) {
			continue;
		}
		std::string epath = ent.path ? std::string(ent.path) : std::string();
		std::string ename = ent.name ? std::string(ent.name) : std::string();
		if (glob_match(file_pat.c_str(), ename.c_str()) == 0) {
			results.emplace_back(BuildUrl(scheme, auth, epath));
		}
	}
	odop_list_free(list);

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
	// Only the local `fs` scheme is on-disk; everything else is treated as remote.
	std::string scheme, auth, rel;
	if (ParseUrl(handle.path, scheme, auth, rel)) {
		return scheme == "fs";
	}
	return false;
}

// ─── Mutations ──────────────────────────────────────────────────────────────
void OpenDalFileSystem::CreateDirectory(const string &directory, optional_ptr<FileOpener> opener) {
	std::string scheme, auth, rel;
	if (!ParseUrl(directory, scheme, auth, rel) || !IsSupportedScheme(scheme)) {
		throw IOException("opendal: unsupported or invalid URL: " + directory);
	}
	auto *op = OperatorFor(scheme, auth, directory, opener);
	OdopError err = {};
	if (odop_create_dir(op, rel.c_str(), &err) != 0) {
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
	OdopError err = {};
	if (odop_remove(op, rel.c_str(), /*recursive=*/0, &err) != 0) {
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
	OdopError err = {};
	if (odop_remove(op, dir.c_str(), /*recursive=*/1, &err) != 0) {
		ThrowIfError(err, "opendal remove (dir): " + directory);
		throw IOException("opendal remove (dir) failed: " + directory);
	}
	ClearError(err);
}

void OpenDalFileSystem::MoveFile(const string &source, const string &target,
                                 optional_ptr<FileOpener> opener) {
	std::string s_scheme, s_auth, s_rel, t_scheme, t_auth, t_rel;
	if (!ParseUrl(source, s_scheme, s_auth, s_rel) || !IsSupportedScheme(s_scheme)) {
		throw IOException("opendal: unsupported or invalid source URL: " + source);
	}
	if (!ParseUrl(target, t_scheme, t_auth, t_rel) || !IsSupportedScheme(t_scheme)) {
		throw IOException("opendal: unsupported or invalid target URL: " + target);
	}
	if (s_scheme != t_scheme || s_auth != t_auth) {
		throw IOException("opendal: move across schemes/buckets is not supported (" + source + " -> " +
		                  target + ")");
	}
	auto *op = OperatorFor(s_scheme, s_auth, source, opener);
	OdopError err = {};
	if (odop_rename(op, s_rel.c_str(), t_rel.c_str(), &err) != 0) {
		ThrowIfError(err, "opendal move: " + source + " -> " + target);
		throw IOException("opendal move failed: " + source + " -> " + target);
	}
	ClearError(err);
}

} // namespace duckdb
