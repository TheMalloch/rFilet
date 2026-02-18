mod routes;
mod state;
mod static_assets;
mod ws;

use axum::Router;

use tokio::net::TcpListener;
use tracing::info;

use crate::state::AppState;

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "filetransfer=info".into()),
        )
        .init();

    let state = AppState::new();

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
