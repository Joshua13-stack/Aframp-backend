-- Initial schema for the application.
--
-- NOTE: The `api_keys` table below is planned infrastructure. It is created
-- here so the schema is ready, but there are currently no API endpoints to
-- create, list, or revoke API keys, and the `ApiKey` model in
-- `src/models/api_key.rs` is not yet wired to any handlers. API key
-- authentication (including multi-environment keys) is a planned feature;
-- see PRD.md. Do not assume this table is in active use.

CREATE TABLE IF NOT EXISTS api_keys (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    name TEXT NOT NULL,
    key_hash TEXT NOT NULL UNIQUE,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    revoked_at TIMESTAMPTZ
);
