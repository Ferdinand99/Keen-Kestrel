-- Upload tokens move to vole-kit's token scopes (vole-kit 0.2).
--
-- 0004 kept Kestrel's own token_grants (difs:write: upload debug files and
-- manage releases with sentry-cli). The kit now has api_token_scopes
-- (core migration kit/0002_token_scopes.sql, which runs before this one)
-- and checks the scope itself (CurrentUser::has_scope). Every grant moves
-- over unchanged, so upload tokens keep working, and token_grants goes.
INSERT OR IGNORE INTO api_token_scopes (token_id, scope)
    SELECT token_id, scope FROM token_grants;
DROP TABLE token_grants;
