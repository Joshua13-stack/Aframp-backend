use axum::extract::{Path, Query, State};
use axum::Json;
use chrono::{DateTime, Utc};
use serde::Serialize;
use uuid::Uuid;

use crate::auth::extractor::AuthUser;
use crate::error::{bad_request, internal, not_found, ApiResult, ErrorCode};
use crate::models::{CreatePaymentRequestRequest, PaymentRequest};
use crate::pagination::{Cursor, Page};
use crate::services::{payment_requests, wallets};
use crate::AppState;

#[derive(Serialize)]
pub struct PaymentRequestView {
    pub id: Uuid,
    pub merchant_id: Uuid,
    pub address: String,
    pub network: String,
    pub amount_stroops: i64,
    pub asset: String,
    pub memo: String,
    pub status: String,
    pub expires_at: DateTime<Utc>,
    pub created_at: DateTime<Utc>,
    /// SEP-0007 payment URI a Stellar wallet can open directly to pay this
    /// request. `None` for credit assets we don't have a real issuer address
    /// configured for yet (see PRD §9.4) — we don't guess one.
    pub sep7_uri: Option<String>,
}

pub async fn create(
    State(state): State<AppState>,
    auth: AuthUser,
    Json(req): Json<CreatePaymentRequestRequest>,
) -> ApiResult<Json<PaymentRequestView>> {
    let merchant_id = auth
        .merchant_id
        .ok_or_else(|| bad_request(ErrorCode::MerchantNotFound, "no merchant associated with this account"))?;

    let wallet = wallets::wallet_by_merchant(&state.db, merchant_id)
        .await
        .map_err(internal)?
        .ok_or_else(|| bad_request(ErrorCode::WalletNotFound, "create a wallet before generating payment requests"))?;

    // Defaults to XLM, not cNGN like withdrawals: XLM is what's actually
    // scannable/testable today (no cNGN issuer address configured yet).
    let asset = req.asset.unwrap_or_else(|| "XLM".into());

    let pr = payment_requests::create_payment_request(
        &state.db,
        merchant_id,
        wallet.id,
        req.amount_stroops,
        asset,
        req.expires_in_secs,
    )
    .await
    .map_err(map_payment_request_error)?;

    Ok(Json(to_view(&pr, &wallet.address, &wallet.network)))
}

pub async fn get(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> ApiResult<Json<PaymentRequestView>> {
    let pr = payment_requests::payment_request_by_id(&state.db, id)
        .await
        .map_err(internal)?
        .ok_or_else(|| not_found(ErrorCode::PaymentRequestNotFound, "payment request not found"))?;

    let wallet = wallets::wallet_by_id(&state.db, pr.wallet_id)
        .await
        .map_err(internal)?
        .ok_or_else(|| internal("payment request references a missing wallet"))?;

    Ok(Json(to_view(&pr, &wallet.address, &wallet.network)))
}

pub async fn list(
    State(state): State<AppState>,
    auth: AuthUser,
    Query(params): Query<ListParams>,
) -> ApiResult<Json<Page<PaymentRequestView>>> {
    let merchant_id = auth
        .merchant_id
        .ok_or_else(|| bad_request(ErrorCode::MerchantNotFound, "no merchant associated with this account"))?;
    let limit = params.limit.unwrap_or(50).clamp(1, 200);
    let cursor = match params.cursor.as_deref() {
        Some(raw) => Some(Cursor::decode(raw).ok_or_else(|| bad_request(ErrorCode::InvalidParameters, "invalid cursor"))?),
        None => None,
    };

    let rows =
        payment_requests::payment_requests_by_merchant_cursor(&state.db, merchant_id, limit, cursor)
            .await
            .map_err(internal)?;

    Ok(Json(Page::new(
        rows.iter().map(row_to_view).collect(),
        limit,
        |v: &PaymentRequestView| Cursor {
            created_at: v.created_at,
            id: v.id,
        },
    )))
}

#[derive(serde::Deserialize)]
pub struct ListParams {
    pub limit: Option<i64>,
    pub cursor: Option<String>,
}

/// A `pending` row whose expiry has passed is reported as `expired` at read
/// time, so a request going stale needs no background job to flip it.
fn effective_status(status: &str, expires_at: DateTime<Utc>) -> String {
    if status == "pending" && expires_at < Utc::now() {
        "expired".to_string()
    } else {
        status.to_string()
    }
}

fn to_view(pr: &PaymentRequest, address: &str, network: &str) -> PaymentRequestView {
    PaymentRequestView {
        id: pr.id,
        merchant_id: pr.merchant_id,
        address: address.to_string(),
        network: network.to_string(),
        amount_stroops: pr.amount_stroops,
        asset: pr.asset.clone(),
        memo: pr.memo.clone(),
        status: effective_status(&pr.status, pr.expires_at),
        expires_at: pr.expires_at,
        created_at: pr.created_at,
        sep7_uri: build_sep7_uri(address, pr.amount_stroops, &pr.asset, &pr.memo),
    }
}

fn row_to_view(row: &payment_requests::PaymentRequestWithWallet) -> PaymentRequestView {
    PaymentRequestView {
        id: row.id,
        merchant_id: row.merchant_id,
        address: row.address.clone(),
        network: row.network.clone(),
        amount_stroops: row.amount_stroops,
        asset: row.asset.clone(),
        memo: row.memo.clone(),
        status: effective_status(&row.status, row.expires_at),
        expires_at: row.expires_at,
        created_at: row.created_at,
        sep7_uri: build_sep7_uri(&row.address, row.amount_stroops, &row.asset, &row.memo),
    }
}

/// Percent-encodes a value for safe interpolation into a URI query string.
///
/// Encodes everything outside the unreserved set (`A-Z a-z 0-9 - . _ ~`), so
/// query delimiters like `&`, `=`, `#`, `?` and `+` cannot break out of the
/// parameter they belong to.
fn encode_query_value(value: &str) -> String {
    const UNRESERVED: &percent_encoding::AsciiSet = &percent_encoding::NON_ALPHANUMERIC
        .remove(b'-')
        .remove(b'.')
        .remove(b'_')
        .remove(b'~');
    percent_encoding::utf8_percent_encode(value, UNRESERVED).to_string()
}

fn build_sep7_uri(address: &str, amount_stroops: i64, asset: &str, memo: &str) -> Option<String> {
    if asset != "XLM" && asset != "native" {
        return None;
    }
    let amount = format!("{}.{:07}", amount_stroops / 10_000_000, amount_stroops % 10_000_000);
    Some(format!(
        "web+stellar:pay?destination={}&amount={}&memo={}&memo_type=MEMO_TEXT",
        encode_query_value(address),
        encode_query_value(&amount),
        encode_query_value(memo),
    ))
}

fn map_payment_request_error(
    err: payment_requests::PaymentRequestError,
) -> (axum::http::StatusCode, Json<crate::error::ApiError>) {
    match err {
        payment_requests::PaymentRequestError::InvalidAmount => {
            bad_request(ErrorCode::InvalidAmount, "amount_stroops must be positive")
        }
        payment_requests::PaymentRequestError::Database(e) => internal(e),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sep7_uri_encodes_memo_special_characters() {
        let memo = "a&b=c#d";
        let uri = build_sep7_uri("GABCDEF", 10_000_000, "XLM", memo).expect("XLM yields a URI");

        // Raw delimiters must not survive into the query string.
        assert!(!uri.contains("&b"), "raw `&` leaked into URI: {uri}");
        assert!(!uri.contains("=c"), "raw `=` leaked into URI: {uri}");
        assert!(!uri.contains("#d"), "raw `#` leaked into URI: {uri}");

        // They must appear percent-encoded instead.
        assert!(uri.contains("memo=a%26b%3Dc%23d"), "unexpected URI: {uri}");

        // The URI still parses to the expected parameters.
        let query = uri
            .strip_prefix("web+stellar:pay?")
            .expect("expected SEP-7 prefix");
        let params: std::collections::HashMap<_, _> = query
            .split('&')
            .map(|pair| pair.split_once('=').expect("key=value pair"))
            .collect();
        assert_eq!(params.get("destination"), Some(&"GABCDEF"));
        assert_eq!(params.get("amount"), Some(&"1.0000000"));
        assert_eq!(params.get("memo"), Some(&"a%26b%3Dc%23d"));
        assert_eq!(params.get("memo_type"), Some(&"MEMO_TEXT"));
    }

    #[test]
    fn sep7_uri_encodes_address_and_amount() {
        let uri = build_sep7_uri("G&addr", 12_345_678, "XLM", "deadbeef").expect("XLM yields a URI");
        assert!(uri.contains("destination=G%26addr"), "unexpected URI: {uri}");
        assert!(uri.contains("amount=1.2345678"), "unexpected URI: {uri}");
    }

    #[test]
    fn sep7_uri_none_for_non_native_asset() {
        assert!(build_sep7_uri("GABCDEF", 1, "cNGN", "deadbeef").is_none());
    }
}
