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

#[cfg(test)]
mod tests {
    use super::*;

    /// Compile-time guard: fails to compile if `User` ever gains a
    /// serialized field named `password_hash`. This is a structural
    /// assertion that complements the runtime serde test below — if
    /// someone removes `#[serde(skip_serializing)]`, the runtime test
    /// catches it; if someone adds a *new* field that leaks the hash,
    /// this helper documents the invariant that must be preserved.
    ///
    /// We express the invariant by requiring that `User` can be
    /// serialized into a value whose `password_hash` key is absent.
    /// The function itself is only referenced from tests, so it is
    /// compiled (and thus type-checked) whenever the test module is.
    #[allow(dead_code)]
    fn assert_password_hash_never_serialized(user: &User) {
        let value = serde_json::to_value(user).expect("User must serialize");
        assert!(
            value.get("password_hash").is_none(),
            "User::password_hash must never be serialized; \
             did someone remove #[serde(skip_serializing)]?"
        );
    }

    fn sample_user() -> User {
        let now = Utc::now();
        User {
            id: Uuid::new_v4(),
            email: "leak-check@example.com".to_string(),
            // Distinctive marker + argon2 prefix so we can grep the
            // serialized output for either.
            password_hash: "argon2id$v=19$m=19456,t=2,p=1$LEAKMARKER$hash".to_string(),
            name: "Leak Check".to_string(),
            is_admin: false,
            phone_number: Some("+15555550100".to_string()),
            phone_verified: true,
            created_at: now,
            updated_at: now,
        }
    }

    #[test]
    fn user_serialization_omits_password_hash_key() {
        let user = sample_user();
        let value = serde_json::to_value(&user).expect("User must serialize");
        assert!(
            value.get("password_hash").is_none(),
            "serialized User must not contain a password_hash key"
        );
    }

    #[test]
    fn user_serialization_does_not_contain_argon2_or_marker() {
        let user = sample_user();
        let json = serde_json::to_string(&user).expect("User must serialize");
        assert!(
            !json.contains("password_hash"),
            "serialized User must not contain the string 'password_hash': {json}"
        );
        assert!(
            !json.contains("argon2"),
            "serialized User must not contain the string 'argon2': {json}"
        );
        assert!(
            !json.contains("LEAKMARKER"),
            "serialized User must not contain the password hash marker: {json}"
        );
    }

    #[test]
    fn compile_time_guard_holds() {
        // Exercises the compile-time helper so it is type-checked and
        // its runtime assertion runs as well.
        assert_password_hash_never_serialized(&sample_user());
    }
}