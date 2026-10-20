use axum::extract::{Path, Query, State};
use axum::response::Html;
use axum::Json;
use serde::Deserialize;

use crate::auth::extractor::{AdminUser, AuthUser};
use crate::error::{bad_request, forbidden, internal, not_found, ApiResult};
use crate::models::{
    AdminMerchantRow, AdminOverview, AdminPaymentRequestRow, AdminTransactionRow, AdminUserRow,
    AdminWalletRow, AdminWithdrawalRow, Merchant,
};
use crate::services::admin;
use crate::services::merchant;
use crate::AppState;

#[derive(Deserialize)]
pub struct ListParams {
    pub limit: Option<i64>,
}

fn limit(params: &ListParams) -> i64 {
    params.limit.unwrap_or(100).clamp(1, 500)
}

pub async fn overview(
    State(state): State<AppState>,
    _admin: AdminUser,
) -> ApiResult<Json<AdminOverview>> {
    let overview = admin::overview(&state.db).await.map_err(internal)?;
    Ok(Json(overview))
}

pub async fn users(
    State(state): State<AppState>,
    _admin: AdminUser,
    Query(params): Query<ListParams>,
) -> ApiResult<Json<Vec<AdminUserRow>>> {
    let rows = admin::users(&state.db, limit(&params)).await.map_err(internal)?;
    Ok(Json(rows))
}

pub async fn merchants(
    State(state): State<AppState>,
    _admin: AdminUser,
    Query(params): Query<ListParams>,
) -> ApiResult<Json<Vec<AdminMerchantRow>>> {
    let rows = admin::merchants(&state.db, limit(&params)).await.map_err(internal)?;
    Ok(Json(rows))
}

pub async fn wallets(
    State(state): State<AppState>,
    _admin: AdminUser,
    Query(params): Query<ListParams>,
) -> ApiResult<Json<Vec<AdminWalletRow>>> {
    let rows = admin::wallets(&state.db, limit(&params)).await.map_err(internal)?;
    Ok(Json(rows))
}

pub async fn transactions(
    State(state): State<AppState>,
    _admin: AdminUser,
    Query(params): Query<ListParams>,
) -> ApiResult<Json<Vec<AdminTransactionRow>>> {
    let rows = admin::transactions(&state.db, limit(&params)).await.map_err(internal)?;
    Ok(Json(rows))
}

pub async fn withdrawals(
    State(state): State<AppState>,
    _admin: AdminUser,
    Query(params): Query<ListParams>,
) -> ApiResult<Json<Vec<AdminWithdrawalRow>>> {
    let rows = admin::withdrawals(&state.db, limit(&params)).await.map_err(internal)?;
    Ok(Json(rows))
}

pub async fn payment_requests(
    State(state): State<AppState>,
    _admin: AdminUser,
    Query(params): Query<ListParams>,
) -> ApiResult<Json<Vec<AdminPaymentRequestRow>>> {
    let rows = admin::payment_requests(&state.db, limit(&params))
        .await
        .map_err(internal)?;
    Ok(Json(rows))
}

#[derive(Deserialize)]
pub struct KycSubmission {
    pub bvn: Option<String>,
    pub cac_number: Option<String>,
}

/// `PUT /merchants/kyc` — a merchant submits basic KYC info. On success the
/// merchant's `kyc_status` moves to `pending` and the updated record is
/// returned.
pub async fn submit_kyc(
    State(state): State<AppState>,
    auth: AuthUser,
    Json(payload): Json<KycSubmission>,
) -> ApiResult<Json<Merchant>> {
    let merchant_id = auth
        .merchant_id
        .ok_or_else(|| forbidden("only merchants can submit KYC"))?;

    let bvn = payload
        .bvn
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty());
    let cac_number = payload
        .cac_number
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty());

    if bvn.is_none() && cac_number.is_none() {
        return Err(bad_request("provide at least a BVN or CAC number"));
    }

    if let Some(bvn) = bvn {
        if bvn.len() != 11 || !bvn.chars().all(|c| c.is_ascii_digit()) {
            return Err(bad_request("BVN must be exactly 11 digits"));
        }
    }

    if let Some(cac_number) = cac_number {
        if cac_number.len() < 2 || cac_number.len() > 32 {
            return Err(bad_request("CAC number must be between 2 and 32 characters"));
        }
        if !cac_number
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '/')
        {
            return Err(bad_request("CAC number contains invalid characters"));
        }
    }

    let merchant = merchant::submit_kyc(&state.db, merchant_id, bvn, cac_number)
        .await
        .map_err(internal)?;
    Ok(Json(merchant))
}

#[derive(Deserialize)]
pub struct KycDecision {
    pub status: String,
}

/// `PATCH /admin/merchants/{id}/kyc` — an admin approves or rejects a
/// merchant's KYC submission.
pub async fn update_merchant_kyc(
    State(state): State<AppState>,
    _admin: AdminUser,
    Path(id): Path<i64>,
    Json(payload): Json<KycDecision>,
) -> ApiResult<Json<Merchant>> {
    let status = payload.status.trim().to_ascii_lowercase();
    if status != "verified" && status != "rejected" {
        return Err(bad_request("status must be 'verified' or 'rejected'"));
    }

    let merchant = merchant::set_kyc_status(&state.db, id, &status)
        .await
        .map_err(internal)?
        .ok_or_else(|| not_found("merchant not found"))?;
    Ok(Json(merchant))
}

/// Static dashboard shell. Unauthenticated by design — it's markup and JS with
/// no data baked in. It logs in through the normal `/login` endpoint (setting
/// the same HttpOnly session cookie a browser client would get) and every data
/// fetch after that rides the cookie same-origin; nothing here bypasses the
/// `AdminUser` check on the JSON routes below.
pub async fn dashboard() -> Html<&'static str> {
    Html(include_str!("admin_dashboard.html"))
}
