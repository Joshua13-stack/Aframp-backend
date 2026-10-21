use axum::extract::{Query, State};
use axum::response::Html;
use axum::Json;
use serde::Deserialize;

use crate::auth::extractor::AdminUser;
use crate::error::{internal, ApiResult};
use crate::models::{
    AdminMerchantRow, AdminOverview, AdminPaymentRequestRow, AdminTransactionRow, AdminUserRow,
    AdminWalletRow, AdminWithdrawalRow,
};
use crate::services::admin;
use crate::AppState;

#[derive(Deserialize)]
pub struct ListParams {
    pub limit: Option<i64>,
}

fn limit(params: &ListParams) -> i64 {
    params.limit.unwrap_or(100).clamp(1, 500)
}

pub async fn overview(
    State(state): State<AppState>,
    _admin: AdminUser,
) -> ApiResult<Json<AdminOverview>> {
    let overview = admin::overview(&state.db).await.map_err(internal)?;
    Ok(Json(overview))
}

pub async fn users(
    State(state): State<AppState>,
    _admin: AdminUser,
    Query(params): Query<ListParams>,
) -> ApiResult<Json<Vec<AdminUserRow>>> {
    let rows = admin::users(&state.db, limit(&params)).await.map_err(internal)?;
    Ok(Json(rows))
}

pub async fn merchants(
    State(state): State<AppState>,
    _admin: AdminUser,
    Query(params): Query<ListParams>,
) -> ApiResult<Json<Vec<AdminMerchantRow>>> {
    let rows = admin::merchants(&state.db, limit(&params)).await.map_err(internal)?;
    Ok(Json(rows))
}

pub async fn wallets(
    State(state): State<AppState>,
    _admin: AdminUser,
    Query(params): Query<ListParams>,
) -> ApiResult<Json<Vec<AdminWalletRow>>> {
    let rows = admin::wallets(&state.db, limit(&params)).await.map_err(internal)?;
    Ok(Json(rows))
}

pub async fn transactions(
    State(state): State<AppState>,
    _admin: AdminUser,
    Query(params): Query<ListParams>,
) -> ApiResult<Json<Vec<AdminTransactionRow>>> {
    let rows = admin::transactions(&state.db, limit(&params)).await.map_err(internal)?;
    Ok(Json(rows))
}

pub async fn withdrawals(
    State(state): State<AppState>,
    _admin: AdminUser,
    Query(params): Query<ListParams>,
) -> ApiResult<Json<Vec<AdminWithdrawalRow>>> {
    let rows = admin::withdrawals(&state.db, limit(&params)).await.map_err(internal)?;
    Ok(Json(rows))
}

pub async fn payment_requests(
    State(state): State<AppState>,
    _admin: AdminUser,
    Query(params): Query<ListParams>,
) -> ApiResult<Json<Vec<AdminPaymentRequestRow>>> {
    let rows = admin::payment_requests(&state.db, limit(&params))
        .await
        .map_err(internal)?;
    Ok(Json(rows))
}

/// Static dashboard shell. Unauthenticated by design — it's markup and JS with
/// no data baked in. It logs in through the normal `/login` endpoint (setting
/// the same HttpOnly session cookie a browser client would get) and every data
/// fetch after that rides the cookie same-origin; nothing here bypasses the
/// `AdminUser` check on the JSON routes below.
pub async fn dashboard() -> Html<&'static str> {
    Html(include_str!("admin_dashboard.html"))
}

#[cfg(test)]
mod tests {
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use tower::ServiceExt;

    use crate::auth::jwt;
    use crate::routes::build_router;
    use crate::AppState;

    async fn test_state() -> AppState {
        AppState::for_tests().await
    }

    async fn body_json(resp: axum::response::Response) -> serde_json::Value {
        let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .expect("read body");
        serde_json::from_slice(&bytes).expect("parse json")
    }

    async fn post_json(
        app: &axum::Router,
        uri: &str,
        payload: serde_json::Value,
    ) -> axum::response::Response {
        app.clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(uri)
                    .header("content-type", "application/json")
                    .body(Body::from(payload.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap()
    }

    async fn get_with_token(
        app: &axum::Router,
        uri: &str,
        token: &str,
    ) -> axum::response::Response {
        app.clone()
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri(uri)
                    .header("authorization", format!("Bearer {token}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap()
    }

    #[tokio::test]
    async fn admin_login_with_phone_returns_challenge_not_token() {
        let state = test_state().await;
        let app = build_router(state.clone());

        let admin = crate::services::users::create_admin_with_phone(
            &state.db,
            "admin-otp@example.com",
            "+15550000001",
        )
        .await
        .expect("create admin");

        let resp = post_json(
            &app,
            "/login",
            serde_json::json!({
                "email": admin.email,
                "password": "password123",
            }),
        )
        .await;

        assert_eq!(resp.status(), StatusCode::OK);
        let body = body_json(resp).await;
        assert!(
            body.get("challenge_id").is_some(),
            "expected challenge response, got {body:?}"
        );
        assert!(
            body.get("token").is_none(),
            "admin with phone must not receive a session token directly"
        );
    }

    #[tokio::test]
    async fn admin_otp_verification_yields_admin_token() {
        let state = test_state().await;
        let app = build_router(state.clone());

        let admin = crate::services::users::create_admin_with_phone(
            &state.db,
            "admin-otp2@example.com",
            "+15550000002",
        )
        .await
        .expect("create admin");

        let login = post_json(
            &app,
            "/login",
            serde_json::json!({
                "email": admin.email,
                "password": "password123",
            }),
        )
        .await;
        let login_body = body_json(login).await;
        let challenge_id = login_body
            .get("challenge_id")
            .and_then(|v| v.as_str())
            .expect("challenge_id")
            .to_string();

        let code = crate::services::otp::peek_code_for_tests(&state.db, &challenge_id)
            .await
            .expect("otp code");

        let verify = post_json(
            &app,
            "/verify-otp",
            serde_json::json!({
                "challenge_id": challenge_id,
                "code": code,
            }),
        )
        .await;
        assert_eq!(verify.status(), StatusCode::OK);
        let verify_body = body_json(verify).await;
        let token = verify_body
            .get("token")
            .and_then(|v| v.as_str())
            .expect("token")
            .to_string();

        let claims = jwt::decode(&token).expect("decode token");
        assert!(claims.is_admin, "admin token must carry is_admin: true");
    }

    #[tokio::test]
    async fn admin_token_can_access_overview() {
        let state = test_state().await;
        let app = build_router(state.clone());

        let admin = crate::services::users::create_admin_with_phone(
            &state.db,
            "admin-otp3@example.com",
            "+15550000003",
        )
        .await
        .expect("create admin");

        let token = jwt::encode(&admin.id, true).expect("encode admin token");
        let resp = get_with_token(&app, "/admin/overview", &token).await;
        assert_eq!(resp.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn non_admin_token_is_forbidden_from_overview() {
        let state = test_state().await;
        let app = build_router(state.clone());

        let user = crate::services::users::create(
            &state.db,
            "plain-user@example.com",
            "password123",
        )
        .await
        .expect("create user");

        let token = jwt::encode(&user.id, false).expect("encode user token");
        let resp = get_with_token(&app, "/admin/overview", &token).await;
        assert_eq!(resp.status(), StatusCode::FORBIDDEN);
    }
}
