use axum::extract::{Query, State};
use axum::response::Html;
use axum::Json;
use serde::Deserialize;

use crate::auth::extractor::AdminUser;
use crate::error::{internal, ApiResult};
use crate::models::{
    AdminMerchantRow, AdminOverview, AdminPaymentRequestRow, AdminTransactionRow, AdminUserRow,
    AdminWalletRow, AdminWithdrawalRow,
};
use crate::services::admin;
use crate::AppState;

#[derive(Deserialize)]
pub struct ListParams {
    pub limit: Option<i64>,
}

fn limit(params: &ListParams) -> i64 {
    params.limit.unwrap_or(100).clamp(1, 500)
}

/// Financial data (balances, withdrawals) is restricted to `super_admin` and
/// `finance` admins. Other admin roles get a 403.
fn require_finance(admin: &AdminUser) -> ApiResult<()> {
    if admin.has_role(&["super_admin", "finance"]) {
        Ok(())
    } else {
        Err(crate::error::ApiError::Forbidden)
    }
}

/// PII-heavy data (user emails) is restricted to `super_admin` and `support`
/// admins. Other admin roles get a 403.
fn require_pii(admin: &AdminUser) -> ApiResult<()> {
    if admin.has_role(&["super_admin", "support"]) {
        Ok(())
    } else {
        Err(crate::error::ApiError::Forbidden)
    }
}

pub async fn overview(
    State(state): State<AppState>,
    admin: AdminUser,
) -> ApiResult<Json<AdminOverview>> {
    require_finance(&admin)?;
    let overview = admin::overview(&state.db).await.map_err(internal)?;
    Ok(Json(overview))
}

pub async fn users(
    State(state): State<AppState>,
    admin: AdminUser,
    Query(params): Query<ListParams>,
) -> ApiResult<Json<Vec<AdminUserRow>>> {
    require_pii(&admin)?;
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
    admin: AdminUser,
    Query(params): Query<ListParams>,
) -> ApiResult<Json<Vec<AdminWithdrawalRow>>> {
    require_finance(&admin)?;
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

/// Static dashboard shell. Unauthenticated by design — it's markup and JS with
/// no data baked in. It logs in through the normal `/login` endpoint (setting
/// the same HttpOnly session cookie a browser client would get) and every data
/// fetch after that rides the cookie same-origin; nothing here bypasses the
/// `AdminUser` check on the JSON routes below.
pub async fn dashboard() -> Html<&'static str> {
    Html(include_str!("admin_dashboard.html"))
}
