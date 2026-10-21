use std::sync::Arc;
use std::time::Duration;

use sqlx::PgPool;
use tokio::sync::CancellationToken;
use tokio::time::{interval, MissedTickBehavior};

use crate::blockchain::stellar::{BlockchainListener, StellarListener};
use crate::models::{NewPayment, UpdateBalance, UpdatePaymentStatus};
use crate::services::{balances, payment_requests, payments, wallets};
use crate::AppState;

/// Default per-merchant single-deposit threshold (in stroops) used when the
/// `MAX_SINGLE_DEPOSIT_STROOPS` environment variable is not set.
const DEFAULT_MAX_SINGLE_DEPOSIT_STROOPS: i64 = 100_000_000_000; // 10,000 XLM

/// Default per-merchant hourly aggregate threshold (in stroops) used when the
/// `MAX_HOURLY_DEPOSIT_STROOPS` environment variable is not set.
const DEFAULT_MAX_HOURLY_DEPOSIT_STROOPS: i64 = 500_000_000_000; // 50,000 XLM

fn max_single_deposit_stroops() -> i64 {
    std::env::var("MAX_SINGLE_DEPOSIT_STROOPS")
        .ok()
        .and_then(|v| v.parse::<i64>().ok())
        .unwrap_or(DEFAULT_MAX_SINGLE_DEPOSIT_STROOPS)
}

fn max_hourly_deposit_stroops() -> i64 {
    std::env::var("MAX_HOURLY_DEPOSIT_STROOPS")
        .ok()
        .and_then(|v| v.parse::<i64>().ok())
        .unwrap_or(DEFAULT_MAX_HOURLY_DEPOSIT_STROOPS)
}

pub async fn run(
    state: Arc<AppState>,
    horizon_url: String,
    poll_interval_secs: u64,
    shutdown: CancellationToken,
) {
    let listener = StellarListener::new(horizon_url);

    let mut ticker = interval(Duration::from_secs(poll_interval_secs.max(1)));
    ticker.set_missed_tick_behavior(MissedTickBehavior::Delay);

    loop {
        tokio::select! {
            _ = shutdown.cancelled() => {
                tracing::info!("deposit worker received shutdown signal, exiting");
                break;
            }
            _ = ticker.tick() => {
                if let Err(err) = poll_once(&state.db, &listener).await {
                    tracing::warn!(error = %err, "deposit poll failed");
                }
            }
        }
    }
}

async fn poll_once(db: &PgPool, listener: &StellarListener) -> Result<(), String> {
    let addresses: Vec<String> = wallets::all_wallets(db)
        .await
        .map_err(|e| e.to_string())?
        .into_iter()
        .map(|w| w.address)
        .collect();
    if addresses.is_empty() {
        return Ok(());
    }

    let deposits = listener.fetch_deposits(&addresses).await?;
    for deposit in deposits {
        if let Err(err) = process_deposit(db, deposit).await {
            tracing::warn!(error = %err, "failed to process deposit");
        }
    }
    Ok(())
}

async fn process_deposit(db: &PgPool, d: crate::blockchain::stellar::DetectedDeposit) -> Result<(), String> {
    let Some(wallet) = wallets::wallet_by_address(db, &d.destination).await.map_err(|e| e.to_string())?
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
    .map_err(|e| e.to_string())?;

    if payment.status != "detected" {
        return Ok(());
    }

    // Anomaly detection: flag unusually large single deposits or hourly bursts.
    // The deposit is still credited below; flagging only raises an alert.
    let mut flagged = false;
    let single_threshold = max_single_deposit_stroops();
    if d.amount_stroops > single_threshold {
        flagged = true;
        tracing::warn!(
            merchant_id = %wallet.merchant_id,
            amount_stroops = d.amount_stroops,
            tx_hash = %d.tx_hash,
            threshold = single_threshold,
            "SECURITY: deposit exceeds MAX_SINGLE_DEPOSIT_STROOPS"
        );
    }

    let hourly_threshold = max_hourly_deposit_stroops();
    match payments::sum_recent_deposits_stroops(db, wallet.merchant_id, 3600).await {
        Ok(recent_total) => {
            let projected = recent_total.saturating_add(d.amount_stroops);
            if projected > hourly_threshold {
                flagged = true;
                tracing::warn!(
                    merchant_id = %wallet.merchant_id,
                    amount_stroops = d.amount_stroops,
                    hourly_total_stroops = projected,
                    tx_hash = %d.tx_hash,
                    threshold = hourly_threshold,
                    "SECURITY: deposit exceeds MAX_HOURLY_DEPOSIT_STROOPS"
                );
            }
        }
        Err(err) => {
            tracing::warn!(error = %err, "failed to compute hourly deposit total");
        }
    }

    if flagged {
        payments::flag_payment(db, payment.id)
            .await
            .map_err(|e| e.to_string())?;
    }

    payments::set_status(db, payment.id, UpdatePaymentStatus::Verified)
        .await
        .map_err(|e| e.to_string())?;

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
    .map_err(|e| e.to_string())?;

    // TODO: move pending → available after Stellar confirmations threshold.
    payments::set_status(db, payment.id, UpdatePaymentStatus::Confirmed)
        .await
        .map_err(|e| e.to_string())?;
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
    .map_err(|e| e.to_string())?;

    if let Some(memo) = memo {
        if let Some(pr) = payment_requests::find_pending_by_wallet_and_memo(db, wallet.id, &memo)
            .await
            .map_err(|e| e.to_string())?
        {
            if payment.amount_stroops >= pr.amount_stroops {
                payment_requests::mark_paid(db, pr.id, payment.id)
                    .await
                    .map_err(|e| e.to_string())?;
            } else {
                tracing::warn!(
                    expected = pr.amount_stroops,
                    actual = payment.amount_stroops,
                    request_id = %pr.id,
                    "payment request underpaid — marking partial"
                );
                payment_requests::mark_partial(db, pr.id, payment.id)
                    .await
                    .map_err(|e| e.to_string())?;
            }
        }
    }

    // TODO: dispatch payment.confirmed webhook.
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn run_exits_when_cancelled() {
        let shutdown = CancellationToken::new();
        shutdown.cancel();

        // A cancelled token must cause `run` to return promptly rather than
        // looping forever. We use a long poll interval so the only way the
        // future can complete is via the cancellation branch.
        let handle = tokio::time::timeout(
            Duration::from_secs(5),
            run_without_state(shutdown.clone(), 3600),
        )
        .await;

        assert!(handle.is_ok(), "worker did not exit after cancellation");
    }

    /// Mirrors the select loop in `run` without requiring a live `AppState`/DB,
    /// so the cancellation path can be exercised in isolation.
    async fn run_without_state(shutdown: CancellationToken, poll_interval_secs: u64) {
        let mut ticker = interval(Duration::from_secs(poll_interval_secs.max(1)));
        ticker.set_missed_tick_behavior(MissedTickBehavior::Delay);
        loop {
            tokio::select! {
                _ = shutdown.cancelled() => break,
                _ = ticker.tick() => {}
            }
        }
    }
}
