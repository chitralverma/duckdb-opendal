#include "opendal_filesystem.hpp"

#include "duckdb/common/exception.hpp"
#include "duckdb/common/types/timestamp.hpp"

#include <cstring>
#include <unistd.h>

namespace duckdb {

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

OpenDalFileHandle::~OpenDalFileHandle() {
	Close();
}

void OpenDalFileHandle::Close() {
	if (reader) {
		odop_reader_free(reader);
		reader = nullptr;
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

// scheme://rest  →  scheme + OpenDAL-relative path.
// For fs:///tmp/x  → scheme="fs", path="/tmp/x".
// For memory://foo → scheme="memory", path="foo".
bool OpenDalFileSystem::ParseUrl(const std::string &url, std::string &out_scheme,
                                 std::string &out_path) {
	auto pos = url.find("://");
	if (pos == std::string::npos) {
		return false;
	}
	out_scheme = url.substr(0, pos);
	if (out_scheme.empty()) {
		return false;
	}
	out_path = url.substr(pos + 3);
	if (out_path.empty()) {
		out_path = "/";
	}
	// The `fs` operator uses root "/", so resolve relative paths against CWD.
	if (out_scheme == "fs") {
		out_path = AbsolutizeFsPath(out_path);
	}
	return true;
}

// Schemes this Phase-1 build serves. (Expanded per-service in later phases.)
static bool IsSupportedScheme(const std::string &scheme) {
	return scheme == "fs" || scheme == "memory";
}

OdopOperator *OpenDalFileSystem::OperatorForScheme(const std::string &scheme) {
	std::lock_guard<std::mutex> lk(mu_);
	auto it = operators_.find(scheme);
	if (it != operators_.end()) {
		return it->second;
	}

	// Build config for the scheme. For `fs`, root="/" so absolute paths resolve.
	std::vector<std::string> keys;
	std::vector<std::string> vals;
	if (scheme == "fs") {
		keys.push_back("root");
		vals.push_back("/");
	}

	std::vector<const char *> key_ptrs;
	std::vector<const char *> val_ptrs;
	for (size_t i = 0; i < keys.size(); i++) {
		key_ptrs.push_back(keys[i].c_str());
		val_ptrs.push_back(vals[i].c_str());
	}

	OdopError err = {};
	OdopOperator *op = odop_operator_new(scheme.c_str(), key_ptrs.empty() ? nullptr : key_ptrs.data(),
	                                     val_ptrs.empty() ? nullptr : val_ptrs.data(), keys.size(), &err);
	if (!op) {
		ThrowIfError(err, "opendal: failed to create operator for scheme '" + scheme + "'");
		throw IOException("opendal: null operator for scheme '" + scheme + "'");
	}
	ClearError(err);
	operators_[scheme] = op;
	return op;
}

bool OpenDalFileSystem::CanHandleFile(const string &path) {
	std::string scheme, rel;
	if (!ParseUrl(path, scheme, rel)) {
		return false;
	}
	return IsSupportedScheme(scheme);
}

unique_ptr<FileHandle> OpenDalFileSystem::OpenFile(const string &path, FileOpenFlags flags,
                                                   optional_ptr<FileOpener>) {
	// Phase 1: read only.
	if (flags.OpenForWriting()) {
		throw IOException("opendal: write support not yet implemented (Phase 1 is read-only)");
	}

	std::string scheme, rel;
	if (!ParseUrl(path, scheme, rel)) {
		throw IOException("opendal: unrecognized URL: " + path);
	}
	auto *op = OperatorForScheme(scheme);

	// Stat first to get size + type.
	OdopMetadata meta = {};
	OdopError serr = {};
	odop_stat(op, rel.c_str(), &meta, &serr);
	ThrowIfError(serr, "opendal stat: " + path);
	if (meta.is_dir) {
		throw IOException("opendal: cannot open directory as file: " + path);
	}

	// Open a reader.
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
	// Handles are only opened for files in Phase 1.
	return FileType::FILE_TYPE_REGULAR;
}

bool OpenDalFileSystem::FileExists(const string &filename, optional_ptr<FileOpener>) {
	std::string scheme, rel;
	if (!ParseUrl(filename, scheme, rel) || !IsSupportedScheme(scheme)) {
		return false;
	}
	OdopOperator *op;
	try {
		op = OperatorForScheme(scheme);
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

bool OpenDalFileSystem::DirectoryExists(const string &directory, optional_ptr<FileOpener>) {
	std::string scheme, rel;
	if (!ParseUrl(directory, scheme, rel) || !IsSupportedScheme(scheme)) {
		return false;
	}
	OdopOperator *op;
	try {
		op = OperatorForScheme(scheme);
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

// Phase 1: resolve only exact paths (no wildcards). If the path has glob
// characters we return empty (true globbing lands with the lister in Phase 2).
// For a concrete path, return it iff it exists as a file.
vector<OpenFileInfo> OpenDalFileSystem::Glob(const string &path, FileOpener *) {
	vector<OpenFileInfo> results;
	std::string scheme, rel;
	if (!ParseUrl(path, scheme, rel) || !IsSupportedScheme(scheme)) {
		return results;
	}
	bool has_glob = path.find_first_of("*?[") != std::string::npos;
	if (has_glob) {
		// Not yet supported; surface as empty rather than throwing so callers
		// that tolerate no-match behave sanely.
		return results;
	}
	if (FileExists(path)) {
		results.emplace_back(path);
	}
	return results;
}

idx_t OpenDalFileSystem::SeekPosition(FileHandle &handle) {
	return (idx_t)handle.Cast<OpenDalFileHandle>().position;
}

bool OpenDalFileSystem::OnDiskFile(FileHandle &handle) {
	// Only the local `fs` scheme is on-disk; everything else is treated as remote.
	std::string scheme, rel;
	if (ParseUrl(handle.path, scheme, rel)) {
		return scheme == "fs";
	}
	return false;
}

} // namespace duckdb
