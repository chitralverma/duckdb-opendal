#pragma once

#include "duckdb/common/file_system.hpp"
#include "duckdb/common/file_opener.hpp"
// #include "duckdb/common/shared_ptr.hpp"
#include "duckdb/common/unique_ptr.hpp"
// #include "duckdb/main/client_context_state.hpp"
#include "opendal.hpp"

namespace duckdb {

struct OpendalReadOptions {
	idx_t buffer_size = 1ULL * 1024 * 1024;
};

class OpendalFileSystem;

class OpendalFileHandle : public FileHandle {
public:
	~OpendalFileHandle() override = default;
	void Close() override {
	}

	OpendalFileHandle(OpendalFileSystem &fs, const OpenFileInfo &info, FileOpenFlags flags,
	                const OpendalReadOptions &read_options);

public:
	FileOpenFlags flags;

	// File info
	idx_t length;
	timestamp_t last_modified;

	// Read buffer
	duckdb::unique_ptr<data_t[]> read_buffer;
	// Read info
	idx_t buffer_available;
	idx_t buffer_idx;
	idx_t file_offset;
	idx_t buffer_start;
	idx_t buffer_end;

	const OpendalReadOptions read_options;
};

class OpendalFileSystem : public FileSystem {
public:
	OpendalFileSystem();

	duckdb::unique_ptr<FileHandle> OpenFile(const string &path, FileOpenFlags flags,
	                                        optional_ptr<FileOpener> opener = nullptr) override;

	void Read(FileHandle &handle, void *buffer, int64_t nr_bytes, idx_t location) override;
	int64_t Read(FileHandle &handle, void *buffer, int64_t nr_bytes) override;
	int64_t GetFileSize(FileHandle &handle) override;
	timestamp_t GetLastModifiedTime(FileHandle &handle) override;
	bool FileExists(const string &filename, optional_ptr<FileOpener> opener = nullptr) override;
	void Seek(FileHandle &handle, idx_t location) override;
	idx_t SeekPosition(FileHandle &handle) override;
	vector<OpenFileInfo> Glob(const string &path, FileOpener *opener = nullptr) override;
	bool CanHandleFile(const string &fpath) override;
	bool CanSeek() override {
		return true;
	}
	bool OnDiskFile(FileHandle &handle) override {
		return false;
	}

	string GetName() const override {
		return "OpendalFileSystem";
	}

private:
	void ReadRange(FileHandle &handle, idx_t file_offset, char *buffer_out, idx_t buffer_out_len);
	static OpendalReadOptions ParseOpendalReadOptions(optional_ptr<FileOpener> opener);
	void LoadRemoteFileInfo(OpendalFileHandle &handle);

	opendal::Operator op;
};

} // namespace duckdb
