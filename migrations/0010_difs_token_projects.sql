-- Debug-file upload tokens are bound to one project.
--
-- A token with the difs:write scope (api_token_scopes, kit) used to upload to
-- and manage releases in every project. The binding lives here, keyed by the
-- token: project_id is the one project it may use, NULL means every project.
-- A difs:write token without a row here (one made on the API tokens page,
-- which has no project field, or whose project was deleted: the row goes with
-- it) is refused until a new token is made, so a missing binding never widens
-- access.
--
-- Tokens that exist keep working: they become "all projects" tokens.
CREATE TABLE difs_token_projects (
    token_id INTEGER PRIMARY KEY REFERENCES api_tokens(id) ON DELETE CASCADE,
    project_id INTEGER REFERENCES projects(id) ON DELETE CASCADE
);

INSERT INTO difs_token_projects (token_id, project_id)
    SELECT token_id, NULL FROM api_token_scopes WHERE scope = 'difs:write';
