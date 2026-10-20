use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::FromRow;
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, FromRow)]
pub struct User {
    pub id: Uuid,
    pub email: String,
    #[serde(skip_serializing)]
    pub password_hash: String,
    pub name: String,
    pub is_admin: bool,
    /// `None` only for accounts created before phone/OTP verification
    /// existed — they keep logging in without a challenge. Every account
    /// created since always has one.
    pub phone_number: Option<String>,
    pub phone_verified: bool,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct SignupRequest {
    pub email: String,
    pub password: String,
    pub name: String,
    pub phone_number: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct LoginRequest {
    pub email: String,
    pub password: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct AuthResponse {
    pub token: String,
    pub user_id: Uuid,
    pub merchant_id: Option<Uuid>,
}

#[derive(Debug, Clone)]
pub struct NewUser {
    pub email: String,
    pub password_hash: String,
    pub name: String,
}

// Compile-time assertion that `User` remains serializable. If the
// `#[serde(skip_serializing)]` attribute on `password_hash` is ever removed,
// the unit test below will fail, and this assertion guarantees `User` keeps
// implementing `Serialize` so the test can run.
const _: fn() = || {
    fn assert_serialize<T: serde::Serialize>() {}
    assert_serialize::<User>();
};

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_user() -> User {
        let now = Utc::now();
        User {
            id: Uuid::new_v4(),
            email: "user@example.com".to_string(),
            password_hash: "$argon2id$v=19$m=19456,t=2,p=1$c2FsdHNhbHQ$hashvalue".to_string(),
            name: "Test User".to_string(),
            is_admin: false,
            phone_number: Some("+15555550100".to_string()),
            phone_verified: true,
            created_at: now,
            updated_at: now,
        }
    }

    #[test]
    fn user_serialization_never_exposes_password_hash() {
        let user = sample_user();
        let json = serde_json::to_string(&user).expect("User should serialize");

        assert!(
            !json.contains("password_hash"),
            "serialized User must not contain the `password_hash` key: {json}"
        );
        assert!(
            !json.contains("argon2"),
            "serialized User must not contain any argon2 hash material: {json}"
        );
        assert!(
            !json.contains(&user.password_hash),
            "serialized User must not contain the raw password hash value: {json}"
        );

        // Sanity check: other fields are still present.
        assert!(json.contains("user@example.com"));
        assert!(json.contains("Test User"));
    }
}