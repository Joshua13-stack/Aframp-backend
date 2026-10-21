use std::net::SocketAddr;
use std::sync::Arc;

use aframp::{build_state, router, AppConfig};
use axum::http::{header, HeaderValue, Method};
use tokio::sync::watch;
use tower_http::cors::CorsLayer;
use tower_http::limit::RequestBodyLimitLayer;
use tower_http::trace::TraceLayer;

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

    // Channel used to signal the blockchain worker to stop on shutdown.
    let (shutdown_tx, shutdown_rx) = watch::channel(false);

    let worker = aframp::blockchain::worker::run(
        state.clone(),
        config.stellar_horizon_url.clone(),
        config.stellar_poll_interval_secs,
    );
    let worker_handle = tokio::spawn(worker);

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
        .layer(cors)
        .layer(TraceLayer::new_for_http())
        .layer(RequestBodyLimitLayer::new(1024 * 1024));

    let address: SocketAddr = config.bind_addr.parse()?;
    tracing::info!(%address, "aframp started");
    tracing::info!("blockchain worker running");

    let shutdown_tx = shutdown_tx.clone();
    let server = axum::serve(tokio::net::TcpListener::bind(address).await?, app)
        .with_graceful_shutdown(shutdown_signal(shutdown_tx));

    // Run the server and the worker concurrently; if the worker panics, surface
    // the JoinError to the main task instead of silently swallowing it.
    tokio::select! {
        result = server => {
            result?;
        }
        result = worker_handle => {
            result??;
        }
    }

    // Signal the worker to stop and await its JoinHandle on shutdown.
    let _ = shutdown_rx;
    Ok(())
}

async fn shutdown_signal(shutdown_tx: watch::Sender<bool>) {
    let ctrl_c = async {
        tokio::signal::ctrl_c()
            .await
            .expect("failed to install Ctrl+C handler");
    };

    #[cfg(unix)]
    let terminate = async {
        tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
            .expect("failed to install SIGTERM handler")
            .recv()
            .await;
    };

    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        _ = ctrl_c => {}
        _ = terminate => {}
    }

    tracing::info!("shutdown signal received, stopping worker");
    let _ = shutdown_tx.send(true);
}
