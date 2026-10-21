use sqlx::PgPool;
use uuid::Uuid;

use crate::models::{Balance, UpdateBalance};

pub async fn get_balances(
    db: &PgPool,
    merchant_id: Uuid,
) -> Result<Vec<Balance>, sqlx::Error> {
    sqlx::query_as!(
        Balance,
        "SELECT merchant_id, asset, available, pending, updated_at
           FROM balances
          WHERE merchant_id = $1",
        merchant_id
    )
    .fetch_all(db)
    .await
}

pub async fn apply_delta(db: &PgPool, delta: &UpdateBalance) -> Result<(), sqlx::Error> {
    sqlx::query!(
        "INSERT INTO balances (merchant_id, asset, available, pending)
         VALUES ($1, $2, $3, $4)
         ON CONFLICT (merchant_id, asset)
         DO UPDATE SET
           available = balances.available + $3,
           pending = balances.pending + $4,
           updated_at = now()",
        delta.merchant_id,
        delta.asset,
        delta.available_delta,
        delta.pending_delta
    )
    .execute(db)
    .await
    .map(|_| ())
}
