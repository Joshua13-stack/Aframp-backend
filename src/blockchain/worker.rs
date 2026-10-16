use std::sync::Arc;
use std::time::Duration;

use sqlx::PgPool;

use crate::blockchain::stellar::{BlockchainListener, StellarListener};
use crate::models::{NewPayment, UpdateBalance, UpdatePaymentStatus};
use crate::services::{balances, payment_requests, payments, wallets};
use crate::AppState;

pub async fn run(state: Arc<AppState>, horizon_url: String, poll_interval_secs: u64) {
    let listener = StellarListener::new(horizon_url);

    loop {
        if let Err(err) = poll_once(&state.db, &listener).await {
            tracing::warn!(error = %err, "deposit poll failed");
        }
        tokio::time::sleep(Duration::from_secs(poll_interval_secs)).await;
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

    let (payment, inserted) = payments::record_deposit(
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

    // Idempotency guard: if the payment row already existed, a previous poll
    // cycle has already (or is in the process of) crediting the balance. Never
    // mutate balances for an existing row, otherwise a crash between
    // `record_deposit` and `set_status(Verified)` would double-credit the
    // pending balance on every subsequent poll cycle.
    if !inserted {
        return Ok(());
    }

    if payment.status != "detected" {
        return Ok(());
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
    use crate::models::NewPayment;
    use crate::services::{balances, payments, wallets};

    /// Simulates a crash between `record_deposit` (which inserts a row with
    /// status `detected`) and `set_status(Verified)`. On the next poll cycle
    /// `record_deposit` returns the existing row; `process_deposit` must NOT
    /// credit the pending balance a second time.
    #[sqlx::test]
    async fn deposit_reentrancy_does_not_double_credit(db: PgPool) {
        // Arrange: a wallet for the merchant.
        let merchant_id = uuid::Uuid::new_v4();
        let wallet = wallets::create(
            &db,
            crate::models::NewWallet {
                merchant_id,
                address: "GAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAWHF".into(),
                network: "stellar".into(),
            },
        )
        .await
        .expect("create wallet");

        let deposit = crate::blockchain::stellar::DetectedDeposit {
            tx_hash: "deadbeef".into(),
            destination: wallet.address.clone(),
            amount_stroops: 1_000,
            asset: "XLM".into(),
            memo: None,
        };

        // First poll: fresh insert, credits pending once.
        process_deposit(&db, deposit.clone())
            .await
            .expect("first process_deposit");

        // Simulate the crash: force the row back to `detected` so the
        // status-based guard does not fire, mimicking a partial write.
        let existing = payments::record_deposit(
            &db,
            NewPayment {
                merchant_id,
                wallet_id: wallet.id,
                wallet_address: wallet.address.clone(),
                tx_hash: deposit.tx_hash.clone(),
                amount_stroops: deposit.amount_stroops,
                asset: deposit.asset.clone(),
                network: "stellar".into(),
            },
        )
        .await
        .expect("re-record deposit");
        payments::set_status(&db, existing.0.id, crate::models::UpdatePaymentStatus::Detected)
            .await
            .expect("reset to detected");

        // Second poll with the same tx_hash: must be a no-op for balances.
        process_deposit(&db, deposit.clone())
            .await
            .expect("second process_deposit");

        let balance = balances::get(&db, merchant_id, "XLM")
            .await
            .expect("get balance");
        assert_eq!(
            balance.pending_stroops, 0,
            "pending balance must be credited exactly once (already moved to available)"
        );
        assert_eq!(
            balance.available_stroops, deposit.amount_stroops,
            "available balance must be credited exactly once"
        );
    }
}
