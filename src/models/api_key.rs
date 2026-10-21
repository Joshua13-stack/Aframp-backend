use chrono::{DateTime, Utc};
use serde::Serialize;
use sqlx::FromRow;
use uuid::Uuid;

/// Planned infrastructure — not yet implemented.
///
/// The `api_keys` table (see `migrations/0001_init.sql`) and this model exist
/// to support API key authentication, but there are currently no endpoints to
/// create, list, or revoke API keys. `WEBHOOK_SECRET` is only used for signing
/// and is not used for API key authentication. See PRD.md for the planned
/// feature (including multi-environment API keys).
#[derive(Debug, Clone, Serialize, FromRow)]
pub struct ApiKey {
    pub id: Uuid,
    pub merchant_id: Uuid,
    pub key_prefix: String,
    pub environment: String,
    pub created_at: DateTime<Utc>,
    pub revoked_at: Option<DateTime<Utc>>,
}
