mod common;

use axum::http::StatusCode;
use serde_json::json;

use common::{ensure_merchant, send, state};

async fn create_wallet(app: &axum::Router, token: &str) {
    let (status, json) = send(app.clone(), "POST", "/wallet/create", Some(token), Some(json!({}))).await;
    assert_eq!(status, StatusCode::OK, "wallet create failed: {json}");
}

#[tokio::test]
async fn payment_request_requires_wallet() {
    let Some(state) = state().await else {
        return;
    };
    let app = aframp::router(state.clone());
    let (token, _) = ensure_merchant(&app, "pr_no_wallet").await;

    let (status, json) = send(
        app.clone(),
        "POST",
        "/payment-requests",
        Some(&token),
        Some(json!({ "amount_stroops": 10_000_000 })),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "expected rejection: {json}");
    assert_eq!(json["error"], "create a wallet before generating payment requests");
}

#[tokio::test]
async fn payment_request_create_and_fetch_publicly() {
    let Some(state) = state().await else {
        return;
    };
    let app = aframp::router(state.clone());
    let (token, merchant_id) = ensure_merchant(&app, "pr_create").await;
    create_wallet(&app, &token).await;

    let (status, created) = send(
        app.clone(),
        "POST",
        "/payment-requests",
        Some(&token),
        Some(json!({ "amount_stroops": 10_000_000 })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "create failed: {created}");
    assert_eq!(created["status"], "pending");
    assert_eq!(created["asset"], "XLM", "default asset should be XLM, not cNGN");
    assert_eq!(created["merchant_id"], merchant_id);
    assert!(created["memo"].as_str().unwrap().len() >= 8);
    let sep7 = created["sep7_uri"].as_str().expect("XLM requests should have a sep7_uri");
    assert!(sep7.starts_with("web+stellar:pay?destination="));
    assert!(sep7.contains(&format!("memo={}", created["memo"].as_str().unwrap())));

    // Fetch with NO auth token — this must be publicly readable so a
    // customer's wallet app can look it up before paying.
    let id = created["id"].as_str().unwrap();
    let (status, fetched) = send(app.clone(), "GET", &format!("/payment-requests/{id}"), None, None).await;
    assert_eq!(status, StatusCode::OK, "public fetch failed: {fetched}");
    assert_eq!(fetched["id"], created["id"]);
    assert_eq!(fetched["status"], "pending");
}

#[tokio::test]
async fn payment_request_cngn_has_no_sep7_uri_yet() {
    let Some(state) = state().await else {
        return;
    };
    let app = aframp::router(state.clone());
    let (token, _) = ensure_merchant(&app, "pr_cngn").await;
    create_wallet(&app, &token).await;

    let (status, created) = send(
        app.clone(),
        "POST",
        "/payment-requests",
        Some(&token),
        Some(json!({ "amount_stroops": 10_000_000, "asset": "cNGN" })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "create failed: {created}");
    assert_eq!(created["asset"], "cNGN");
    assert!(
        created["sep7_uri"].is_null(),
        "cNGN has no configured issuer address yet, so no QR should be generated"
    );
}

#[tokio::test]
async fn payment_request_reports_expired_past_its_expiry() {
    let Some(state) = state().await else {
        return;
    };
    let app = aframp::router(state.clone());
    let (token, _) = ensure_merchant(&app, "pr_expiry").await;
    create_wallet(&app, &token).await;

    let (status, created) = send(
        app.clone(),
        "POST",
        "/payment-requests",
        Some(&token),
        Some(json!({ "amount_stroops": 5_000_000 })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "create failed: {created}");
    let id = created["id"].as_str().unwrap();

    // Force it into the past directly — no need to actually wait.
    sqlx::query("UPDATE payment_requests SET expires_at = now() - interval '1 minute' WHERE id = $1::uuid")
        .bind(id)
        .execute(&state.db)
        .await
        .unwrap();

    let (status, fetched) = send(app.clone(), "GET", &format!("/payment-requests/{id}"), None, None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(fetched["status"], "expired", "past-expiry pending row should report as expired");
}

#[tokio::test]
async fn payment_request_list_is_scoped_to_the_authenticated_merchant() {
    let Some(state) = state().await else {
        return;
    };
    let app = aframp::router(state.clone());

    let (token_a, _) = ensure_merchant(&app, "pr_list_a").await;
    create_wallet(&app, &token_a).await;
    let (token_b, _) = ensure_merchant(&app, "pr_list_b").await;
    create_wallet(&app, &token_b).await;

    for amount in [10_000_000, 20_000_000] {
        let (status, json) = send(
            app.clone(),
            "POST",
            "/payment-requests",
            Some(&token_a),
            Some(json!({ "amount_stroops": amount })),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "create failed: {json}");
    }
    let (status, json) = send(
        app.clone(),
        "POST",
        "/payment-requests",
        Some(&token_b),
        Some(json!({ "amount_stroops": 99_000_000 })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "create failed: {json}");

    let (status, list_a) = send(app.clone(), "GET", "/payment-requests", Some(&token_a), None).await;
    assert_eq!(status, StatusCode::OK, "list failed: {list_a}");
    let rows = list_a.as_array().unwrap();
    assert_eq!(rows.len(), 2, "merchant A should see only their own two requests");
    // Newest first.
    assert_eq!(rows[0]["amount_stroops"], 20_000_000);
    assert_eq!(rows[1]["amount_stroops"], 10_000_000);
    assert!(
        rows.iter().all(|r| r["sep7_uri"].is_string()),
        "listed XLM requests should each carry a scannable URI"
    );

    let (status, list_b) = send(app.clone(), "GET", "/payment-requests", Some(&token_b), None).await;
    assert_eq!(status, StatusCode::OK);
    let rows_b = list_b.as_array().unwrap();
    assert_eq!(rows_b.len(), 1, "merchant B must not see merchant A's requests");
    assert_eq!(rows_b[0]["amount_stroops"], 99_000_000);
}

#[tokio::test]
async fn payment_request_list_requires_auth() {
    let Some(state) = state().await else {
        return;
    };
    let app = aframp::router(state.clone());
    let (status, _) = send(app.clone(), "GET", "/payment-requests", None, None).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn payment_request_marked_paid_on_memo_correlated_deposit() {
    let Some(state) = state().await else {
        return;
    };
    let app = aframp::router(state.clone());
    let (token, _) = ensure_merchant(&app, "pr_paid").await;
    create_wallet(&app, &token).await;

    let (status, created) = send(
        app.clone(),
        "POST",
        "/payment-requests",
        Some(&token),
        Some(json!({ "amount_stroops": 25_000_000 })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "create failed: {created}");
    let id: uuid::Uuid = created["id"].as_str().unwrap().parse().unwrap();
    let memo = created["memo"].as_str().unwrap();

    // Simulates exactly what blockchain::worker::process_deposit does once it
    // detects a deposit whose memo matches a pending request — without needing
    // a real signed Stellar transaction (that's a separate, unbuilt capability;
    // see PRD's "Stellar transaction creation" row).
    let wallet_id: uuid::Uuid =
        sqlx::query_scalar("SELECT wallet_id FROM payment_requests WHERE id = $1")
            .bind(id)
            .fetch_one(&state.db)
            .await
            .unwrap();
    let pending = aframp::services::payment_requests::find_pending_by_wallet_and_memo(&state.db, wallet_id, memo)
        .await
        .unwrap();
    assert!(pending.is_some(), "should find the pending request by wallet_id + memo");

    let fake_payment_id = uuid::Uuid::new_v4();
    // A real payments row is required by the FK — insert one directly, standing
    // in for what payments::record_deposit would have created from a real
    // detected deposit.
    sqlx::query(
        "INSERT INTO payments (id, merchant_id, wallet_id, wallet_address, tx_hash, amount_stroops, asset, network, status)
         VALUES ($1, $2, $3, 'PLACEHOLDER', $4, 25000000, 'XLM', 'stellar', 'confirmed')",
    )
    .bind(fake_payment_id)
    .bind(created["merchant_id"].as_str().unwrap().parse::<uuid::Uuid>().unwrap())
    .bind(wallet_id)
    .bind(format!("test_tx_{memo}"))
    .execute(&state.db)
    .await
    .unwrap();

    aframp::services::payment_requests::mark_paid(&state.db, id, fake_payment_id)
        .await
        .unwrap();

    let (status, fetched) = send(app.clone(), "GET", &format!("/payment-requests/{id}"), None, None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(fetched["status"], "paid");
}

#[tokio::test]
async fn payment_after_expiry_is_not_correlated_to_the_request() {
    let Some(state) = state().await else {
        return;
    };
    let app = aframp::router(state.clone());
    let (token, _) = ensure_merchant(&app, "pr_expired_pay").await;
    create_wallet(&app, &token).await;

    let (status, created) = send(
        app.clone(),
        "POST",
        "/payment-requests",
        Some(&token),
        Some(json!({ "amount_stroops": 25_000_000 })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "create failed: {created}");
    let id: uuid::Uuid = created["id"].as_str().unwrap().parse().unwrap();
    let memo = created["memo"].as_str().unwrap();

    // Force the request into the past — the DB row is still 'pending', but it
    // must no longer be eligible for memo correlation.
    sqlx::query("UPDATE payment_requests SET expires_at = now() - interval '1 minute' WHERE id = $1")
        .bind(id)
        .execute(&state.db)
        .await
        .unwrap();

    let wallet_id: uuid::Uuid =
        sqlx::query_scalar("SELECT wallet_id FROM payment_requests WHERE id = $1")
            .bind(id)
            .fetch_one(&state.db)
            .await
            .unwrap();

    // A deposit arriving after expiry must NOT be correlated to the request.
    let pending = aframp::services::payment_requests::find_pending_by_wallet_and_memo(&state.db, wallet_id, memo)
        .await
        .unwrap();
    assert!(
        pending.is_none(),
        "an expired request must not be returned by find_pending_by_wallet_and_memo"
    );

    // The request must remain untouched: not paid, not partial.
    let (status, fetched) = send(app.clone(), "GET", &format!("/payment-requests/{id}"), None, None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(fetched["status"], "expired");
    assert_ne!(fetched["status"], "paid");
    assert_ne!(fetched["status"], "partial");

    let db_status: String = sqlx::query_scalar("SELECT status FROM payment_requests WHERE id = $1")
        .bind(id)
        .fetch_one(&state.db)
        .await
        .unwrap();
    assert_eq!(db_status, "pending", "the row itself should be left untouched by the deposit");
}

#[tokio::test]
async fn payment_request_cancel_by_owner_succeeds() {
    let Some(state) = state().await else {
        return;
    };
    let app = aframp::router(state.clone());
    let (token, _) = ensure_merchant(&app, "pr_cancel_ok").await;
    create_wallet(&app, &token).await;

    let (status, created) = send(
        app.clone(),
        "POST",
        "/payment-requests",
        Some(&token),
        Some(json!({ "amount_stroops": 10_000_000 })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "create failed: {created}");
    let id = created["id"].as_str().unwrap();

    let (status, cancelled) = send(
        app.clone(),
        "POST",
        &format!("/payment-requests/{id}/cancel"),
        Some(&token),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "cancel failed: {cancelled}");
    assert_eq!(cancelled["status"], "cancelled");

    let (status, fetched) = send(app.clone(), "GET", &format!("/payment-requests/{id}"), None, None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(fetched["status"], "cancelled");
}

#[tokio::test]
async fn payment_request_cancel_unknown_id_returns_404() {
    let Some(state) = state().await else {
        return;
    };
    let app = aframp::router(state.clone());
    let (token, _) = ensure_merchant(&app, "pr_cancel_404").await;
    create_wallet(&app, &token).await;

    let unknown = uuid::Uuid::new_v4();
    let (status, _) = send(
        app.clone(),
        "POST",
        &format!("/payment-requests/{unknown}/cancel"),
        Some(&token),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn payment_request_cancel_requires_auth() {
    let Some(state) = state().await else {
        return;
    };
    let app = aframp::router(state.clone());
    let (token, _) = ensure_merchant(&app, "pr_cancel_auth").await;
    create_wallet(&app, &token).await;

    let (status, created) = send(
        app.clone(),
        "POST",
        "/payment-requests",
        Some(&token),
        Some(json!({ "amount_stroops": 10_000_000 })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "create failed: {created}");
    let id = created["id"].as_str().unwrap();

    let (status, _) = send(
        app.clone(),
        "POST",
        &format!("/payment-requests/{id}/cancel"),
        None,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn payment_request_cancel_by_other_merchant_is_rejected() {
    let Some(state) = state().await else {
        return;
    };
    let app = aframp::router(state.clone());

    let (token_a, _) = ensure_merchant(&app, "pr_cancel_owner").await;
    create_wallet(&app, &token_a).await;
    let (token_b, _) = ensure_merchant(&app, "pr_cancel_other").await;
    create_wallet(&app, &token_b).await;

    let (status, created) = send(
        app.clone(),
        "POST",
        "/payment-requests",
        Some(&token_a),
        Some(json!({ "amount_stroops": 10_000_000 })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "create failed: {created}");
    let id = created["id"].as_str().unwrap();

    let (status, _) = send(
        app.clone(),
        "POST",
        &format!("/payment-requests/{id}/cancel"),
        Some(&token_b),
        None,
    )
    .await;
    assert!(
        status == StatusCode::NOT_FOUND || status == StatusCode::FORBIDDEN,
        "another merchant must not be able to cancel this request, got {status}"
    );

    // The request must still be pending for its owner.
    let (status, fetched) = send(app.clone(), "GET", &format!("/payment-requests/{id}"), None, None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(fetched["status"], "pending");
}

#[tokio::test]
async fn payment_request_cancel_expired_returns_409() {
    let Some(state) = state().await else {
        return;
    };
    let app = aframp::router(state.clone());
    let (token, _) = ensure_merchant(&app, "pr_cancel_expired").await;
    create_wallet(&app, &token).await;

    let (status, created) = send(
        app.clone(),
        "POST",
        "/payment-requests",
        Some(&token),
        Some(json!({ "amount_stroops": 10_000_000 })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "create failed: {created}");
    let id = created["id"].as_str().unwrap();

    sqlx::query("UPDATE payment_requests SET expires_at = now() - interval '1 minute' WHERE id = $1::uuid")
        .bind(id)
        .execute(&state.db)
        .await
        .unwrap();

    let (status, _) = send(
        app.clone(),
        "POST",
        &format!("/payment-requests/{id}/cancel"),
        Some(&token),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "an expired request cannot be cancelled");
}

#[tokio::test]
async fn payment_request_cancel_paid_returns_409() {
    let Some(state) = state().await else {
        return;
    };
    let app = aframp::router(state.clone());
    let (token, _) = ensure_merchant(&app, "pr_cancel_paid").await;
    create_wallet(&app, &token).await;

    let (status, created) = send(
        app.clone(),
        "POST",
        "/payment-requests",
        Some(&token),
        Some(json!({ "amount_stroops": 25_000_000 })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "create failed: {created}");
    let id: uuid::Uuid = created["id"].as_str().unwrap().parse().unwrap();
    let memo = created["memo"].as_str().unwrap();

    let wallet_id: uuid::Uuid =
        sqlx::query_scalar("SELECT wallet_id FROM payment_requests WHERE id = $1")
            .bind(id)
            .fetch_one(&state.db)
            .await
            .unwrap();

    let fake_payment_id = uuid::Uuid::new_v4();
    sqlx::query(
        "INSERT INTO payments (id, merchant_id, wallet_id, wallet_address, tx_hash, amount_stroops, asset, network, status)
         VALUES ($1, $2, $3, 'PLACEHOLDER', $4, 25000000, 'XLM', 'stellar', 'confirmed')",
    )
    .bind(fake_payment_id)
    .bind(created["merchant_id"].as_str().unwrap().parse::<uuid::Uuid>().unwrap())
    .bind(wallet_id)
    .bind(format!("test_tx_cancel_{memo}"))
    .execute(&state.db)
    .await
    .unwrap();

    aframp::services::payment_requests::mark_paid(&state.db, id, fake_payment_id)
        .await
        .unwrap();

    let (status, _) = send(
        app.clone(),
        "POST",
        &format!("/payment-requests/{id}/cancel"),
        Some(&token),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "a paid request cannot be cancelled");
}
