use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::FromRow;
use uuid::Uuid;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, sqlx::Type)]
#[serde(rename_all = "snake_case")]
#[sqlx(type_name = "TEXT", rename_all = "snake_case")]
pub enum KycStatus {
    Unverified,
    Pending,
    Verified,
    Rejected,
}

impl Default for KycStatus {
    fn default() -> Self {
        KycStatus::Unverified
    }
}

#[derive(Debug, Clone, Serialize, FromRow)]
pub struct Merchant {
    pub id: Uuid,
    pub user_id: Uuid,
    pub name: String,
    pub kyc_status: KycStatus,
    pub kyc_bvn: Option<String>,
    pub kyc_cac_number: Option<String>,
    pub kyc_submitted_at: Option<DateTime<Utc>>,
    pub kyc_reviewed_at: Option<DateTime<Utc>>,
    pub kyc_rejection_reason: Option<String>,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone)]
pub struct NewMerchant {
    pub user_id: Uuid,
    pub name: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct SubmitKycRequest {
    pub bvn: Option<String>,
    pub cac_number: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct AdminKycDecisionRequest {
    pub status: KycStatus,
    pub rejection_reason: Option<String>,
}