use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use aframp::{build_state, router, AppConfig};
use axum::extract::State;
use axum::http::{header, HeaderValue, Method, StatusCode};
use axum::response::IntoResponse;
use axum::routing::get;
use axum::{Json, Router};
use serde_json::json;
use tower_http::cors::CorsLayer;
use tower_http::limit::RequestBodyLimitLayer;
use tower_http::trace::TraceLayer;

const HEALTH_CHECK_TIMEOUT: Duration = Duration::from_secs(2);

async fn health_detailed(State(state): State<Arc<aframp::AppState>>) -> impl IntoResponse {
    let db_check = tokio::time::timeout(HEALTH_CHECK_TIMEOUT, sqlx::query("SELECT 1").execute(&state.db));
    let horizon_check = tokio::time::timeout(
        HEALTH_CHECK_TIMEOUT,
        reqwest::Client::new()
            .head(&state.config.stellar_horizon_url)
            .send(),
    );

    let (db_result, horizon_result) = tokio::join!(db_check, horizon_check);

    let database = match db_result {
        Ok(Ok(_)) => "ok",
        _ => "fail",
    };
    let horizon = match horizon_result {
        Ok(Ok(resp)) if resp.status().is_success() => "ok",
        _ => "fail",
    };

    let all_ok = database == "ok" && horizon == "ok";
    let status = if all_ok { "ok" } else { "fail" };
    let code = if all_ok {
        StatusCode::OK
    } else {
        StatusCode::SERVICE_UNAVAILABLE
    };

    let body = json!({
        "status": status,
        "checks": {
            "database": database,
            "horizon": horizon,
            "timestamp": chrono::Utc::now().to_rfc3339(),
        }
    });

    (code, Json(body))
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    dotenvy::dotenv().ok();
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "info".into()),
        )
        .init();

    let config = AppConfig::from_env()?;
    let state = Arc::new(build_state(&config).await?);

    let listener = aframp::blockchain::worker::run(
        state.clone(),
        config.stellar_horizon_url.clone(),
        config.stellar_poll_interval_secs,
    );
    tokio::spawn(listener);

    // Auth travels as an HttpOnly cookie for browsers, so credentials are on —
    // which means origins must be listed explicitly, never mirrored back.
    let origins = config
        .cors_allowed_origins
        .iter()
        .map(|origin| origin.parse::<HeaderValue>())
        .collect::<Result<Vec<_>, _>>()?;
    tracing::info!(?origins, "cors allowed origins");

    if config.cookie.same_site == aframp::SameSite::None {
        tracing::warn!(
            "COOKIE_SAME_SITE=none sends the session on cross-site requests; \
             serve the frontend same-origin instead if you can, or add CSRF tokens"
        );
    }

    let cors = CorsLayer::new()
        .allow_origin(origins)
        .allow_credentials(true)
        .allow_methods([Method::GET, Method::POST])
        .allow_headers([header::AUTHORIZATION, header::CONTENT_TYPE]);

    let app = router((*state).clone())
        .route("/health/detailed", get(health_detailed))
        .layer(cors)
        .layer(TraceLayer::new_for_http())
        .layer(RequestBodyLimitLayer::new(1024 * 1024));

    let address: SocketAddr = config.bind_addr.parse()?;
    tracing::info!(%address, "aframp started");
    axum::serve(tokio::net::TcpListener::bind(address).await?, app).await?;
    Ok(())
}
