use std::sync::Arc;
use std::time::Duration;

use sqlx::PgPool;

use crate::blockchain::error::BlockchainError;
use crate::blockchain::stellar::{BlockchainListener, StellarListener};
use crate::models::{NewPayment, UpdateBalance, UpdatePaymentStatus};
use crate::services::{balances, payment_requests, payments, wallets};
use crate::AppState;

pub async fn run(state: Arc<AppState>, horizon_url: String, poll_interval_secs: u64) {
    let listener = StellarListener::new(horizon_url);

    loop {
        if let Err(err) = poll_once(&state.db, &listener).await {
            log_blockchain_error(&err, "deposit poll failed");
        }
        tokio::time::sleep(Duration::from_secs(poll_interval_secs)).await;
    }
}

/// Log a `BlockchainError` at the appropriate severity level:
///
/// - `HttpError`     → `warn`  (transient network issue; next cycle will retry)
/// - `ParseError`    → `error` (bad data from Horizon; retrying won't help)
/// - `DatabaseError` → `error` (platform storage problem; needs attention)
/// - `NotFound`      → `warn`  (can occur transiently during wallet creation)
fn log_blockchain_error(err: &BlockchainError, context: &str) {
    match err {
        BlockchainError::HttpError(_) | BlockchainError::NotFound(_) => {
            tracing::warn!(error = %err, "{context}");
        }
        BlockchainError::ParseError(_) | BlockchainError::DatabaseError(_) => {
            tracing::error!(error = %err, "{context}");
        }
    }
}

async fn poll_once(
    db: &PgPool,
    listener: &StellarListener,
) -> Result<(), BlockchainError> {
    let addresses: Vec<String> = wallets::all_wallets(db)
        .await
        .map_err(|e| BlockchainError::DatabaseError(e.to_string()))?
        .into_iter()
        .map(|w| w.address)
        .collect();
    if addresses.is_empty() {
        return Ok(());
    }

    let deposits = listener.fetch_deposits(&addresses).await?;
    for deposit in deposits {
        if let Err(err) = process_deposit(db, deposit).await {
            log_blockchain_error(&err, "failed to process deposit");
        }
    }
    Ok(())
}

async fn process_deposit(
    db: &PgPool,
    d: crate::blockchain::stellar::DetectedDeposit,
) -> Result<(), BlockchainError> {
    let Some(wallet) = wallets::wallet_by_address(db, &d.destination)
        .await
        .map_err(|e| BlockchainError::DatabaseError(e.to_string()))?
    else {
        return Ok(());
    };
    let memo = d.memo.clone();

    let payment = payments::record_deposit(
        db,
        NewPayment {
            merchant_id: wallet.merchant_id,
            wallet_id: wallet.id,
            wallet_address: wallet.address.clone(),
            tx_hash: d.tx_hash.clone(),
            amount_stroops: d.amount_stroops,
            asset: d.asset.clone(),
            network: "stellar".into(),
        },
    )
    .await
    .map_err(|e| BlockchainError::DatabaseError(e.to_string()))?;

    if payment.status != "detected" {
        return Ok(());
    }

    payments::set_status(db, payment.id, UpdatePaymentStatus::Verified)
        .await
        .map_err(|e| BlockchainError::DatabaseError(e.to_string()))?;

    balances::apply_delta(
        db,
        &UpdateBalance {
            merchant_id: wallet.merchant_id,
            asset: d.asset.clone(),
            available_delta: 0,
            pending_delta: d.amount_stroops,
        },
    )
    .await
    .map_err(|e| BlockchainError::DatabaseError(e.to_string()))?;

    // TODO: move pending → available after Stellar confirmations threshold.
    payments::set_status(db, payment.id, UpdatePaymentStatus::Confirmed)
        .await
        .map_err(|e| BlockchainError::DatabaseError(e.to_string()))?;
    balances::apply_delta(
        db,
        &UpdateBalance {
            merchant_id: wallet.merchant_id,
            asset: d.asset,
            available_delta: d.amount_stroops,
            pending_delta: -d.amount_stroops,
        },
    )
    .await
    .map_err(|e| BlockchainError::DatabaseError(e.to_string()))?;

    if let Some(memo) = memo {
        if let Some(pr) = payment_requests::find_pending_by_wallet_and_memo(db, wallet.id, &memo)
            .await
            .map_err(|e| BlockchainError::DatabaseError(e.to_string()))?
        {
            if payment.amount_stroops >= pr.amount_stroops {
                payment_requests::mark_paid(db, pr.id, payment.id)
                    .await
                    .map_err(|e| BlockchainError::DatabaseError(e.to_string()))?;
            } else {
                tracing::warn!(
                    expected = pr.amount_stroops,
                    actual = payment.amount_stroops,
                    request_id = %pr.id,
                    "payment request underpaid — marking partial"
                );
                payment_requests::mark_partial(db, pr.id, payment.id)
                    .await
                    .map_err(|e| BlockchainError::DatabaseError(e.to_string()))?;
            }
        }
    }

    // TODO: dispatch payment.confirmed webhook.
    Ok(())
}
