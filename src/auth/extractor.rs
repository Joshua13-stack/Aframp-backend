use axum::extract::FromRequestParts;
use axum::http::request::Parts;
use axum::http::Method;
use axum::http::StatusCode;
use axum::Json;

use crate::auth::{cookie, jwt};
use crate::auth::jwt::Claims;
use crate::error::{forbidden, ApiError, ErrorCode};
use crate::AppState;

#[derive(Debug, Clone)]
pub struct AuthUser {
    pub user_id: uuid::Uuid,
    pub merchant_id: Option<uuid::Uuid>,
}

/// Same session proof as [`AuthUser`], but additionally requires the `is_admin`
/// JWT claim. The claim is baked in at login and not re-checked against the
/// database, so revoking admin access takes up to [`jwt::TOKEN_TTL_HOURS`] to
/// take effect on outstanding tokens.
///
/// State-changing requests (anything other than `GET`/`HEAD`/`OPTIONS`) that
/// authenticate via the session cookie must also present the per-session CSRF
/// token in the `X-CSRF-Token` header. The token is derived from the session
/// token and the server secret, so it is stable for the life of the session and
/// can be embedded in the admin dashboard HTML as a meta tag. Bearer-token
/// clients are exempt: they are not subject to cross-site cookie riding.
#[derive(Debug, Clone)]
pub struct AdminUser;

fn authenticate(parts: &Parts, state: &AppState) -> Result<Claims, (StatusCode, Json<ApiError>)> {
    // API clients send a bearer token; browsers send the HttpOnly session
    // cookie, which JS on the page cannot read. Either proves the session.
    let token = parts
        .headers
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .or_else(|| cookie::from_headers(&parts.headers))
        .ok_or_else(|| (StatusCode::UNAUTHORIZED, Json(ApiError { code: ErrorCode::InvalidCredentials, error: "missing session cookie or bearer token".into(), field: None })))?;
    jwt::verify(&state.jwt_secret, token)
        .map_err(|_| (StatusCode::UNAUTHORIZED, Json(ApiError { code: ErrorCode::InvalidCredentials, error: "invalid or expired token".into(), field: None })))
}

/// Derive the per-session CSRF token from the raw session token and the server
/// secret. Deterministic so the dashboard can render it once and reuse it for
/// every mutating request without extra server-side storage.
pub fn csrf_token(jwt_secret: &[u8], session_token: &str) -> String {
    use hmac::{Hmac, Mac};
    use sha2::Sha256;
    let mut mac = Hmac::<Sha256>::new_from_slice(jwt_secret)
        .expect("HMAC accepts keys of any size");
    mac.update(b"admin-csrf-v1:");
    mac.update(session_token.as_bytes());
    hex::encode(mac.finalize().into_bytes())
}

fn is_state_changing(method: &Method) -> bool {
    !matches!(*method, Method::GET | Method::HEAD | Method::OPTIONS)
}

impl FromRequestParts<AppState> for AuthUser {
    type Rejection = (StatusCode, Json<ApiError>);

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        let claims = authenticate(parts, state)?;
        Ok(AuthUser {
            user_id: claims.sub,
            merchant_id: claims.merchant_id,
        })
    }
}

impl FromRequestParts<AppState> for AdminUser {
    type Rejection = (StatusCode, Json<ApiError>);

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        let claims = authenticate(parts, state)?;
        if !claims.is_admin {
            return Err(forbidden(ErrorCode::Forbidden, "admin access required"));
        }

        // CSRF protection for cookie-authenticated state-changing requests.
        // Bearer-token clients are exempt (no ambient cookie to ride).
        let via_bearer = parts
            .headers
            .get("authorization")
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.strip_prefix("Bearer "))
            .is_some();
        if !via_bearer && is_state_changing(&parts.method) {
            let session_token = cookie::from_headers(&parts.headers).ok_or_else(|| {
                forbidden(ErrorCode::Forbidden, "missing session cookie")
            })?;
            let expected = csrf_token(&state.jwt_secret, &session_token);
            let provided = parts
                .headers
                .get("x-csrf-token")
                .and_then(|v| v.to_str().ok())
                .unwrap_or_default();
            if !constant_time_eq(expected.as_bytes(), provided.as_bytes()) {
                return Err(forbidden(ErrorCode::Forbidden, "invalid or missing CSRF token"));
            }
        }

        tracing::info!(admin_user_id = %claims.sub, path = %parts.uri.path(), "admin access");
        Ok(AdminUser)
    }
}

fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut diff = 0u8;
    for (x, y) in a.iter().zip(b.iter()) {
        diff |= x ^ y;
    }
    diff == 0
}
