mod routes;
mod state;
mod static_assets;
mod ws;

use axum::Router;
use std::time::Duration;

use tokio::net::TcpListener;
use tracing::info;

use crate::state::{AppState, TransferState};

const CLEANUP_INTERVAL: Duration = Duration::from_secs(60);

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "filetransfer=info".into()),
        )
        .init();

    let state = AppState::new();

    // Spawn cleanup task - only removes completed transfers.
    // WaitingForRecipient entries are kept alive as long as the sender WS is connected;
    // when the sender disconnects, the WS handler removes the entry directly.
    let cleanup_state = state.clone();
    tokio::spawn(async move {
        loop {
            tokio::time::sleep(CLEANUP_INTERVAL).await;
            cleanup_state.transfers.retain(|_id, entry| {
                !matches!(entry, TransferState::Done)
            });
        }
    });

    let app = Router::new()
        .route("/", axum::routing::get(routes::sender_page))
        .route("/d/{id}", axum::routing::get(routes::receiver_page))
        .route("/api/transfer/{id}", axum::routing::get(routes::transfer_info))
        .route("/ws/send", axum::routing::get(routes::ws_send))
        .route("/ws/recv/{id}", axum::routing::get(routes::ws_recv))
        .with_state(state);

    let port = std::env::var("PORT").unwrap_or_else(|_| "4010".to_string());
    let addr = format!("0.0.0.0:{port}");
    let listener = TcpListener::bind(&addr).await.unwrap();
    info!("filet listening on http://localhost:{port}");
    axum::serve(listener, app).await.unwrap();
}
