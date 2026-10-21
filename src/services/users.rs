use sqlx::PgPool;

use crate::auth::password;
use crate::models::{Merchant, User};

#[derive(Debug, thiserror::Error)]
pub enum UserError {
    #[error("invalid email or password")]
    InvalidCredentials,
    #[error(transparent)]
    Database(#[from] sqlx::Error),
}

/// Materializes a brand-new account. The only caller is
/// `services::otp::verify`, once a signup's OTP challenge passes — never
/// called speculatively, so an unverified phone can never end up attached
/// to a real, loggable-in account. `password_hash` must already be hashed.
///
/// Data model: a merchant's `name` mirrors its owning user's `name` — there
/// is no separate business-name field. Both rows are written here with the
/// same `name`, and any later profile rename must update `users.name` and
/// `merchants.name` together in one transaction (see `update_name`) so the
/// admin view and merchant profile never disagree.
pub async fn create_verified(
    db: &PgPool,
    email: &str,
    password_hash: &str,
    name: &str,
    phone_number: &str,
) -> Result<(User, Merchant), sqlx::Error> {
    let mut tx = db.begin().await?;
    let user = sqlx::query_as::<_, User>(
        "INSERT INTO users (email, password_hash, name, phone_number, phone_verified)
         VALUES ($1, $2, $3, $4, true)
         RETURNING id, email, password_hash, name, is_admin, phone_number, phone_verified, created_at, updated_at",
    )
    .bind(email)
    .bind(password_hash)
    .bind(name)
    .bind(phone_number)
    .fetch_one(&mut *tx)
    .await?;

    let merchant = sqlx::query_as::<_, Merchant>(
        "INSERT INTO merchants (user_id, name)
         VALUES ($1, $2)
         RETURNING id, user_id, name, created_at",
    )
    .bind(user.id)
    .bind(name)
    .fetch_one(&mut *tx)
    .await?;

    tx.commit().await?;
    Ok((user, merchant))
}

/// Renames a user and keeps the linked merchant row in sync. Both writes
/// happen in a single transaction so `users.name` and `merchants.name` can
/// never diverge (see the data-model note on `create_verified`). Returns the
/// updated user and, if one exists, the updated merchant.
pub async fn update_name(
    db: &PgPool,
    user_id: uuid::Uuid,
    name: &str,
) -> Result<(User, Option<Merchant>), sqlx::Error> {
    let mut tx = db.begin().await?;
    let user = sqlx::query_as::<_, User>(
        "UPDATE users SET name = $2, updated_at = now()
          WHERE id = $1
          RETURNING id, email, password_hash, name, is_admin, phone_number, phone_verified, created_at, updated_at",
    )
    .bind(user_id)
    .bind(name)
    .fetch_one(&mut *tx)
    .await?;

    let merchant = sqlx::query_as::<_, Merchant>(
        "UPDATE merchants SET name = $2
          WHERE user_id = $1
          RETURNING id, user_id, name, created_at",
    )
    .bind(user_id)
    .bind(name)
    .fetch_optional(&mut *tx)
    .await?;

    tx.commit().await?;
    Ok((user, merchant))
}

/// Which unique constraint a `23505` violation hit, if any — lets a caller
/// distinguish "email taken" from "phone taken" from an unrelated conflict.
pub(crate) fn unique_violation_field(err: &sqlx::Error) -> Option<&str> {
    match err {
        sqlx::Error::Database(db) if db.code().as_deref() == Some("23505") => db.constraint(),
        _ => None,
    }
}

pub async fn login(db: &PgPool, email: &str, password_raw: &str) -> Result<(User, Option<Merchant>), UserError> {
    let user = sqlx::query_as::<_, User>(
        "SELECT id, email, password_hash, name, is_admin, phone_number, phone_verified, created_at, updated_at
           FROM users WHERE email = $1",
    )
    .bind(email)
    .fetch_optional(db)
    .await?
    .ok_or(UserError::InvalidCredentials)?;

    if !password::verify(password_raw, &user.password_hash) {
        return Err(UserError::InvalidCredentials);
    }

    let merchant = sqlx::query_as::<_, Merchant>(
        "SELECT id, user_id, name, created_at FROM merchants WHERE user_id = $1 LIMIT 1",
    )
    .bind(user.id)
    .fetch_optional(db)
    .await?;

    Ok((user, merchant))
}

pub async fn user_by_id(db: &PgPool, user_id: uuid::Uuid) -> Result<Option<User>, sqlx::Error> {
    sqlx::query_as::<_, User>(
        "SELECT id, email, password_hash, name, is_admin, phone_number, phone_verified, created_at, updated_at
           FROM users WHERE id = $1",
    )
    .bind(user_id)
    .fetch_optional(db)
    .await
}

pub async fn merchant_by_user(db: &PgPool, user_id: uuid::Uuid) -> Result<Option<Merchant>, sqlx::Error> {
    sqlx::query_as::<_, Merchant>(
        "SELECT id, user_id, name, created_at FROM merchants WHERE user_id = $1 LIMIT 1",
    )
    .bind(user_id)
    .fetch_optional(db)
    .await
}

pub async fn merchant_by_id(db: &PgPool, merchant_id: uuid::Uuid) -> Result<Option<Merchant>, sqlx::Error> {
    sqlx::query_as::<_, Merchant>(
        "SELECT id, user_id, name, created_at FROM merchants WHERE id = $1",
    )
    .bind(merchant_id)
    .fetch_optional(db)
    .await
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Verifies that renaming a user keeps `users.name` and `merchants.name`
    /// in sync after `update_name`. Requires a live `DATABASE_URL`; skipped
    /// otherwise so the suite still runs without a database.
    #[tokio::test]
    async fn update_name_keeps_merchant_in_sync() {
        let url = match std::env::var("DATABASE_URL") {
            Ok(url) => url,
            Err(_) => return,
        };
        let db = match PgPool::connect(&url).await {
            Ok(db) => db,
            Err(_) => return,
        };

        let email = format!("name-sync-{}@example.test", uuid::Uuid::new_v4());
        let phone = format!("+234{}", uuid::Uuid::new_v4().as_u128() % 1_000_000_0000);
        let (user, merchant) = create_verified(&db, &email, "hash", "Old Name", &phone)
            .await
            .expect("create_verified");
        assert_eq!(merchant.name, "Old Name");

        let (updated_user, updated_merchant) = update_name(&db, user.id, "New Name")
            .await
            .expect("update_name");
        assert_eq!(updated_user.name, "New Name");
        let updated_merchant = updated_merchant.expect("merchant should exist");
        assert_eq!(updated_merchant.name, "New Name");

        let fetched = merchant_by_user(&db, user.id)
            .await
            .expect("merchant_by_user")
            .expect("merchant row");
        assert_eq!(fetched.name, updated_user.name);

        // Cleanup so repeated runs don't collide.
        let _ = sqlx::query("DELETE FROM merchants WHERE user_id = $1")
            .bind(user.id)
            .execute(&db)
            .await;
        let _ = sqlx::query("DELETE FROM users WHERE id = $1")
            .bind(user.id)
            .execute(&db)
            .await;
    }
}
