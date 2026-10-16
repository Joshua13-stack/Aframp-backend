use sqlx::PgPool;
use uuid::Uuid;

use crate::models::{Balance, UpdateBalance};

pub async fn get_balances(
    db: &PgPool,
    merchant_id: Uuid,
) -> Result<Vec<Balance>, sqlx::Error> {
    sqlx::query_as::<_, Balance>(
        "SELECT merchant_id, asset, available, pending, updated_at
           FROM balances
          WHERE merchant_id = $1",
    )
    .bind(merchant_id)
    .fetch_all(db)
    .await
}

/// Applies a balance delta without any idempotency guard.
///
/// Prefer [`apply_delta_idempotent`] when the delta is derived from a payment
/// so that a partial write or a re-entrant poll cycle cannot double-credit.
pub async fn apply_delta(db: &PgPool, delta: &UpdateBalance) -> Result<(), sqlx::Error> {
    sqlx::query(
        "INSERT INTO balances (merchant_id, asset, available, pending)
         VALUES ($1, $2, $3, $4)
         ON CONFLICT (merchant_id, asset)
         DO UPDATE SET
           available = balances.available + $3,
           pending = balances.pending + $4,
           updated_at = now()",
    )
    .bind(delta.merchant_id)
    .bind(&delta.asset)
    .bind(delta.available_delta)
    .bind(delta.pending_delta)
    .execute(db)
    .await
    .map(|_| ())
}

/// Applies a balance delta at most once per `(payment_id, kind)`.
///
/// Defense-in-depth for issue #1195: even if the worker logic regresses and
/// calls this twice for the same payment (e.g. a payment stuck in `detected`
/// after a crash between `record_deposit` and `set_status(Verified)`), the
/// `balance_credits` unique constraint makes the second call a no-op.
///
/// `kind` distinguishes the individual balance mutations for a payment, e.g.
/// `"pending_credit"` for the initial detected credit and `"promote"` for the
/// pending -> available promotion once the deposit is verified.
///
/// The credit marker and the balance mutation are performed in a single
/// transaction so the marker is only committed if the balance update succeeds.
pub async fn apply_delta_idempotent(
    db: &PgPool,
    delta: &UpdateBalance,
    payment_id: Uuid,
    kind: &str,
) -> Result<(), sqlx::Error> {
    let mut tx = db.begin().await?;

    // Insert the idempotency marker first. If this payment/kind has already
    // been credited, the unique constraint makes this a no-op and we skip the
    // balance mutation entirely.
    let inserted = sqlx::query(
        "INSERT INTO balance_credits (payment_id, kind)
         VALUES ($1, $2)
         ON CONFLICT (payment_id, kind) DO NOTHING",
    )
    .bind(payment_id)
    .bind(kind)
    .execute(&mut *tx)
    .await?
    .rows_affected();

    if inserted == 0 {
        // Already credited for this payment/kind; nothing to do.
        tx.rollback().await?;
        return Ok(());
    }

    sqlx::query(
        "INSERT INTO balances (merchant_id, asset, available, pending)
         VALUES ($1, $2, $3, $4)
         ON CONFLICT (merchant_id, asset)
         DO UPDATE SET
           available = balances.available + $3,
           pending = balances.pending + $4,
           updated_at = now()",
    )
    .bind(delta.merchant_id)
    .bind(&delta.asset)
    .bind(delta.available_delta)
    .bind(delta.pending_delta)
    .execute(&mut *tx)
    .await?;

    tx.commit().await?;
    Ok(())
}