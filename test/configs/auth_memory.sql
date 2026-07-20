-- Init script for the memory:// service (loaded by test/configs/memory.json).
--
-- NOTE: this is OPTIONAL. The in-process memory service needs no configuration,
-- so memory:// works with no secret at all (memory-secret registration itself is
-- covered by test/sql/common/secret_config.test). It is kept only for symmetry
-- with the other services' auth_<svc>.sql, so every config has one init script.
CREATE SECRET memory_common (TYPE memory, SCOPE 'memory://');
