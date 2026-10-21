use axum::extract::{Query, State};
use axum::http::HeaderMap;
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

/// Validate the `X-CSRF-Token` header on state-changing admin requests.
///
/// The dashboard embeds a per-session token in a `<meta name="csrf-token">`
/// tag and echoes it back on every mutating fetch. When `COOKIE_SAME_SITE=none`
/// the session cookie rides on cross-site requests, so this header check is what
/// stops a forged cross-site POST from reaching a mutating route.
fn require_csrf(headers: &HeaderMap, admin: &AdminUser) -> ApiResult<()> {
    let provided = headers
        .get("x-csrf-token")
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default();
    if provided.is_empty() || provided != admin.csrf_token {
        return Err(crate::error::ApiError::forbidden("invalid or missing CSRF token"));
    }
    Ok(())
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

/// Static dashboard shell. Unauthenticated by design — it's markup and JS with
/// no data baked in. It logs in through the normal `/login` endpoint (setting
/// the same HttpOnly session cookie a browser client would get) and every data
/// fetch after that rides the cookie same-origin; nothing here bypasses the
/// `AdminUser` check on the JSON routes below.
///
/// The shell also exposes the per-session CSRF token via a `<meta>` tag so the
/// dashboard JS can echo it back in the `X-CSRF-Token` header on mutating calls.
pub async fn dashboard(admin: AdminUser) -> Html<String> {
    let shell = include_str!("admin_dashboard.html");
    let meta = format!(
        "<meta name=\"csrf-token\" content=\"{}\">",
        admin.csrf_token
    );
    let html = shell.replacen("</head>", &format!("{}\n</head>", meta), 1);
    Html(html)
}
