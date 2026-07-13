mod client;
mod device;
mod state;

use std::sync::Arc;

use axum::routing::get;
use axum::Router;
use tower_http::cors::CorsLayer;
use tracing_subscriber::EnvFilter;

use state::AppState;

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .init();

    let admin_password = match std::env::var("ADMIN_PASSWORD") {
        Ok(p) if !p.is_empty() => p,
        _ => {
            tracing::error!("ADMIN_PASSWORD is required and must be non-empty");
            std::process::exit(1);
        }
    };
    let device_token = std::env::var("DEVICE_TOKEN").ok().filter(|t| !t.is_empty());
    if device_token.is_none() {
        tracing::warn!(
            "DEVICE_TOKEN is unset: the /board device endpoint is UNAUTHENTICATED and any \
             client can register devices. Set DEVICE_TOKEN for any exposed deployment."
        );
    }
    let port: u16 = std::env::var("PORT")
        .ok()
        .and_then(|p| p.parse().ok())
        .unwrap_or(8080);

    let state = Arc::new(AppState::new(admin_password, device_token));

    // Periodically reclaim long-disconnected device entries.
    {
        let state = state.clone();
        tokio::spawn(async move {
            let mut ticker = tokio::time::interval(std::time::Duration::from_secs(60));
            loop {
                ticker.tick().await;
                state.sweep_stale();
            }
        });
    }

    let app = Router::new()
        .route("/healthz", get(healthz))
        .route("/api/state", get(state::api_state))
        .route("/board", get(device::board_ws))
        .route("/ws", get(client::client_ws))
        .layer(CorsLayer::permissive())
        .with_state(state);

    let addr = format!("0.0.0.0:{port}");
    let listener = tokio::net::TcpListener::bind(&addr)
        .await
        .expect("bind listener");
    tracing::info!(%addr, "arcade-chess-server listening");
    axum::serve(listener, app).await.expect("serve");
}

async fn healthz() -> &'static str {
    "ok"
}
