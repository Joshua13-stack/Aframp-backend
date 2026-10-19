use axum::extract::State;
use axum::Json;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::auth::extractor::AuthUser;
use crate::error::{bad_request, internal, not_found, ApiResult, ErrorCode};
use crate::services::users;
use crate::AppState;

/// KYC verification status for a merchant.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum KycStatus {
    Unverified,
    Pending,
    Verified,
    Rejected,
}

impl KycStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            KycStatus::Unverified => "unverified",
            KycStatus::Pending => "pending",
            KycStatus::Verified => "verified",
            KycStatus::Rejected => "rejected",
        }
    }

    pub fn from_str(s: &str) -> Option<Self> {
        match s {
            "unverified" => Some(KycStatus::Unverified),
            "pending" => Some(KycStatus::Pending),
            "verified" => Some(KycStatus::Verified),
            "rejected" => Some(KycStatus::Rejected),
            _ => None,
        }
    }
}

/// The authenticated merchant's own profile. The JWT only carries ids, so a
/// frontend that reloads with a stored token needs this to render anything
/// human-readable ("signed in as …") without forcing a re-login.
#[derive(Serialize)]
pub struct MeView {
    pub user_id: Uuid,
    pub email: String,
    pub name: String,
    pub is_admin: bool,
    pub created_at: DateTime<Utc>,
    pub merchant_id: Option<Uuid>,
    pub merchant_name: Option<String>,
    pub merchant_kyc_status: Option<KycStatus>,
}

pub async fn get(State(state): State<AppState>, auth: AuthUser) -> ApiResult<Json<MeView>> {
    let user = users::user_by_id(&state.db, auth.user_id)
        .await
        .map_err(internal)?
        .ok_or_else(|| not_found(ErrorCode::UserNotFound, "user not found"))?;

    let merchant = users::merchant_by_user(&state.db, auth.user_id)
        .await
        .map_err(internal)?;

    Ok(Json(MeView {
        user_id: user.id,
        email: user.email,
        name: user.name,
        is_admin: user.is_admin,
        created_at: user.created_at,
        merchant_id: merchant.as_ref().map(|m| m.id),
        merchant_name: merchant.as_ref().map(|m| m.name.clone()),
        merchant_kyc_status: merchant
            .as_ref()
            .and_then(|m| KycStatus::from_str(&m.kyc_status)),
    }))
}

/// Request body for a merchant submitting KYC information.
#[derive(Deserialize)]
pub struct SubmitKycRequest {
    pub bvn: Option<String>,
    pub cac_number: Option<String>,
    pub business_name: Option<String>,
}

/// Response returned after a merchant submits KYC information.
#[derive(Serialize)]
pub struct KycView {
    pub merchant_id: Uuid,
    pub merchant_name: String,
    pub kyc_status: KycStatus,
}

/// `PUT /merchants/kyc` — a merchant submits basic KYC info. The merchant's
/// status transitions to `pending` awaiting admin review.
pub async fn submit_kyc(
    State(state): State<AppState>,
    auth: AuthUser,
    Json(req): Json<SubmitKycRequest>,
) -> ApiResult<Json<KycView>> {
    let merchant = users::merchant_by_user(&state.db, auth.user_id)
        .await
        .map_err(internal)?
        .ok_or_else(|| not_found(ErrorCode::MerchantNotFound, "merchant not found"))?;

    if req.bvn.is_none() && req.cac_number.is_none() && req.business_name.is_none() {
        return Err(bad_request(
            ErrorCode::InvalidRequest,
            "at least one KYC field (bvn, cac_number, business_name) is required",
        ));
    }

    let updated = users::submit_merchant_kyc(
        &state.db,
        merchant.id,
        req.bvn.as_deref(),
        req.cac_number.as_deref(),
        req.business_name.as_deref(),
    )
    .await
    .map_err(internal)?
    .ok_or_else(|| not_found(ErrorCode::MerchantNotFound, "merchant not found"))?;

    let kyc_status = KycStatus::from_str(&updated.kyc_status).unwrap_or(KycStatus::Unverified);

    Ok(Json(KycView {
        merchant_id: updated.id,
        merchant_name: updated.name,
        kyc_status,
    }))
}