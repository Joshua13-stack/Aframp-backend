use axum::extract::FromRequestParts;
use axum::http::request::Parts;
use axum::http::StatusCode;
use axum::Json;
use axum_extra::headers::authorization::Bearer;
use axum_extra::headers::Authorization;
use axum_extra::TypedHeader;

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
#[derive(Debug, Clone)]
pub struct AdminUser;

fn authenticate(parts: &Parts, state: &AppState) -> Result<Claims, (StatusCode, Json<ApiError>)> {
    // API clients send a bearer token; browsers send the HttpOnly session
    // cookie, which JS on the page cannot read. Either proves the session.
    // `TypedHeader<Authorization<Bearer>>` handles case-insensitive scheme
    // matching (e.g. `bearer`), so we fall back to the cookie when the header
    // is absent or malformed.
    let bearer = TypedHeader::<Authorization<Bearer>>::from_request_parts(
        &mut Parts {
            method: parts.method.clone(),
            uri: parts.uri.clone(),
            version: parts.version,
            headers: parts.headers.clone(),
            extensions: Default::default(),
        },
        state,
    )
    .ok()
    .map(|TypedHeader(Authorization(bearer))| bearer.token().to_owned());

    let token = bearer
        .or_else(|| cookie::from_headers(&parts.headers))
        .ok_or_else(|| (StatusCode::UNAUTHORIZED, Json(ApiError { code: ErrorCode::InvalidCredentials, error: "missing session cookie or bearer token".into(), field: None })))?;
    jwt::verify(&state.jwt_secret, &token)
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
        tracing::info!(admin_user_id = %claims.sub, path = %parts.uri.path(), "admin access");
        Ok(AdminUser)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::HeaderValue;

    fn parts_with_authorization(value: &str) -> Parts {
        let mut parts = Parts {
            method: axum::http::Method::GET,
            uri: "/".parse().unwrap(),
            version: axum::http::Version::HTTP_11,
            headers: axum::http::HeaderMap::new(),
            extensions: Default::default(),
        };
        parts
            .headers
            .insert("authorization", HeaderValue::from_str(value).unwrap());
        parts
    }

    fn extract_bearer(parts: &Parts) -> Option<String> {
        TypedHeader::<Authorization<Bearer>>::from_request_parts(
            &mut Parts {
                method: parts.method.clone(),
                uri: parts.uri.clone(),
                version: parts.version,
                headers: parts.headers.clone(),
                extensions: Default::default(),
            },
            &(),
        )
        .ok()
        .map(|TypedHeader(Authorization(bearer))| bearer.token().to_owned())
    }

    #[test]
    fn bearer_scheme_is_case_insensitive() {
        let parts = parts_with_authorization("bearer abc.def.ghi");
        assert_eq!(extract_bearer(&parts).as_deref(), Some("abc.def.ghi"));
    }

    #[test]
    fn bearer_scheme_uppercase() {
        let parts = parts_with_authorization("Bearer abc.def.ghi");
        assert_eq!(extract_bearer(&parts).as_deref(), Some("abc.def.ghi"));
    }
}
