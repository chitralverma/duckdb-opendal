-- Init script for the fs:// backend (loaded by test/configs/fs.json).
-- The local filesystem backend needs a root; the common suite writes under
-- ${OPENDAL_BASE} = fs:///__TEST_DIR__/odfs (absolute), so root '.' suffices.
CREATE SECRET fs_common (TYPE fs, SCOPE 'fs://', config MAP{'root': '.'});
