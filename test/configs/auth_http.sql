-- Init script for the http:// service (loaded by test/configs/http.json).
--
-- The http service is read-only (stat + read; no write, list or delete), so
-- this config deliberately sets neither OPENDAL_BASE — the common suite seeds
-- itself by writing — nor OPENDAL_EXTERNAL_BASE, whose last assertion globs and
-- so needs list. The service-agnostic common tests still run; the read, stat and
-- capability coverage lives in test/sql/services/http.test.
--
-- http:// and https:// are reserved by DuckDB's native httpfs, so claim them
-- explicitly instead of depending on extension load order.
SET GLOBAL opendal_override_native_filesystems = 'http,https';

-- Anonymous reads need no secret at all: the endpoint comes from the URL. This
-- one only exists for the Basic-auth prefix, matched by its narrower scope.
CREATE SECRET http_protected (
    TYPE http, SCOPE '${OPENDAL_HTTP_PROTECTED_BASE}',
    config MAP{
        'username': '${OPENDAL_HTTP_USER}',
        'password': '${OPENDAL_HTTP_PASSWORD}'
    }
);
