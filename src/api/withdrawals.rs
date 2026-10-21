use axum::extract::{Query, State};
use axum::Json;
use serde::Deserialize;

use crate::auth::extractor::AuthUser;
use crate::error::{bad_gateway, bad_request, bad_request_field, internal, ApiResult, ErrorCode};
use crate::models::{CreateWithdrawalRequest, NewWithdrawal, Withdrawal};
use crate::pagination::{Cursor, Page};
use crate::services::withdrawals::{self, WithdrawalError};
use crate::validation::{is_valid_account_number, is_valid_bank_code};
use crate::AppState;

#[derive(Deserialize)]
pub struct ListParams {
    pub limit: Option<i64>,
    pub cursor: Option<String>,
}

pub async fn create(
    State(state): State<AppState>,
    auth: AuthUser,
    Json(req): Json<CreateWithdrawalRequest>,
) -> ApiResult<Json<Withdrawal>> {
    let merchant_id = auth
        .merchant_id
        .ok_or_else(|| bad_request(ErrorCode::MerchantNotFound, "no merchant associated with this account"))?;
    if req.amount_stroops <= 0 {
        return Err(bad_request_field(
            "amount_stroops",
            "must be a positive number",
        ));
    }
    if !is_valid_bank_code(&req.bank_code) {
        return Err(bad_request_field("bank_code", "must be a 3-digit code"));
    }
    if !is_valid_account_number(&req.account_number) {
        return Err(bad_request_field(
            "account_number",
            "must be a 10-digit NUBAN account number",
        ));
    }
    let withdrawal = withdrawals::create_withdrawal(
        &state.db,
        state.payment_provider.as_ref(),
        NewWithdrawal {
            merchant_id,
            amount_stroops: req.amount_stroops,
            asset: req.asset.unwrap_or_else(|| "cNGN".into()),
            bank_code: req.bank_code,
            account_number: req.account_number,
        },
    )
    .await
    .map_err(map_withdrawal_error)?;
    Ok(Json(withdrawal))
}

pub async fn list(
    State(state): State<AppState>,
    auth: AuthUser,
    Query(params): Query<ListParams>,
) -> ApiResult<Json<Page<Withdrawal>>> {
    let merchant_id = auth
        .merchant_id
        .ok_or_else(|| bad_request(ErrorCode::MerchantNotFound, "no merchant associated with this account"))?;
    let limit = params.limit.unwrap_or(50).clamp(1, 200);
    let cursor = match params.cursor.as_deref() {
        Some(raw) => Some(Cursor::decode(raw).ok_or_else(|| bad_request(ErrorCode::InvalidParameters, "invalid cursor"))?),
        None => None,
    };
    let withdrawals =
        withdrawals::withdrawals_by_merchant_cursor(&state.db, merchant_id, limit, cursor)
            .await
            .map_err(internal)?;
    Ok(Json(Page::new(withdrawals, limit, |w| Cursor {
        created_at: w.created_at,
        id: w.id,
    })))
}

fn map_withdrawal_error(err: WithdrawalError) -> (axum::http::StatusCode, Json<crate::error::ApiError>) {
    match err {
        WithdrawalError::InsufficientBalance => {
            bad_request(ErrorCode::InsufficientBalance, "insufficient available balance")
        }
        WithdrawalError::UnsupportedAsset => bad_request(
            ErrorCode::UnsupportedAsset,
            "withdrawals are only supported for the cNGN asset",
        ),
        WithdrawalError::InvalidAmountPrecision => bad_request(
            ErrorCode::InvalidAmount,
            "amount_stroops must be a whole number of kobo",
        ),
        WithdrawalError::PayoutFailed(msg) => bad_gateway(ErrorCode::PayoutFailed, &msg),
        WithdrawalError::Database(e) => internal(e),
    }
}

#[cfg(test)]
mod pagination_tests {
    use std::collections::HashSet;

    use chrono::{Duration, Utc};
    use uuid::Uuid;

    use crate::pagination::{Cursor, Page};

    /// Minimal row shape mirroring the fields the withdrawals cursor is built from.
    #[derive(Clone)]
    struct Row {
        id: Uuid,
        created_at: chrono::DateTime<Utc>,
    }

    /// In-memory keyset paginator that mirrors the SQL ordering used by
    /// `withdrawals_by_merchant_cursor`: ORDER BY created_at DESC, id DESC.
    fn fetch_page(rows: &[Row], limit: i64, cursor: Option<Cursor>) -> Page<Row> {
        let mut ordered: Vec<Row> = rows.to_vec();
        ordered.sort_by(|a, b| {
            b.created_at
                .cmp(&a.created_at)
                .then_with(|| b.id.cmp(&a.id))
        });
        let filtered: Vec<Row> = ordered
            .into_iter()
            .filter(|r| match &cursor {
                None => true,
                Some(c) => {
                    r.created_at < c.created_at
                        || (r.created_at == c.created_at && r.id < c.id)
                }
            })
            .take(limit as usize)
            .collect();
        Page::new(filtered, limit, |r| Cursor {
            created_at: r.created_at,
            id: r.id,
        })
    }

    fn seed(count: usize) -> Vec<Row> {
        let base = Utc::now();
        (0..count)
            .map(|i| Row {
                id: Uuid::new_v4(),
                created_at: base - Duration::seconds(i as i64),
            })
            .collect()
    }

    #[test]
    fn cursor_pagination_across_multiple_pages() {
        let rows = seed(25);

        // Page 1: 10 items, non-null cursor.
        let page1 = fetch_page(&rows, 10, None);
        assert_eq!(page1.items.len(), 10, "page 1 should have 10 items");
        let cursor1 = page1.next_cursor.clone();
        assert!(cursor1.is_some(), "page 1 should expose a next cursor");

        // Page 2: 10 items, different from page 1, non-null cursor.
        let page2 = fetch_page(&rows, 10, cursor1);
        assert_eq!(page2.items.len(), 10, "page 2 should have 10 items");
        let cursor2 = page2.next_cursor.clone();
        assert!(cursor2.is_some(), "page 2 should expose a next cursor");
        let page1_ids: HashSet<Uuid> = page1.items.iter().map(|r| r.id).collect();
        let page2_ids: HashSet<Uuid> = page2.items.iter().map(|r| r.id).collect();
        assert!(
            page1_ids.is_disjoint(&page2_ids),
            "page 2 must not repeat page 1 rows"
        );

        // Page 3: 5 items, null cursor (last page).
        let page3 = fetch_page(&rows, 10, cursor2);
        assert_eq!(page3.items.len(), 5, "page 3 should have 5 items");
        assert!(page3.next_cursor.is_none(), "last page must have a null cursor");

        // No duplicates or skips across all pages.
        let mut all_ids: Vec<Uuid> = Vec::new();
        all_ids.extend(page1.items.iter().map(|r| r.id));
        all_ids.extend(page2.items.iter().map(|r| r.id));
        all_ids.extend(page3.items.iter().map(|r| r.id));
        let unique: HashSet<Uuid> = all_ids.iter().copied().collect();
        assert_eq!(unique.len(), all_ids.len(), "pagination produced duplicate ids");
        assert_eq!(unique.len(), 25, "pagination skipped rows");
    }

    #[test]
    fn cursor_pagination_stable_with_concurrent_inserts() {
        let mut rows = seed(25);

        // Fetch page 1, then simulate a concurrent insert that sorts before the
        // cursor (newer created_at). It must not shift or duplicate page 2.
        let page1 = fetch_page(&rows, 10, None);
        let cursor1 = page1.next_cursor.clone().expect("page 1 cursor");
        rows.push(Row {
            id: Uuid::new_v4(),
            created_at: Utc::now() + Duration::seconds(1),
        });

        let page2 = fetch_page(&rows, 10, cursor1);
        let page1_ids: HashSet<Uuid> = page1.items.iter().map(|r| r.id).collect();
        let page2_ids: HashSet<Uuid> = page2.items.iter().map(|r| r.id).collect();
        assert_eq!(page2.items.len(), 10, "page 2 should still have 10 items");
        assert!(
            page1_ids.is_disjoint(&page2_ids),
            "concurrent insert must not duplicate rows across pages"
        );
    }
}
