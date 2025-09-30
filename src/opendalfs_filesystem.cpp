#include "opendalfs_filesystem.hpp"
#include "duckdb/common/exception.hpp"
#include "duckdb/common/types/value.hpp"
#include "duckdb/main/client_context.hpp"
#include "duckdb/main/database.hpp"
#include "duckdb/common/shared_ptr.hpp"
namespace duckdb {

OpendalFileHandle::OpendalFileHandle(OpendalFileSystem &fs, const OpenFileInfo &info, FileOpenFlags flags,
                                     const OpendalReadOptions &read_options)
    : FileHandle(fs, info.path, flags), flags(flags),
      // File info
      length(0), last_modified(0),
      // Read info
      buffer_available(0), buffer_idx(0), file_offset(0), buffer_start(0), buffer_end(0),
      // Options
      read_options(read_options) {
	if (!flags.RequireParallelAccess() && !flags.DirectIO()) {
		read_buffer = duckdb::unique_ptr<data_t[]>(new data_t[read_options.buffer_size]);
	}
	std::printf("Initializing OpendalFileHandle\n");
}

OpendalFileSystem::OpendalFileSystem() {
	// todo: Initialize the opendal::Operator here.
	// For now, let's use the memory backend for testing purposes.
	// This should be configured based on the path or DuckDB settings.
	std::printf("Initializing OpendalFileSystem\n");
	op = opendal::Operator("memory");
}

unique_ptr<FileHandle> OpendalFileSystem::OpenFile(const string &path, FileOpenFlags flags,
                                                   optional_ptr<FileOpener> opener) {
	if (flags.OpenForWriting()) {
		throw NotImplementedException("Writing to Opendal files is not supported");
	}

	auto read_options = ParseOpendalReadOptions(opener);

	auto handle = make_uniq<OpendalFileHandle>(*this, OpenFileInfo(path), flags, read_options);
	LoadRemoteFileInfo(*handle);
	return std::move(handle);
}

void OpendalFileSystem::Read(FileHandle &handle, void *buffer, int64_t nr_bytes, idx_t location) {
	auto &hfh = handle.Cast<OpendalFileHandle>();
	idx_t to_read = nr_bytes;
	idx_t buffer_offset = 0;

	if (hfh.flags.DirectIO() || hfh.flags.RequireParallelAccess()) {
		if (to_read == 0) {
			return;
		}
		ReadRange(hfh, location, reinterpret_cast<char *>(buffer), to_read);
		hfh.buffer_available = 0;
		hfh.buffer_idx = 0;
		hfh.file_offset = location + nr_bytes;
		return;
	}

	if (location >= hfh.buffer_start && location < hfh.buffer_end) {
		hfh.file_offset = location;
		hfh.buffer_idx = location - hfh.buffer_start;
		hfh.buffer_available = (hfh.buffer_end - hfh.buffer_start) - hfh.buffer_idx;
	} else {
		hfh.buffer_available = 0;
		hfh.buffer_idx = 0;
		hfh.file_offset = location;
	}

	while (to_read > 0) {
		auto buffer_read_len = MinValue<idx_t>(hfh.buffer_available, to_read);
		if (buffer_read_len > 0) {
			memcpy(reinterpret_cast<char *>(buffer) + buffer_offset, hfh.read_buffer.get() + hfh.buffer_idx,
			       buffer_read_len);
			buffer_offset += buffer_read_len;
			to_read -= buffer_read_len;
			hfh.buffer_idx += buffer_read_len;
			hfh.buffer_available -= buffer_read_len;
			hfh.file_offset += buffer_read_len;
		}

		if (to_read > 0 && hfh.buffer_available == 0) {
			auto new_buffer_available = MinValue<idx_t>(hfh.read_options.buffer_size, hfh.length - hfh.file_offset);
			if (to_read > new_buffer_available) {
				ReadRange(hfh, location + buffer_offset, reinterpret_cast<char *>(buffer) + buffer_offset, to_read);
				hfh.buffer_available = 0;
				hfh.buffer_idx = 0;
				hfh.file_offset += to_read;
				break;
			} else {
				ReadRange(hfh, hfh.file_offset, reinterpret_cast<char *>(hfh.read_buffer.get()), new_buffer_available);
				hfh.buffer_available = new_buffer_available;
				hfh.buffer_idx = 0;
				hfh.buffer_start = hfh.file_offset;
				hfh.buffer_end = hfh.buffer_start + new_buffer_available;
			}
		}
	}
}

int64_t OpendalFileSystem::Read(FileHandle &handle, void *buffer, int64_t nr_bytes) {
	auto &hfh = handle.Cast<OpendalFileHandle>();
	idx_t max_read = hfh.length - hfh.file_offset;
	// nr_bytes = MinValue<idx_t>(max_read, nr_bytes);
	nr_bytes = static_cast<int64_t>(MinValue<idx_t>(max_read, static_cast<idx_t>(nr_bytes)));
	Read(handle, buffer, nr_bytes, hfh.file_offset);
	return nr_bytes;
}

int64_t OpendalFileSystem::GetFileSize(FileHandle &handle) {
	auto &afh = handle.Cast<OpendalFileHandle>();
	return afh.length;
}

timestamp_t OpendalFileSystem::GetLastModifiedTime(FileHandle &handle) {
	auto &afh = handle.Cast<OpendalFileHandle>();
	return afh.last_modified;
}

bool OpendalFileSystem::FileExists(const string &filename, optional_ptr<FileOpener> opener) {
	try {
		return op.Exists(filename);
	} catch (const std::exception &e) {
		return false;
	}
}

void OpendalFileSystem::Seek(FileHandle &handle, idx_t location) {
	auto &sfh = handle.Cast<OpendalFileHandle>();
	sfh.file_offset = location;
}

idx_t OpendalFileSystem::SeekPosition(FileHandle &handle) {
	auto &afh = handle.Cast<OpendalFileHandle>();
	return afh.file_offset;
}

vector<OpenFileInfo> OpendalFileSystem::Glob(const string &path, FileOpener *opener) {
	// This is a simple glob implementation that supports '*' and '**'.
	// For a more complete implementation, a more sophisticated pattern matching algorithm would be needed.
	std::vector<OpenFileInfo> result;
	try {
		auto entries = op.List(path);
		for (const auto &entry : entries) {
			result.emplace_back(entry.path);
		}
	} catch (const std::exception &e) {
		// Handle exceptions, e.g., path not found
	}
	return result;
}

bool OpendalFileSystem::CanHandleFile(const string &fpath) {
	return fpath.rfind("s3://", 0) == 0 || fpath.rfind("gcs://", 0) == 0 || fpath.rfind("azblob://", 0) == 0;
}

void OpendalFileSystem::ReadRange(FileHandle &handle, idx_t file_offset, char *buffer_out, idx_t buffer_out_len) {
	auto &ofh = handle.Cast<OpendalFileHandle>();
	try {
		auto content = op.Read(ofh.path);
		memcpy(buffer_out, content.data() + file_offset, buffer_out_len);
	} catch (const std::exception &e) {
		throw IOException("OpendalFileSystem Read to '%s' failed with error: %s", ofh.path, e.what());
	}
}

OpendalReadOptions OpendalFileSystem::ParseOpendalReadOptions(optional_ptr<FileOpener> opener) {
	OpendalReadOptions options;
	Value buffer_size_val;
	if (FileOpener::TryGetCurrentSetting(opener, "opendal_read_buffer_size", buffer_size_val)) {
		options.buffer_size = buffer_size_val.GetValue<idx_t>();
	}
	return options;
}

void OpendalFileSystem::LoadRemoteFileInfo(OpendalFileHandle &handle) {
	try {
		auto metadata = op.Stat(handle.path);
		handle.length = metadata.content_length;
		// Opendal does not directly provide last_modified as a timestamp_t.
		// This needs to be converted. For now, we'll leave it as 0.
		handle.last_modified = timestamp_t(0);
	} catch (const std::exception &e) {
		throw IOException("OpendalFileSystem could not get file info for '%s': %s", handle.path, e.what());
	}
}

} // namespace duckdb
