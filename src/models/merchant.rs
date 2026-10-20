use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::FromRow;
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, FromRow)]
pub struct Merchant {
    pub id: Uuid,
    pub user_id: Uuid,
    pub name: String,
    pub kyc_status: String,
    pub bvn: Option<String>,
    pub cac_number: Option<String>,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone)]
pub struct NewMerchant {
    pub user_id: Uuid,
    pub name: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct KycSubmission {
    pub bvn: Option<String>,
    pub cac_number: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct AdminKycUpdate {
    pub status: String,
}