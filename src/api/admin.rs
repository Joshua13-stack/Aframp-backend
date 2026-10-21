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
use crate::pagination::{Cursor, Page};
use crate::services::admin;
use crate::AppState;

#[derive(Deserialize)]
pub struct ListParams {
    pub limit: Option<i64>,
    pub cursor: Option<String>,
}

fn limit(params: &ListParams) -> i64 {
    params.limit.unwrap_or(100).clamp(1, 500)
}

fn cursor(params: &ListParams) -> ApiResult<Option<Cursor>> {
    match params.cursor.as_deref() {
        Some(raw) => Cursor::decode(raw)
            .map(Some)
            .map_err(|_| crate::error::ApiError::bad_request("invalid cursor")),
        None => Ok(None),
    }
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
) -> ApiResult<Json<Page<AdminUserRow>>> {
    let page = admin::users(&state.db, limit(&params), cursor(&params)?)
        .await
        .map_err(internal)?;
    Ok(Json(page))
}

pub async fn merchants(
    State(state): State<AppState>,
    _admin: AdminUser,
    Query(params): Query<ListParams>,
) -> ApiResult<Json<Page<AdminMerchantRow>>> {
    let page = admin::merchants(&state.db, limit(&params), cursor(&params)?)
        .await
        .map_err(internal)?;
    Ok(Json(page))
}

pub async fn wallets(
    State(state): State<AppState>,
    _admin: AdminUser,
    Query(params): Query<ListParams>,
) -> ApiResult<Json<Page<AdminWalletRow>>> {
    let page = admin::wallets(&state.db, limit(&params), cursor(&params)?)
        .await
        .map_err(internal)?;
    Ok(Json(page))
}

pub async fn transactions(
    State(state): State<AppState>,
    _admin: AdminUser,
    Query(params): Query<ListParams>,
) -> ApiResult<Json<Page<AdminTransactionRow>>> {
    let page = admin::transactions(&state.db, limit(&params), cursor(&params)?)
        .await
        .map_err(internal)?;
    Ok(Json(page))
}

pub async fn withdrawals(
    State(state): State<AppState>,
    _admin: AdminUser,
    Query(params): Query<ListParams>,
) -> ApiResult<Json<Page<AdminWithdrawalRow>>> {
    let page = admin::withdrawals(&state.db, limit(&params), cursor(&params)?)
        .await
        .map_err(internal)?;
    Ok(Json(page))
}

pub async fn payment_requests(
    State(state): State<AppState>,
    _admin: AdminUser,
    Query(params): Query<ListParams>,
) -> ApiResult<Json<Page<AdminPaymentRequestRow>>> {
    let page = admin::payment_requests(&state.db, limit(&params), cursor(&params)?)
        .await
        .map_err(internal)?;
    Ok(Json(page))
}

/// Static dashboard shell. Unauthenticated by design — it's markup and JS with
/// no data baked in. It logs in through the normal `/login` endpoint (setting
/// the same HttpOnly session cookie a browser client would get) and every data
/// fetch after that rides the cookie same-origin; nothing here bypasses the
/// `AdminUser` check on the JSON routes below.
pub async fn dashboard() -> Html<&'static str> {
    Html(include_str!("admin_dashboard.html"))
}
