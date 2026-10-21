use axum::extract::FromRequestParts;
use axum::http::request::Parts;
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

/// Multi-level admin role carried on the `admin_role` JWT claim. `None` means
/// the token has no admin role (legacy admin tokens) and is treated as
/// [`AdminRole::SuperAdmin`] for backward compatibility.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AdminRole {
    SuperAdmin,
    Support,
    Finance,
}

impl AdminRole {
    pub fn from_claim(claim: Option<&str>) -> Option<Self> {
        match claim {
            Some("super_admin") => Some(AdminRole::SuperAdmin),
            Some("support") => Some(AdminRole::Support),
            Some("finance") => Some(AdminRole::Finance),
            _ => None,
        }
    }

    /// Financial data (withdrawals, overview balances) is restricted to
    /// `super_admin` and `finance`.
    pub fn can_view_financials(self) -> bool {
        matches!(self, AdminRole::SuperAdmin | AdminRole::Finance)
    }

    /// PII-heavy data (user emails) is restricted to `super_admin` and
    /// `support`.
    pub fn can_view_pii(self) -> bool {
        matches!(self, AdminRole::SuperAdmin | AdminRole::Support)
    }
}

/// Same session proof as [`AuthUser`], but additionally requires the `is_admin`
/// JWT claim. The claim is baked in at login and not re-checked against the
/// database, so revoking admin access takes up to [`jwt::TOKEN_TTL_HOURS`] to
/// take effect on outstanding tokens.
#[derive(Debug, Clone)]
pub struct AdminUser {
    pub user_id: uuid::Uuid,
    pub role: AdminRole,
}

impl AdminUser {
    /// Reject the request unless the admin holds a role allowed to view
    /// financial data.
    pub fn require_financials(&self) -> Result<(), (StatusCode, Json<ApiError>)> {
        if self.role.can_view_financials() {
            Ok(())
        } else {
            Err(forbidden(ErrorCode::Forbidden, "insufficient admin role for financial data"))
        }
    }

    /// Reject the request unless the admin holds a role allowed to view PII.
    pub fn require_pii(&self) -> Result<(), (StatusCode, Json<ApiError>)> {
        if self.role.can_view_pii() {
            Ok(())
        } else {
            Err(forbidden(ErrorCode::Forbidden, "insufficient admin role for user PII"))
        }
    }
}

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
        // Legacy admin tokens without an explicit role are treated as
        // super_admin so existing sessions keep working until they expire.
        let role = AdminRole::from_claim(claims.admin_role.as_deref())
            .unwrap_or(AdminRole::SuperAdmin);
        tracing::info!(admin_user_id = %claims.sub, admin_role = ?role, path = %parts.uri.path(), "admin access");
        Ok(AdminUser {
            user_id: claims.sub,
            role,
        })
    }
}
